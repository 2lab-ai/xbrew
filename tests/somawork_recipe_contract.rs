//! Contract for the generic `macos.formula` backend and the two SomaWork
//! channel recipes that are its first users.
//!
//! The lifecycle half drives the real `xbrew` binary against a **fake brew** on
//! an isolated `HOME`/`PATH`, so every route (install, adopt, version, latest,
//! upgrade, uninstall) is exercised through production code with no test-only
//! branch in it. The fake records each invocation as unit-separator delimited
//! argv, which is what makes "the fully-qualified token stayed one argv item"
//! an observation rather than an assumption.

use std::fs;
use std::path::PathBuf;

const STABLE_TOKEN: &str = "2lab-ai/tap/somawork";
const PREVIEW_TOKEN: &str = "2lab-ai/tap/somawork-preview";

fn recipe(file: &str) -> toml::Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("recipes")
        .join(file);
    let source = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{} must exist: {error}", path.display()));
    toml::from_str(&source)
        .unwrap_or_else(|error| panic!("{} must be valid TOML: {error}", path.display()))
}

#[test]
fn somawork_channels_declare_the_approved_tap_formulae() {
    for (file, name, token) in [
        ("somawork.toml", "somawork", STABLE_TOKEN),
        ("somawork-preview.toml", "somawork-preview", PREVIEW_TOKEN),
    ] {
        let recipe = recipe(file);
        assert_eq!(recipe["name"].as_str(), Some(name));
        assert_eq!(
            recipe["macos"]["formula"].as_str(),
            Some(token),
            "{file}: must install the approved fully-qualified formula"
        );
        assert!(
            recipe["macos"].get("cask").is_none() && recipe["macos"].get("dmg").is_none(),
            "{file}: the formula is the only supported macOS backend"
        );
    }
}

#[test]
fn somawork_channels_ship_no_linux_backend_and_no_shell_installer() {
    for file in ["somawork.toml", "somawork-preview.toml"] {
        let recipe = recipe(file);
        for section in ["arch", "debian", "rhel"] {
            assert!(
                recipe.get(section).is_none(),
                "{file}: macOS ARM64 is v1 scope — do not imply {section} support"
            );
        }
        assert!(
            recipe.get("script").is_none(),
            "{file}: SomaWork installs through Homebrew, never a shell installer"
        );
    }
}

#[test]
fn somawork_descriptions_name_their_channel_and_the_onboarding_role() {
    // Positive *and* negative: `xbrew search`/`info` print these verbatim, so a
    // description that names the other channel would mis-sell which runtime the
    // user is about to install.
    for (file, channel, forbidden) in [
        (
            "somawork.toml",
            "production runtime",
            ["preview", "bleeding-edge"],
        ),
        (
            "somawork-preview.toml",
            "preview runtime",
            ["production", "stable"],
        ),
    ] {
        let recipe = recipe(file);
        let description = recipe["description"]
            .as_str()
            .unwrap_or_else(|| panic!("{file}: description must be a string"))
            .to_lowercase();
        assert!(
            description.contains(channel),
            "{file}: description must name the {channel}"
        );
        assert!(
            description.contains("controller onboarding"),
            "{file}: description must name controller onboarding"
        );
        for term in forbidden {
            assert!(
                !description.contains(term),
                "{file}: description must not claim the other channel ({term:?})"
            );
        }
    }
}

/// The Homebrew bootstrap runs `recipes/brew.toml` rather than carrying its own
/// copy of the installer command, so the one thing that ever curl|bashes
/// Homebrew stays reviewable in one file — and a test can stub it out by name.
#[test]
fn the_brew_recipe_is_the_only_home_of_the_installer() {
    let brew = recipe("brew.toml");
    assert_eq!(
        brew["script"]["provides_bin"].as_str(),
        Some("brew"),
        "brew.toml: the bootstrap probes PATH for this binary"
    );
    let install = brew["script"]["install"]
        .as_str()
        .expect("brew.toml: script.install must be a string");
    assert!(
        install.contains("Homebrew/install"),
        "brew.toml must own the official installer, got: {install}"
    );

    let source = fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("resolve.rs"),
    )
    .expect("read src/resolve.rs");
    assert!(
        !source.contains("Homebrew/install"),
        "resolve.rs must bootstrap through the 'brew' recipe, not a second copy \
         of the installer command"
    );
}

/// The `macos.formula` backend only runs on macOS, so its lifecycle receipts
/// are collected there. Everything above is platform-independent recipe data.
#[cfg(target_os = "macos")]
mod lifecycle {
    use super::{PREVIEW_TOKEN, STABLE_TOKEN};

    use std::fs;
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::process::{Command, Output, Stdio};

    /// Records `brew <argv…>`, one invocation per line with argv items joined
    /// by US (0x1f) so a split token is visible, then answers the handful of
    /// queries xbrew makes. "Installed" formulae are files under $STATE keyed
    /// by the token with `/` flattened, holding the version string.
    ///
    /// It also models Homebrew's trust store, because that is what the
    /// dependency bug lives in: `brew install user/tap/formula` grants implicit
    /// trust to that **one named formula** and never to its tap (Homebrew
    /// `cmd/install.rb` calls `Trust.trust_fully_qualified_items!` on
    /// `args.named` only), so a fully-qualified dependency from the very same
    /// tap is still refused as untrusted. `brew upgrade` has the identical gap
    /// (`cmd/upgrade.rb` trusts `args.named`, then resolves), so both run the
    /// same `resolve_named`. Declaring a dependency via
    /// `Sandbox::declare_dependency` therefore makes an install *or* an upgrade
    /// fail for the real reason unless xbrew trusted the tap first.
    ///
    /// Two knobs let a test pick which Homebrew it is talking to:
    /// `XBREW_TEST_BREW_TRUST_FAILS=1` is one too old to have a `trust`
    /// subcommand, and `XBREW_TEST_BREW_ENFORCES_TRUST=0` is one with no trust
    /// gate at all — together, the pre-trust Homebrew that xbrew must still
    /// work against.
    const FAKE_BREW: &str = r#"#!/bin/sh
set -u
{
  printf 'brew'
  for argument in "$@"; do printf '\037%s' "$argument"; done
  printf '\n'
} >>"$XBREW_TEST_BREW_LOG"

subcommand="${1:-}"
if [ "$#" -gt 0 ]; then shift; fi

token=''
for argument in "$@"; do
  case "$argument" in
    --*) ;;
    *) token="$argument" ;;
  esac
done
key=$(printf '%s' "$token" | tr '/' '_')
state="$XBREW_TEST_BREW_STATE"
mkdir -p "$state"

tap_of() {
  printf '%s' "$1" | awk -F/ 'NF == 3 { printf "%s/%s", $1, $2 }'
}

# A plain (core) name needs no trust; a fully-qualified one is trusted either
# in its own right or through its tap.
is_trusted() {
  case "$1" in
    */*/*) ;;
    *) return 0 ;;
  esac
  if [ -f "$state/trusted-formulae" ] && grep -qxF "$1" "$state/trusted-formulae"; then
    return 0
  fi
  if [ -f "$state/trusted-taps" ] && grep -qxF "$(tap_of "$1")" "$state/trusted-taps"; then
    return 0
  fi
  return 1
}

keg() {
  printf '%s' "${XBREW_TEST_BREW_VERSION:-1.0.0}" \
    >"$state/$(printf '%s' "$1" | tr '/' '_')"
}

# What `brew install` and `brew upgrade` share: implicit trust reaches
# `args.named` and stops there, then the declared dependencies are resolved
# against the trust store as it now stands.
resolve_named() {
  printf '%s\n' "$1" >>"$state/trusted-formulae"
  declared="$state/deps/$(printf '%s' "$1" | tr '/' '_')"
  [ -f "$declared" ] || return 0
  while IFS= read -r dependency; do
    [ -n "$dependency" ] || continue
    if [ "${XBREW_TEST_BREW_ENFORCES_TRUST:-1}" = "1" ] && ! is_trusted "$dependency"; then
      printf 'Error: Refusing to load formula %s from untrusted tap %s.\n' \
        "$dependency" "$(tap_of "$dependency")" >&2
      exit 1
    fi
  done <"$declared"
  while IFS= read -r dependency; do
    [ -n "$dependency" ] || continue
    keg "$dependency"
  done <"$declared"
}

case "$subcommand" in
  list)
    if [ -n "$token" ] && [ -f "$state/$key" ]; then
      printf '%s %s\n' "$token" "$(cat "$state/$key")"
    fi
    ;;
  trust)
    # A Homebrew too old to know `trust` exits non-zero without recording.
    if [ "${XBREW_TEST_BREW_TRUST_FAILS:-0}" = "1" ]; then
      printf 'Error: Unknown command: trust\n' >&2
      exit 1
    fi
    case " $* " in
      *' --tap '*) printf '%s\n' "$token" >>"$state/trusted-taps" ;;
      *' --formula '*) printf '%s\n' "$token" >>"$state/trusted-formulae" ;;
    esac
    ;;
  install)
    resolve_named "$token"
    keg "$token"
    ;;
  upgrade)
    resolve_named "$token"
    printf '%s' "${XBREW_TEST_BREW_LATEST:-2.0.0}" >"$state/$key"
    ;;
  uninstall)
    rm -f "$state/$key"
    ;;
  info)
    printf '{"formulae":[{"versions":{"stable":"%s"}}]}\n' "${XBREW_TEST_BREW_LATEST:-2.0.0}"
    ;;
  *)
    ;;
esac
exit 0
"#;

    /// The only `curl` any sandboxed xbrew can reach.
    ///
    /// Armed, it prints the bootstrap payload — and because the embedded brew
    /// recipe *is* `/bin/bash -c "$(curl -fsSL …)"`, whatever it prints is what
    /// the bootstrap executes. That is the whole point: it makes "the embedded
    /// recipe was the one selected" an observation, with no test-only branch in
    /// production code and no user recipe standing in for it.
    ///
    /// Unarmed it refuses loudly, so a test that reaches the bootstrap without
    /// meaning to fails closed instead of fetching Homebrew's real installer
    /// onto the machine running the suite.
    const FAKE_CURL: &str = r#"#!/bin/sh
payload='@PAYLOAD@'
if [ -f "$payload" ]; then
  cat "$payload"
  exit 0
fi
printf 'xbrew test sandbox: refusing to fetch %s (no bootstrap payload armed)\n' "$*" >&2
exit 1
"#;

    /// Hides the *host's* Homebrew from the xbrew under test.
    ///
    /// Production looks for an off-PATH brew in `$HOMEBREW_PREFIX/bin`, then
    /// `/opt/homebrew/bin`, then `/usr/local/bin` — absolute paths no `HOME`,
    /// `PATH` or env var can redirect. On any machine that actually has
    /// Homebrew (every developer Mac, every macOS CI runner) a test about *not*
    /// finding brew would otherwise find the real one, adopt it, and go on to
    /// run `brew install 2lab-ai/tap/somawork` for real.
    ///
    /// So the child runs under a seatbelt profile that denies reads of those two
    /// locations and nothing else. `is_executable_file` sees `EPERM` where the
    /// host brew lives, which is indistinguishable from absence *to production*
    /// — the fallback order is exercised exactly as written, not bypassed. The
    /// declared prefix stays a real, writable sandbox directory, so the
    /// "declared prefix wins" hop is still tested for real.
    const HIDE_HOST_HOMEBREW: &str = r#"(version 1)
(allow default)
(deny file-read* (subpath "/opt/homebrew"))
(deny file-read* (literal "/usr/local/bin/brew"))
"#;

    /// What the sandbox's stand-in for the Homebrew installer does when the
    /// bootstrap runs it.
    enum Bootstrap {
        /// Installs Homebrew: the fake brew lands on the bootstrap PATH.
        Succeeds,
        /// Installs Homebrew the way the official installer actually does on a
        /// fresh Mac: into `$HOMEBREW_PREFIX/bin`, which the already-running
        /// process's PATH does not carry. Exit 0 is real here — the brew is
        /// there, it just isn't reachable by name yet.
        SucceedsIntoPrefix,
        /// The installer itself fails (no network, a declined prompt, …).
        Fails,
        /// Exits 0 and leaves no `brew` anywhere xbrew looks — not on PATH, not
        /// in the prefix. An installer that claims success and delivered
        /// nothing usable.
        LeavesNothingOnPath,
    }

    struct Sandbox {
        root: PathBuf,
        /// Extra environment for the fake brew — which Homebrew generation this
        /// sandbox is pretending to be.
        brew_env: Vec<(String, String)>,
    }

    impl Sandbox {
        fn new(label: &str) -> Sandbox {
            // `{:?}` on a ThreadId renders `ThreadId(2)`, and parentheses are
            // shell syntax. That matters here for one specific reason: the
            // hostile-token canary lives under this root, and a payload like
            // `touch <root>/canary` inside a would-be shell would die on the
            // parens instead of firing — the canary would look clean while
            // proving nothing. Keep every byte of this path shell-inert.
            let thread: String = format!("{:?}", std::thread::current().id())
                .chars()
                .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            let root = std::env::temp_dir().join(format!(
                "xbrew-somawork-{label}-{}-{thread}",
                std::process::id(),
            ));
            let _ = fs::remove_dir_all(&root);
            for directory in [
                "home/.xbrew/recipes",
                "bin",
                "bootstrap-bin",
                "brew-state",
                "prefix/bin",
            ] {
                fs::create_dir_all(root.join(directory)).expect("create sandbox directory");
            }
            let brew = root.join("bin/brew");
            fs::write(&brew, FAKE_BREW).expect("write fake brew");
            fs::set_permissions(&brew, fs::Permissions::from_mode(0o755))
                .expect("make fake brew executable");
            // Shadow `curl` on every PATH this sandbox hands out, armed or not:
            // it is the single seam through which a bootstrap could otherwise
            // reach the network.
            let payload = root.join("curl-payload.sh").display().to_string();
            for directory in ["bin", "bootstrap-bin"] {
                let curl = root.join(directory).join("curl");
                fs::write(&curl, FAKE_CURL.replace("@PAYLOAD@", &payload))
                    .expect("write fake curl");
                fs::set_permissions(&curl, fs::Permissions::from_mode(0o755))
                    .expect("make fake curl executable");
            }
            Sandbox {
                root,
                brew_env: Vec::new(),
            }
        }

        /// Point the fake at a different Homebrew generation (see `FAKE_BREW`).
        fn brew_env(&mut self, key: &str, value: &str) {
            self.brew_env.push((key.to_string(), value.to_string()));
        }

        /// The one PATH directory a bootstrap installer can write into. It
        /// holds the sandbox `curl` and nothing else until an installer drops a
        /// `brew` beside it, which is how "Homebrew became discoverable
        /// *because* the installer ran" is observed rather than assumed.
        fn bootstrap_bin(&self) -> PathBuf {
            self.root.join("bootstrap-bin")
        }

        /// The prefix this sandbox declares through `HOMEBREW_PREFIX`. Never on
        /// PATH — it stands in for `/opt/homebrew` on a process whose
        /// environment never went through `brew shellenv`.
        fn prefix(&self) -> PathBuf {
            self.root.join("prefix")
        }

        fn prefix_bin(&self) -> PathBuf {
            self.prefix().join("bin")
        }

        /// Put a working `brew` in that off-PATH prefix without running
        /// anything: the machine that already has Homebrew and must not be made
        /// to install it again.
        fn preinstall_brew_in_prefix(&self) {
            fs::copy(self.root.join("bin/brew"), self.prefix_bin().join("brew"))
                .expect("seed brew into the off-PATH prefix");
        }

        fn bootstrap_log(&self) -> PathBuf {
            self.root.join("bootstrap.log")
        }

        /// Arm the sandbox `curl` with the payload the **embedded** brew recipe
        /// will run.
        ///
        /// Deliberately not a user recipe override: the implicit bootstrap must
        /// not read `~/.xbrew/recipes/brew.toml` at all, so a test that stubbed
        /// the installer that way would be testing its own stub. Arming `curl`
        /// instead leaves `recipes/brew.toml` — command and all — as the thing
        /// under test, while the payload records each run and decides where (or
        /// whether) a usable `brew` lands.
        fn brew_bootstrap(&self, outcome: Bootstrap) {
            let action = match outcome {
                Bootstrap::Succeeds => format!(
                    "cp '{}' '{}'\nexit 0\n",
                    self.root.join("bin/brew").display(),
                    self.bootstrap_bin().join("brew").display()
                ),
                Bootstrap::SucceedsIntoPrefix => format!(
                    "cp '{}' '{}'\nexit 0\n",
                    self.root.join("bin/brew").display(),
                    self.prefix_bin().join("brew").display()
                ),
                Bootstrap::Fails => "printf 'installer exploded\\n' >&2\nexit 3\n".to_string(),
                Bootstrap::LeavesNothingOnPath => "exit 0\n".to_string(),
            };
            fs::write(
                self.root.join("curl-payload.sh"),
                format!(
                    "printf 'ran\\n' >>'{}'\n{action}",
                    self.bootstrap_log().display()
                ),
            )
            .expect("arm the bootstrap payload");
        }

        /// How many times the bootstrap installer ran.
        fn bootstrap_runs(&self) -> usize {
            fs::read_to_string(self.bootstrap_log())
                .unwrap_or_default()
                .lines()
                .count()
        }

        fn home(&self) -> PathBuf {
            self.root.join("home")
        }

        fn log(&self) -> PathBuf {
            self.root.join("brew.log")
        }

        /// Pretend `token` is already installed at `version`, so xbrew's
        /// adoption probe finds it.
        fn preinstall(&self, token: &str, version: &str) {
            let key = token.replace('/', "_");
            fs::write(self.root.join("brew-state").join(key), version).expect("seed brew state");
        }

        /// Make `dependency` a fully-qualified requirement of `token`, the way
        /// `2lab-ai/tap/somawork-cli` is a requirement of the SomaWork channel
        /// formulae. The fake refuses the install unless it is trusted.
        fn declare_dependency(&self, token: &str, dependency: &str) {
            let directory = self.root.join("brew-state/deps");
            fs::create_dir_all(&directory).expect("create dependency directory");
            let path = directory.join(token.replace('/', "_"));
            let mut declared = fs::read_to_string(&path).unwrap_or_default();
            declared.push_str(dependency);
            declared.push('\n');
            fs::write(path, declared).expect("seed brew dependencies");
        }

        /// Taps that ended up in the fake's trust store.
        fn trusted_taps(&self) -> Vec<String> {
            fs::read_to_string(self.root.join("brew-state/trusted-taps"))
                .unwrap_or_default()
                .lines()
                .map(str::to_string)
                .collect()
        }

        /// Write a bundle manifest into the sandbox and hand back its path.
        fn manifest(&self, body: &str) -> PathBuf {
            let path = self.root.join("manifest.yaml");
            fs::write(&path, body).expect("write manifest");
            path
        }

        /// A path an injected token would create if validation ever regressed.
        /// Inside the sandbox on purpose: a canary pointing at a shared `/tmp`
        /// name would write to the developer's real filesystem on regression.
        fn canary(&self) -> PathBuf {
            self.root.join("canary-must-not-exist")
        }

        /// The inverse of `canary()`: a path a *wanted* installer leaves behind,
        /// so "the user's own brew.toml ran" is a file on disk rather than an
        /// inference from a successful exit.
        fn user_installer_receipt(&self) -> PathBuf {
            self.root.join("user-brew-installer-ran")
        }

        fn is_installed(&self, token: &str) -> bool {
            self.root
                .join("brew-state")
                .join(token.replace('/', "_"))
                .exists()
        }

        /// Drop a user recipe into the sandbox HOME (`~/.xbrew/recipes`), which
        /// is how a hostile or precedence-probing recipe reaches xbrew without
        /// shipping it as a built-in.
        fn user_recipe(&self, name: &str, body: &str) {
            fs::write(
                self.home()
                    .join(".xbrew/recipes")
                    .join(format!("{name}.toml")),
                format!("name = \"{name}\"\n{body}"),
            )
            .expect("write user recipe");
        }

        fn forget_calls(&self) {
            let _ = fs::remove_file(self.log());
        }

        /// Every recorded brew invocation, argv split back apart.
        fn calls(&self) -> Vec<Vec<String>> {
            let Ok(log) = fs::read_to_string(self.log()) else {
                return Vec::new();
            };
            log.lines()
                .map(|line| line.split('\u{1f}').map(str::to_string).collect())
                .collect()
        }

        fn xbrew(&self, arguments: &[&str]) -> Output {
            self.xbrew_answering(arguments, "")
        }

        /// Run the real binary with brew reachable only as our fake.
        fn xbrew_answering(&self, arguments: &[&str], answers: &str) -> Output {
            self.spawn(
                arguments,
                answers,
                &format!("{}:/usr/bin:/bin", self.root.join("bin").display()),
            )
        }

        /// Run the real binary with no brew anywhere on PATH, but with one
        /// writable PATH directory a bootstrap installer can put it in — a
        /// fresh Mac, where Homebrew is missing and installable.
        fn xbrew_bootstrapping(&self, arguments: &[&str]) -> Output {
            self.spawn(
                arguments,
                "",
                &format!("{}:/usr/bin:/bin", self.bootstrap_bin().display()),
            )
        }

        fn spawn(&self, arguments: &[&str], answers: &str, path: &str) -> Output {
            // `sandbox-exec -p <profile> <program> <args…>`: same binary, same
            // argv, minus any view of the host's Homebrew.
            let mut child = Command::new("/usr/bin/sandbox-exec")
                .arg("-p")
                .arg(HIDE_HOST_HOMEBREW)
                .arg(env!("CARGO_BIN_EXE_xbrew"))
                .args(arguments)
                .env_clear()
                .env("HOME", self.home())
                .env("PATH", path)
                .env("XBREW_TEST_BREW_LOG", self.log())
                .env("XBREW_TEST_BREW_STATE", self.root.join("brew-state"))
                // Homebrew's own declaration of which prefix this environment
                // uses. Always sandbox-local, so a suite running on a machine
                // that really does have /opt/homebrew can never leak a real
                // brew into a test that is about not finding one.
                .env("HOMEBREW_PREFIX", self.prefix())
                .envs(self.brew_env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("launch xbrew");
            child
                .stdin
                .take()
                .expect("xbrew stdin")
                .write_all(answers.as_bytes())
                .expect("write xbrew stdin");
            child.wait_with_output().expect("wait for xbrew")
        }

        /// The tracked record for `name`, straight out of state.json.
        fn record(&self, name: &str) -> Option<serde_json::Value> {
            let path = self.home().join(".xbrew/state.json");
            let data = fs::read_to_string(path).ok()?;
            let state: serde_json::Value = serde_json::from_str(&data).expect("state.json is JSON");
            state["packages"].get(name).cloned()
        }
    }

    impl Drop for Sandbox {
        fn drop(&mut self) {
            if !std::thread::panicking() {
                let _ = fs::remove_dir_all(&self.root);
            }
        }
    }

    /// Shell-inert bytes only: letters, digits, and `._-/`. The hostile-token
    /// canaries are paths under a sandbox root, and they can only *prove*
    /// anything if a shell would treat that path as one plain word. A root
    /// carrying `(`, a space, or a quote would make every canary assertion pass
    /// for the wrong reason, so this is checked rather than assumed — including
    /// the inherited TMPDIR prefix, which this test does not control.
    #[test]
    fn sandbox_paths_are_shell_safe_so_a_canary_can_actually_fire() {
        let sandbox = Sandbox::new("shell-safety");
        for path in [sandbox.root.clone(), sandbox.canary(), sandbox.home()] {
            let text = path.to_string_lossy().into_owned();
            let offender = text
                .chars()
                .find(|c| !(c.is_ascii_alphanumeric() || ".-_/".contains(*c)));
            assert!(
                offender.is_none(),
                "sandbox path {text:?} contains {offender:?}, which a shell would \
                 not read as one plain word — canary assertions would be vacuous"
            );
        }
    }

    /// The isolation above is load-bearing for every "brew was not found" test
    /// in this file, and it is the kind of thing that fails open: if
    /// `sandbox-exec` stopped denying those paths, the affected tests would
    /// quietly start testing the host's Homebrew instead. So the denial is
    /// measured — from inside the sandbox, on this machine, right now.
    ///
    /// `/usr/bin/true` is read through the same profile to keep the check
    /// honest: it proves the profile denies *those* paths rather than simply
    /// failing to launch anything.
    #[test]
    fn the_seatbelt_profile_actually_hides_the_host_homebrew() {
        let script = "for p in /opt/homebrew/bin/brew /usr/local/bin/brew; do \
                      if [ -e \"$p\" ]; then echo \"VISIBLE $p\"; fi; done; echo DONE";
        let output = Command::new("/usr/bin/sandbox-exec")
            .arg("-p")
            .arg(HIDE_HOST_HOMEBREW)
            .arg("/bin/sh")
            .arg("-c")
            .arg(script)
            .output()
            .expect("launch sandbox-exec");
        let text = String::from_utf8_lossy(&output.stdout).into_owned();
        assert!(
            text.contains("DONE"),
            "the profile must still let an ordinary process run: {text}{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            !text.contains("VISIBLE"),
            "a host Homebrew is reachable from inside the test sandbox, so every \
             \"brew was not found\" test in this file is testing the host: {text}"
        );
    }

    fn stdout(output: &Output) -> String {
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    fn stderr(output: &Output) -> String {
        String::from_utf8_lossy(&output.stderr).into_owned()
    }

    fn assert_calls(sandbox: &Sandbox, expected: &[&[&str]]) {
        let actual = sandbox.calls();
        let actual: Vec<Vec<&str>> = actual
            .iter()
            .map(|call| call.iter().map(String::as_str).collect())
            .collect();
        let expected: Vec<Vec<&str>> = expected.iter().map(|call| call.to_vec()).collect();
        assert_eq!(actual, expected, "brew was not called exactly as expected");
    }

    fn formula_record(token: &str) -> serde_json::Value {
        serde_json::json!({
            "backend": "brew",
            "reference": token,
            "kind": "formula",
        })
    }

    #[test]
    fn install_passes_the_fully_qualified_token_as_one_argv_item() {
        let sandbox = Sandbox::new("install");
        let output = sandbox.xbrew(&["install", "somawork"]);
        assert!(
            output.status.success(),
            "install failed: {}",
            stderr(&output)
        );

        // Exactly three calls, in this order: the adoption probe, the tap
        // trust, then the install. No `brew info --formula somawork` — the
        // recipe outranks native lookup — and no `brew tap`: `brew install`
        // already calls `tap&.ensure_installed!` on a fully-qualified name
        // (Homebrew `cmd/install.rb`), so tapping here would be a redundant
        // subprocess. Order matters: trust after the install would come too
        // late for the dependency that install resolves.
        assert_calls(
            &sandbox,
            &[
                &["brew", "list", "--formula", "--versions", STABLE_TOKEN],
                &["brew", "trust", "--tap", "2lab-ai/tap"],
                &["brew", "install", STABLE_TOKEN],
            ],
        );
        assert!(sandbox.is_installed(STABLE_TOKEN));
        assert_eq!(
            sandbox.record("somawork"),
            Some(formula_record(STABLE_TOKEN))
        );
    }

    /// The bug this fix exists for, reproduced end to end.
    ///
    /// Public receipt (isolated HOME, fresh Homebrew, public tap): `xbrew
    /// install somawork-preview` ran `brew install
    /// 2lab-ai/tap/somawork-preview`; Homebrew implicitly trusted **only** that
    /// formula (`brew trust --json v1` showed `trustedformulae` holding it and
    /// `trustedtaps` empty) and then refused its dependency
    /// `2lab-ai/tap/somawork-cli` as coming from an untrusted tap. Trusting the
    /// tap by hand made `brew deps --formula 2lab-ai/tap/somawork-preview` exit
    /// 0. The fake brew reproduces exactly that rule, so without the tap-trust
    /// hop this install fails for the real reason rather than by fiat.
    #[test]
    fn install_resolves_a_same_tap_dependency_that_implicit_trust_would_refuse() {
        let sandbox = Sandbox::new("dependency-trust");
        sandbox.declare_dependency(PREVIEW_TOKEN, "2lab-ai/tap/somawork-cli");

        let output = sandbox.xbrew(&["install", "somawork-preview"]);
        assert!(
            output.status.success(),
            "install must trust the tap before brew resolves the dependency: {}",
            stderr(&output)
        );
        assert!(
            sandbox.is_installed("2lab-ai/tap/somawork-cli"),
            "the same-tap dependency should have been installed alongside the formula"
        );
        assert_eq!(sandbox.trusted_taps(), vec!["2lab-ai/tap".to_string()]);
        assert_eq!(
            sandbox.record("somawork-preview"),
            Some(formula_record(PREVIEW_TOKEN))
        );
    }

    /// Generic, not SomaWork-specific: any tap gets the same treatment, and the
    /// trusted tap is derived from the token itself (its first two components),
    /// never from a hard-coded list.
    #[test]
    fn tap_trust_is_derived_from_whatever_tap_the_recipe_names() {
        let sandbox = Sandbox::new("generic-tap");
        sandbox.user_recipe("vendor-cli", "[macos]\nformula = \"vendor/brews/cli\"\n");
        sandbox.declare_dependency("vendor/brews/cli", "vendor/brews/cli-core");

        let output = sandbox.xbrew(&["install", "vendor-cli"]);
        assert!(
            output.status.success(),
            "a third-party tap must be trusted too: {}",
            stderr(&output)
        );
        assert_calls(
            &sandbox,
            &[
                &[
                    "brew",
                    "list",
                    "--formula",
                    "--versions",
                    "vendor/brews/cli",
                ],
                &["brew", "trust", "--tap", "vendor/brews"],
                &["brew", "install", "vendor/brews/cli"],
            ],
        );
    }

    /// A plain formula name has no tap, so there is nothing to trust — and
    /// trusting something derived from a bare name would be a privilege grant
    /// the recipe never asked for.
    #[test]
    fn a_plain_formula_name_does_not_trust_any_tap() {
        let sandbox = Sandbox::new("plain-name");
        sandbox.user_recipe("plain-cli", "[macos]\nformula = \"ripgrep\"\n");

        let output = sandbox.xbrew(&["install", "plain-cli"]);
        assert!(
            output.status.success(),
            "install failed: {}",
            stderr(&output)
        );
        assert_calls(
            &sandbox,
            &[
                &["brew", "list", "--formula", "--versions", "ripgrep"],
                &["brew", "install", "ripgrep"],
            ],
        );
        assert!(
            sandbox.trusted_taps().is_empty(),
            "a bare name must not trust a tap: {:?}",
            sandbox.trusted_taps()
        );
    }

    /// Validation still runs first: a token that is not a Homebrew reference is
    /// refused before *any* subprocess, so the new trust hop cannot become a
    /// way to smuggle an unvalidated string onto a brew command line.
    #[test]
    fn an_invalid_formula_token_is_refused_before_any_trust_subprocess() {
        for (index, token) in [
            "2lab-ai/tap/somawork/extra",
            "2lab-ai//somawork",
            "-2lab-ai/tap/somawork",
            "2lab-ai/-tap/somawork",
            "2lab-ai/tap/.",
            "2lab-ai/tap",
        ]
        .iter()
        .enumerate()
        {
            let sandbox = Sandbox::new(&format!("no-trust-for-invalid-{index}"));
            let name = format!("invalid-{index}");
            sandbox.user_recipe(&name, &format!("[macos]\nformula = \"{token}\"\n"));

            let output = sandbox.xbrew(&["install", &name]);
            assert!(
                !output.status.success(),
                "token {token:?} must be rejected, but install succeeded"
            );
            assert_eq!(
                sandbox.calls(),
                Vec::<Vec<String>>::new(),
                "token {token:?} reached brew — including, possibly, `brew trust`"
            );
            assert!(
                sandbox.trusted_taps().is_empty(),
                "token {token:?} mutated the trust store"
            );
        }
    }

    /// A manifest `trust:` entry has to do both halves: `brew tap` so plain
    /// names from that tap resolve at all, and `brew trust --tap` so Homebrew
    /// will load what it finds there. Both land before the first entry is
    /// installed, or the entry installs against an untrusted tap.
    #[test]
    fn bundle_taps_and_trusts_each_manifest_tap_before_installing_entries() {
        let sandbox = Sandbox::new("bundle-trust");
        let manifest = sandbox.manifest("trust:\n  - 2lab-ai/tap\nxbrew:\n  - somawork\n");

        let output = sandbox.xbrew(&["bundle", &manifest.to_string_lossy()]);
        assert!(
            output.status.success(),
            "bundle failed: {}",
            stderr(&output)
        );
        assert_calls(
            &sandbox,
            &[
                &["brew", "tap", "2lab-ai/tap"],
                &["brew", "trust", "--tap", "2lab-ai/tap"],
                &["brew", "list", "--formula", "--versions", STABLE_TOKEN],
                &["brew", "trust", "--tap", "2lab-ai/tap"],
                &["brew", "install", STABLE_TOKEN],
                &["brew", "list", "--versions", STABLE_TOKEN],
            ],
        );
        assert_eq!(
            sandbox.record("somawork"),
            Some(formula_record(STABLE_TOKEN))
        );
    }

    /// Manifest taps are user data too. `brew tap`/`brew trust` take them as
    /// one argv item and never through a shell, but a `user/tap`-shaped check
    /// still has to run first — otherwise `- --force` would arrive as a flag.
    #[test]
    fn bundle_refuses_a_tap_token_that_is_not_user_slash_tap() {
        for (index, tap) in [
            "--force",
            "2lab-ai",
            "2lab-ai/tap/somawork",
            "2lab-ai/-tap",
            "",
        ]
        .iter()
        .enumerate()
        {
            let sandbox = Sandbox::new(&format!("bundle-bad-tap-{index}"));
            let manifest =
                sandbox.manifest(&format!("trust:\n  - \"{tap}\"\nxbrew:\n  - somawork\n"));

            let output = sandbox.xbrew(&["bundle", &manifest.to_string_lossy()]);
            assert!(
                !output.status.success(),
                "tap {tap:?} must be refused, but bundle succeeded"
            );
            assert!(
                sandbox
                    .calls()
                    .iter()
                    .all(|call| !call.iter().any(|argument| argument == tap)),
                "tap {tap:?} reached brew: {:?}",
                sandbox.calls()
            );
            assert!(
                sandbox.trusted_taps().is_empty(),
                "tap {tap:?} mutated the trust store"
            );
        }
    }

    #[test]
    fn install_adopts_a_formula_homebrew_already_has() {
        let sandbox = Sandbox::new("adopt");
        sandbox.preinstall(STABLE_TOKEN, "1.4.2");

        let output = sandbox.xbrew(&["install", "somawork"]);
        assert!(output.status.success(), "adopt failed: {}", stderr(&output));
        assert!(
            stdout(&output).contains("already installed"),
            "expected an adoption notice, got: {}",
            stdout(&output)
        );
        // Adoption resolves no dependencies and installs nothing, so it must
        // mutate no trust state either: the probe is the only call.
        assert_calls(
            &sandbox,
            &[&["brew", "list", "--formula", "--versions", STABLE_TOKEN]],
        );
        assert!(
            sandbox.trusted_taps().is_empty(),
            "adopting an already-installed formula must not trust anything: {:?}",
            sandbox.trusted_taps()
        );
        assert_eq!(
            sandbox.record("somawork"),
            Some(formula_record(STABLE_TOKEN))
        );
    }

    #[test]
    fn version_reads_the_recorded_token() {
        let sandbox = Sandbox::new("version");
        assert!(sandbox.xbrew(&["install", "somawork"]).status.success());
        sandbox.forget_calls();

        let output = sandbox.xbrew(&["version", "somawork"]);
        assert!(
            output.status.success(),
            "version failed: {}",
            stderr(&output)
        );
        assert_eq!(stdout(&output).trim(), "1.0.0");
        assert_calls(&sandbox, &[&["brew", "list", "--versions", STABLE_TOKEN]]);
    }

    #[test]
    fn update_queries_latest_and_upgrades_the_formula_without_the_cask_flag() {
        let sandbox = Sandbox::new("update");
        assert!(sandbox.xbrew(&["install", "somawork"]).status.success());
        sandbox.forget_calls();

        let output = sandbox.xbrew_answering(&["update", "somawork"], "y\n");
        assert!(
            output.status.success(),
            "update failed: {}",
            stderr(&output)
        );
        assert!(
            stdout(&output).contains("1 updated."),
            "expected one upgrade, got: {}",
            stdout(&output)
        );
        // Four calls now: installed version, latest version, tap trust, then
        // the upgrade. `brew upgrade` trusts only its named args as well, so
        // the trust hop belongs on this path for the same reason it belongs on
        // install — and it has to precede the upgrade that resolves the deps.
        assert_calls(
            &sandbox,
            &[
                &["brew", "list", "--versions", STABLE_TOKEN],
                &["brew", "info", "--json=v2", "--formula", STABLE_TOKEN],
                &["brew", "trust", "--tap", "2lab-ai/tap"],
                &["brew", "upgrade", STABLE_TOKEN],
            ],
        );
        assert!(sandbox.xbrew(&["version", "somawork"]).status.success());
    }

    /// The upgrade half of the same bug, from a record that has never been
    /// through the fixed install path.
    ///
    /// The formula is seeded as already installed and the tap left untrusted,
    /// so `xbrew install` takes the adoption route and mutates no trust state —
    /// which is exactly the state every machine that installed SomaWork before
    /// this fix is in. `brew upgrade` then trusts only its named argument
    /// (Homebrew `cmd/upgrade.rb` calls `Trust.trust_fully_qualified_items!` on
    /// `args.named` and only afterwards resolves the formulae), so the
    /// same-tap dependency the upgrade pulls in is refused. Without the trust
    /// hop this fails with brew's own untrusted-tap error, not by fiat.
    #[test]
    fn update_trusts_the_tap_before_upgrading_a_formula_adopted_while_untrusted() {
        let sandbox = Sandbox::new("update-adopted-untrusted");
        sandbox.preinstall(STABLE_TOKEN, "1.0.0");
        sandbox.declare_dependency(STABLE_TOKEN, "2lab-ai/tap/somawork-cli");

        // Adoption: the record exists, the trust store is still empty.
        assert!(sandbox.xbrew(&["install", "somawork"]).status.success());
        assert!(
            sandbox.trusted_taps().is_empty(),
            "adoption must leave the tap untrusted — that is the state under test: {:?}",
            sandbox.trusted_taps()
        );
        assert!(!sandbox.is_installed("2lab-ai/tap/somawork-cli"));
        sandbox.forget_calls();

        let output = sandbox.xbrew_answering(&["update", "somawork"], "y\n");
        assert!(
            output.status.success(),
            "update must trust the tap before brew resolves the dependency: {}",
            stderr(&output)
        );
        assert_calls(
            &sandbox,
            &[
                &["brew", "list", "--versions", STABLE_TOKEN],
                &["brew", "info", "--json=v2", "--formula", STABLE_TOKEN],
                &["brew", "trust", "--tap", "2lab-ai/tap"],
                &["brew", "upgrade", STABLE_TOKEN],
            ],
        );
        assert!(
            stdout(&output).contains("1 updated."),
            "expected one upgrade, got: {}",
            stdout(&output)
        );
        assert_eq!(sandbox.trusted_taps(), vec!["2lab-ai/tap".to_string()]);
        assert!(
            sandbox.is_installed("2lab-ai/tap/somawork-cli"),
            "the same-tap dependency should have been pulled in by the upgrade"
        );
    }

    /// A plain formula name has no tap on the upgrade path either.
    #[test]
    fn update_of_a_plain_formula_name_does_not_trust_any_tap() {
        let sandbox = Sandbox::new("update-plain");
        sandbox.user_recipe("plain-cli", "[macos]\nformula = \"ripgrep\"\n");
        assert!(sandbox.xbrew(&["install", "plain-cli"]).status.success());
        sandbox.forget_calls();

        let output = sandbox.xbrew_answering(&["update", "plain-cli"], "y\n");
        assert!(
            output.status.success(),
            "update failed: {}",
            stderr(&output)
        );
        assert_calls(
            &sandbox,
            &[
                &["brew", "list", "--versions", "ripgrep"],
                &["brew", "info", "--json=v2", "--formula", "ripgrep"],
                &["brew", "upgrade", "ripgrep"],
            ],
        );
        assert!(
            sandbox.trusted_taps().is_empty(),
            "a bare name must not trust a tap on upgrade either: {:?}",
            sandbox.trusted_taps()
        );
    }

    /// Tap trust is best-effort, and this is what that buys.
    ///
    /// Against a Homebrew old enough to have no `trust` subcommand (the call
    /// exits non-zero) and no trust gate to satisfy (dependencies resolve
    /// regardless), the install must still go through. A hard failure here
    /// would break every such machine in order to warn about a problem it does
    /// not have. The warning still names the tap, so a user on a Homebrew that
    /// *does* enforce trust knows exactly what to run by hand.
    #[test]
    fn a_tap_trust_that_fails_warns_and_still_installs() {
        let mut sandbox = Sandbox::new("trust-fails");
        sandbox.brew_env("XBREW_TEST_BREW_TRUST_FAILS", "1");
        sandbox.brew_env("XBREW_TEST_BREW_ENFORCES_TRUST", "0");
        sandbox.declare_dependency(STABLE_TOKEN, "2lab-ai/tap/somawork-cli");

        let output = sandbox.xbrew(&["install", "somawork"]);
        assert!(
            output.status.success(),
            "a Homebrew without `brew trust` must still install: {}",
            stderr(&output)
        );
        assert_calls(
            &sandbox,
            &[
                &["brew", "list", "--formula", "--versions", STABLE_TOKEN],
                &["brew", "trust", "--tap", "2lab-ai/tap"],
                &["brew", "install", STABLE_TOKEN],
            ],
        );
        assert!(sandbox.is_installed(STABLE_TOKEN));
        assert!(sandbox.is_installed("2lab-ai/tap/somawork-cli"));
        assert_eq!(
            sandbox.record("somawork"),
            Some(formula_record(STABLE_TOKEN))
        );

        // The warning has to be actionable: it names the tap the user would
        // pass to `brew trust --tap` themselves.
        let warning = stderr(&output)
            .lines()
            .find(|line| line.contains("brew trust --tap"))
            .map(str::to_string)
            .unwrap_or_else(|| panic!("no trust warning in stderr: {}", stderr(&output)));
        assert!(
            warning.contains("2lab-ai/tap"),
            "the warning must name the tap: {warning}"
        );
        // …and nothing else. A diagnostic that echoed the process environment
        // would leak whatever the caller had in it, so pin that it carries only
        // the tap and brew's own message.
        for leak in [
            sandbox.root.to_string_lossy().into_owned(),
            sandbox.home().to_string_lossy().into_owned(),
            sandbox.log().to_string_lossy().into_owned(),
        ] {
            assert!(
                !warning.contains(&leak),
                "the warning leaked {leak}: {warning}"
            );
        }
    }

    /// The same best-effort promise on the upgrade path.
    #[test]
    fn a_tap_trust_that_fails_warns_and_still_upgrades() {
        let mut sandbox = Sandbox::new("trust-fails-upgrade");
        sandbox.brew_env("XBREW_TEST_BREW_ENFORCES_TRUST", "0");
        sandbox.preinstall(STABLE_TOKEN, "1.0.0");
        assert!(sandbox.xbrew(&["install", "somawork"]).status.success());

        sandbox.brew_env("XBREW_TEST_BREW_TRUST_FAILS", "1");
        sandbox.forget_calls();
        let output = sandbox.xbrew_answering(&["update", "somawork"], "y\n");
        assert!(
            output.status.success(),
            "a Homebrew without `brew trust` must still upgrade: {}",
            stderr(&output)
        );
        assert!(
            stdout(&output).contains("1 updated."),
            "expected one upgrade, got: {}",
            stdout(&output)
        );
        assert!(
            stderr(&output).contains("brew trust --tap 2lab-ai/tap"),
            "expected an actionable warning, got: {}",
            stderr(&output)
        );
    }

    #[test]
    fn uninstall_routes_the_formula_record_without_the_cask_flag() {
        let sandbox = Sandbox::new("uninstall");
        assert!(sandbox.xbrew(&["install", "somawork"]).status.success());
        sandbox.forget_calls();

        let output = sandbox.xbrew(&["uninstall", "somawork"]);
        assert!(
            output.status.success(),
            "uninstall failed: {}",
            stderr(&output)
        );
        assert_calls(&sandbox, &[&["brew", "uninstall", STABLE_TOKEN]]);
        assert!(!sandbox.is_installed(STABLE_TOKEN));
        assert_eq!(sandbox.record("somawork"), None);
    }

    #[test]
    fn stable_and_preview_channels_coexist_as_separate_records() {
        let sandbox = Sandbox::new("coexist");
        assert!(sandbox.xbrew(&["install", "somawork"]).status.success());
        assert!(sandbox
            .xbrew(&["install", "somawork-preview"])
            .status
            .success());

        assert_eq!(
            sandbox.record("somawork"),
            Some(formula_record(STABLE_TOKEN))
        );
        assert_eq!(
            sandbox.record("somawork-preview"),
            Some(formula_record(PREVIEW_TOKEN))
        );

        // Removing one channel leaves the other's formula installed *and* its
        // xbrew record intact — the record is the half that drives every later
        // uninstall/update, so asserting only the fake keg would let a
        // regression that dropped the sibling entry pass.
        assert!(sandbox
            .xbrew(&["uninstall", "somawork-preview"])
            .status
            .success());
        assert_eq!(
            sandbox.record("somawork"),
            Some(formula_record(STABLE_TOKEN)),
            "uninstalling preview must leave the stable record untouched"
        );
        assert_eq!(sandbox.record("somawork-preview"), None);
        assert!(sandbox.is_installed(STABLE_TOKEN));
        assert!(!sandbox.is_installed(PREVIEW_TOKEN));
    }

    #[test]
    fn a_formula_recipe_outranks_same_named_native_brew_discovery() {
        let sandbox = Sandbox::new("precedence-native");

        // Without a recipe, xbrew asks brew whether it knows the bare name.
        assert!(sandbox
            .xbrew(&["install", "not-a-recipe-package"])
            .status
            .success());
        assert_eq!(
            sandbox.calls().first().map(Vec::as_slice),
            Some(
                ["brew", "info", "--formula", "not-a-recipe-package"]
                    .map(String::from)
                    .as_slice()
            ),
            "native discovery should probe the bare name"
        );

        // With a recipe, that probe never happens — the recipe is authoritative.
        sandbox.forget_calls();
        assert!(sandbox.xbrew(&["install", "somawork"]).status.success());
        assert!(
            sandbox
                .calls()
                .iter()
                .all(|call| !call.contains(&"somawork".to_string())),
            "xbrew must only ever hand brew the recipe's token: {:?}",
            sandbox.calls()
        );
    }

    /// Tap trust is scoped to `macos.formula`. A fully-qualified **cask** goes
    /// on exactly as before — two calls, no `brew trust` — so this test doubles
    /// as the guard that the formula fix did not leak into the cask path.
    #[test]
    fn a_cask_still_outranks_a_formula_in_the_same_recipe() {
        let sandbox = Sandbox::new("precedence-cask");
        sandbox.user_recipe(
            "both-backends",
            "[macos]\ncask = \"vendor/tap/app\"\nformula = \"vendor/tap/cli\"\n",
        );
        assert!(sandbox
            .xbrew(&["install", "both-backends"])
            .status
            .success());
        assert_calls(
            &sandbox,
            &[
                &["brew", "list", "--cask", "--versions", "vendor/tap/app"],
                &["brew", "install", "--cask", "vendor/tap/app"],
            ],
        );
        assert!(
            sandbox.trusted_taps().is_empty(),
            "the cask path must not have grown a trust step: {:?}",
            sandbox.trusted_taps()
        );
        assert_eq!(
            sandbox.record("both-backends"),
            Some(serde_json::json!({
                "backend": "brew",
                "reference": "vendor/tap/app",
                "kind": "cask",
            }))
        );
    }

    #[test]
    fn a_formula_outranks_a_dmg_and_a_script_in_the_same_recipe() {
        let sandbox = Sandbox::new("precedence-formula");
        sandbox.user_recipe(
            "formula-first",
            concat!(
                "[macos]\n",
                "formula = \"vendor/tap/cli\"\n",
                "dmg = \"https://example.test/App.dmg\"\n",
                "app = \"App.app\"\n",
                "[script]\n",
                "install = \"touch $HOME/script-ran\"\n",
            ),
        );
        assert!(sandbox
            .xbrew(&["install", "formula-first"])
            .status
            .success());
        assert_calls(
            &sandbox,
            &[
                &["brew", "list", "--formula", "--versions", "vendor/tap/cli"],
                &["brew", "trust", "--tap", "vendor/tap"],
                &["brew", "install", "vendor/tap/cli"],
            ],
        );
        assert!(
            !sandbox.home().join("script-ran").exists(),
            "the script backend must not run when a formula is declared"
        );
    }

    /// User recipes override built-ins by name at runtime. That is exactly why
    /// the shipped-recipe contracts in `recipe.rs` read `BUILTIN_RECIPES`
    /// directly instead of `get()`: there, an override would silently become
    /// the thing under test. Here HOME is the sandbox's, so it is the subject.
    #[test]
    fn a_user_recipe_overrides_the_built_in_channel() {
        let sandbox = Sandbox::new("user-override");
        sandbox.user_recipe(
            "somawork",
            "[macos]\nformula = \"vendor/tap/local-somawork\"\n",
        );
        assert!(sandbox.xbrew(&["install", "somawork"]).status.success());
        assert_calls(
            &sandbox,
            &[
                &[
                    "brew",
                    "list",
                    "--formula",
                    "--versions",
                    "vendor/tap/local-somawork",
                ],
                &["brew", "trust", "--tap", "vendor/tap"],
                &["brew", "install", "vendor/tap/local-somawork"],
            ],
        );
        assert_eq!(
            sandbox.record("somawork"),
            Some(formula_record("vendor/tap/local-somawork"))
        );
    }

    /// The bug this replaces a refusal with: a fresh Mac with no Homebrew met
    /// "Run `xbrew install brew` first" for a recipe xbrew already knows how to
    /// install. Homebrew is a *dependency* of the formula backend, so xbrew
    /// installs it through the very `brew` recipe `xbrew install brew` runs —
    /// exactly once — and then carries on with the formula it was asked for.
    #[test]
    fn a_formula_recipe_bootstraps_homebrew_and_then_installs() {
        let sandbox = Sandbox::new("bootstrap-formula");
        sandbox.brew_bootstrap(Bootstrap::Succeeds);

        let output = sandbox.xbrew_bootstrapping(&["install", "somawork"]);
        assert!(
            output.status.success(),
            "install failed: {}",
            stderr(&output)
        );
        assert_eq!(
            sandbox.bootstrap_runs(),
            1,
            "Homebrew must be installed exactly once"
        );
        // The formula path is unchanged afterwards, trust hop and all — the
        // bootstrap adds a step in front of it, it does not replace it.
        assert_calls(
            &sandbox,
            &[
                &["brew", "list", "--formula", "--versions", STABLE_TOKEN],
                &["brew", "trust", "--tap", "2lab-ai/tap"],
                &["brew", "install", STABLE_TOKEN],
            ],
        );
        assert_eq!(
            sandbox.record("somawork"),
            Some(formula_record(STABLE_TOKEN))
        );
        assert!(sandbox.is_installed(STABLE_TOKEN));
        // An implicit dependency is not a package the user asked for: recording
        // it would make `xbrew uninstall brew` an offer xbrew never made, and
        // would claim ownership of a Homebrew the user may already manage.
        assert_eq!(
            sandbox.record("brew"),
            None,
            "the bootstrap must not record brew as a requested package"
        );
    }

    /// The bootstrap is conditional on the dependency actually being missing.
    #[test]
    fn an_existing_homebrew_is_never_bootstrapped_over() {
        let sandbox = Sandbox::new("bootstrap-skip");
        sandbox.brew_bootstrap(Bootstrap::Succeeds);

        let output = sandbox.xbrew(&["install", "somawork"]);
        assert!(
            output.status.success(),
            "install failed: {}",
            stderr(&output)
        );
        assert_eq!(
            sandbox.bootstrap_runs(),
            0,
            "brew was already on PATH — running its installer would re-install \
             (or damage) a Homebrew the user already has"
        );
        assert_calls(
            &sandbox,
            &[
                &["brew", "list", "--formula", "--versions", STABLE_TOKEN],
                &["brew", "trust", "--tap", "2lab-ai/tap"],
                &["brew", "install", STABLE_TOKEN],
            ],
        );
    }

    /// A bootstrap that fails is the end of the install, not a warning: there
    /// is no brew to fall through to, and the error has to name the real cause
    /// rather than the missing dependency it was already trying to fix.
    #[test]
    fn a_failing_homebrew_bootstrap_stops_the_install() {
        let sandbox = Sandbox::new("bootstrap-fails");
        sandbox.brew_bootstrap(Bootstrap::Fails);

        let output = sandbox.xbrew_bootstrapping(&["install", "somawork"]);
        assert!(!output.status.success(), "install should have failed");
        assert!(
            stderr(&output).contains("Homebrew"),
            "the error must name the failed bootstrap, got: {}",
            stderr(&output)
        );
        assert_eq!(
            sandbox.bootstrap_runs(),
            1,
            "a failed installer must not be retried"
        );
        assert_eq!(
            sandbox.calls(),
            Vec::<Vec<String>>::new(),
            "there is no brew to call after a failed bootstrap"
        );
        assert_eq!(sandbox.record("somawork"), None);
    }

    /// Homebrew's installer can exit 0 into a prefix the current shell's PATH
    /// does not carry. Exit status is not the receipt — `brew` being reachable
    /// is — so xbrew re-probes and stops with an instruction the user can act
    /// on, instead of running the formula path into a "command not found".
    #[test]
    fn a_bootstrap_that_leaves_no_brew_on_path_is_not_taken_as_success() {
        let sandbox = Sandbox::new("bootstrap-silent");
        sandbox.brew_bootstrap(Bootstrap::LeavesNothingOnPath);

        let output = sandbox.xbrew_bootstrapping(&["install", "somawork"]);
        assert!(!output.status.success(), "install should have failed");
        assert!(
            stderr(&output).contains("PATH"),
            "the error must point at PATH, got: {}",
            stderr(&output)
        );
        assert_eq!(sandbox.bootstrap_runs(), 1);
        assert_eq!(
            sandbox.calls(),
            Vec::<Vec<String>>::new(),
            "brew was never reachable, so it was never called"
        );
        assert_eq!(sandbox.record("somawork"), None);
    }

    /// Casks need Homebrew for the same reason and get the same bootstrap (one
    /// helper, both branches) — and it must not have dragged the formula path's
    /// trust hop onto the cask path with it.
    #[test]
    fn a_cask_recipe_bootstraps_homebrew_the_same_way() {
        let sandbox = Sandbox::new("bootstrap-cask");
        sandbox.brew_bootstrap(Bootstrap::Succeeds);
        sandbox.user_recipe("cask-only", "[macos]\ncask = \"vendor/tap/app\"\n");

        let output = sandbox.xbrew_bootstrapping(&["install", "cask-only"]);
        assert!(
            output.status.success(),
            "install failed: {}",
            stderr(&output)
        );
        assert_eq!(sandbox.bootstrap_runs(), 1);
        assert_calls(
            &sandbox,
            &[
                &["brew", "list", "--cask", "--versions", "vendor/tap/app"],
                &["brew", "install", "--cask", "vendor/tap/app"],
            ],
        );
        assert!(
            sandbox.trusted_taps().is_empty(),
            "the cask path must not have grown a trust step: {:?}",
            sandbox.trusted_taps()
        );
        assert_eq!(
            sandbox.record("cask-only"),
            Some(serde_json::json!({
                "backend": "brew",
                "reference": "vendor/tap/app",
                "kind": "cask",
            }))
        );
        assert_eq!(sandbox.record("brew"), None);
    }

    /// Homebrew installed, but not on *this* process's PATH — the condition a
    /// real Mac is in whenever the shell that launched xbrew never went through
    /// `brew shellenv`, which is exactly how `/opt/homebrew/bin/brew` can exist
    /// while `command -v brew` finds nothing. Reinstalling Homebrew over that
    /// would be the wrong repair; finding it where Homebrew actually lives and
    /// putting that one verified directory on PATH is the right one.
    ///
    /// The armed bootstrap is what makes `0` mean something, and the brew
    /// transcript is the receipt for the PATH edit: every one of those calls is
    /// a bare `brew` that could only have been resolved through PATH.
    #[test]
    fn an_off_path_homebrew_prefix_is_adopted_instead_of_bootstrapped() {
        let sandbox = Sandbox::new("prefix-adopt");
        sandbox.brew_bootstrap(Bootstrap::Succeeds);
        sandbox.preinstall_brew_in_prefix();

        let output = sandbox.xbrew_bootstrapping(&["install", "somawork"]);
        assert!(
            output.status.success(),
            "install failed: {}",
            stderr(&output)
        );
        assert_eq!(
            sandbox.bootstrap_runs(),
            0,
            "Homebrew was already installed under its prefix — re-running its \
             installer would damage a Homebrew the user already has"
        );
        assert_calls(
            &sandbox,
            &[
                &["brew", "list", "--formula", "--versions", STABLE_TOKEN],
                &["brew", "trust", "--tap", "2lab-ai/tap"],
                &["brew", "install", STABLE_TOKEN],
            ],
        );
        assert!(sandbox.is_installed(STABLE_TOKEN));
        assert_eq!(
            sandbox.record("somawork"),
            Some(formula_record(STABLE_TOKEN))
        );
        assert_eq!(sandbox.record("brew"), None);
    }

    /// The same discovery, one hop later: Homebrew's installer exits 0 having
    /// written into `$HOMEBREW_PREFIX/bin`, which the already-running xbrew's
    /// PATH does not carry. Exit 0 is not the receipt and neither is PATH alone
    /// — a reachable brew is, and this invocation must reach it and carry on
    /// through list/trust/install rather than telling the user to open a new
    /// shell and start over.
    #[test]
    fn a_bootstrap_into_an_off_path_prefix_is_discovered_and_used() {
        let sandbox = Sandbox::new("bootstrap-prefix");
        sandbox.brew_bootstrap(Bootstrap::SucceedsIntoPrefix);

        let output = sandbox.xbrew_bootstrapping(&["install", "somawork"]);
        assert!(
            output.status.success(),
            "install failed: {}",
            stderr(&output)
        );
        assert_eq!(sandbox.bootstrap_runs(), 1);
        assert_calls(
            &sandbox,
            &[
                &["brew", "list", "--formula", "--versions", STABLE_TOKEN],
                &["brew", "trust", "--tap", "2lab-ai/tap"],
                &["brew", "install", STABLE_TOKEN],
            ],
        );
        assert!(sandbox.is_installed(STABLE_TOKEN));
        assert_eq!(
            sandbox.record("somawork"),
            Some(formula_record(STABLE_TOKEN))
        );
        assert_eq!(sandbox.record("brew"), None);
    }

    /// Homebrew's installer is not something to run twice because two packages
    /// happened to want it. One attempt per invocation: the second package
    /// reuses the first attempt's failure — the *real* one, exit status and all,
    /// not a fresh "brew isn't installed" that hides why.
    #[test]
    fn a_failed_bootstrap_runs_once_and_every_later_package_reuses_it() {
        let sandbox = Sandbox::new("bootstrap-once-failed");
        sandbox.brew_bootstrap(Bootstrap::Fails);

        let output = sandbox.xbrew_bootstrapping(&["install", "somawork", "somawork-preview"]);
        assert!(!output.status.success(), "install should have failed");
        assert_eq!(
            sandbox.bootstrap_runs(),
            1,
            "a failed installer must not be re-run for the next package"
        );
        let stderr = stderr(&output);
        assert_eq!(
            stderr.matches("exit status: 3").count(),
            2,
            "both packages must report the installer's real exit status, got: {stderr}"
        );
        assert!(
            stderr.contains("failed to install: somawork, somawork-preview"),
            "both packages must fail, got: {stderr}"
        );
        assert_eq!(
            sandbox.calls(),
            Vec::<Vec<String>>::new(),
            "there is no brew to call after a failed bootstrap"
        );
        assert_eq!(sandbox.record("somawork"), None);
        assert_eq!(sandbox.record("somawork-preview"), None);
    }

    /// Same once-per-invocation rule for the other failure shape: the installer
    /// exits 0 but nothing usable lands, on PATH or in the prefix. Retrying it
    /// for the next package would just curl|bash Homebrew a second time to
    /// reach the same conclusion.
    #[test]
    fn a_bootstrap_that_lands_no_usable_brew_runs_once_for_the_whole_invocation() {
        let sandbox = Sandbox::new("bootstrap-once-silent");
        sandbox.brew_bootstrap(Bootstrap::LeavesNothingOnPath);

        let output = sandbox.xbrew_bootstrapping(&["install", "somawork", "somawork-preview"]);
        assert!(!output.status.success(), "install should have failed");
        assert_eq!(
            sandbox.bootstrap_runs(),
            1,
            "the installer must not be re-run for the next package"
        );
        let stderr = stderr(&output);
        assert!(
            stderr.contains("PATH"),
            "the error must point at PATH, got: {stderr}"
        );
        assert!(
            stderr.contains("failed to install: somawork, somawork-preview"),
            "both packages must fail, got: {stderr}"
        );
        assert_eq!(sandbox.calls(), Vec::<Vec<String>>::new());
        assert_eq!(sandbox.record("somawork"), None);
        assert_eq!(sandbox.record("somawork-preview"), None);
    }

    /// A cask token is data on exactly the same terms as a formula token, and
    /// the bootstrap is a shell installer: a token that is not a Homebrew
    /// reference must be refused *before* it can buy itself a whole new process
    /// tree. The cask branch used to bootstrap first and look at the token
    /// afterwards.
    #[test]
    fn a_hostile_cask_token_is_refused_before_homebrew_is_bootstrapped() {
        let sandbox = Sandbox::new("bootstrap-cask-invalid");
        sandbox.brew_bootstrap(Bootstrap::Succeeds);
        let canary = sandbox.canary();
        let token = format!("app;touch {}", canary.display());
        sandbox.user_recipe(
            "hostile-cask",
            &format!("[macos]\ncask = \"{}\"\n", toml_escape(&token)),
        );

        let output = sandbox.xbrew_bootstrapping(&["install", "hostile-cask"]);
        assert!(!output.status.success(), "install should have failed");
        assert_eq!(
            sandbox.bootstrap_runs(),
            0,
            "an unusable cask token must not buy itself a Homebrew install"
        );
        assert_eq!(sandbox.calls(), Vec::<Vec<String>>::new());
        assert!(
            !canary.exists(),
            "the cask token executed its payload — the canary was created"
        );
        assert_eq!(sandbox.record("hostile-cask"), None);
    }

    /// The implicit bootstrap resolves Homebrew for *xbrew's* purposes, so it
    /// reads the embedded recipe and never `~/.xbrew/recipes/brew.toml`.
    /// Otherwise asking for an unrelated formula would execute whatever
    /// anything able to write the user's home had left in that file — a recipe
    /// override is a choice about a package you asked for, not a licence to run
    /// during someone else's install. (`xbrew install brew` keeps normal
    /// override semantics; this is about who did the asking.)
    ///
    /// Which recipe ran is observed, not asserted: the sandbox `curl` is the
    /// only curl on PATH, and its payload can reach bash through exactly one
    /// route — `recipes/brew.toml`'s own `/bin/bash -c "$(curl …)"`. So a run
    /// recorded in the bootstrap log *is* the embedded recipe having been
    /// selected, and the untouched canary is the override having been ignored.
    #[test]
    fn the_implicit_bootstrap_ignores_a_user_brew_recipe_override() {
        let sandbox = Sandbox::new("bootstrap-override");
        sandbox.brew_bootstrap(Bootstrap::Succeeds);
        let canary = sandbox.canary();
        sandbox.user_recipe(
            "brew",
            &format!(
                "[script]\nprovides_bin = \"brew\"\ninstall = \"touch {}\"\n",
                canary.display()
            ),
        );

        let output = sandbox.xbrew_bootstrapping(&["install", "somawork"]);
        assert!(
            output.status.success(),
            "install failed: {}",
            stderr(&output)
        );
        assert!(
            !canary.exists(),
            "an implicit bootstrap executed the user's ~/.xbrew/recipes/brew.toml"
        );
        assert_eq!(
            sandbox.bootstrap_runs(),
            1,
            "the embedded brew recipe's installer is what must have run"
        );
        assert_calls(
            &sandbox,
            &[
                &["brew", "list", "--formula", "--versions", STABLE_TOKEN],
                &["brew", "trust", "--tap", "2lab-ai/tap"],
                &["brew", "install", STABLE_TOKEN],
            ],
        );
        assert_eq!(sandbox.record("brew"), None);
    }

    /// A manifest's `trust:` list needs brew before any entry installs, and its
    /// brew-backed entries need brew too — but that is **one** Homebrew
    /// dependency, not two. The trust gate used to acquire brew through the
    /// *explicit* `install("brew")` route, which leaves the shared bootstrap
    /// context untouched, so the first formula entry would curl|bash Homebrew a
    /// second time in the same invocation.
    #[test]
    fn a_bundle_bootstraps_homebrew_at_most_once_for_the_whole_manifest() {
        let sandbox = Sandbox::new("bundle-once");
        sandbox.brew_bootstrap(Bootstrap::LeavesNothingOnPath);
        let manifest = sandbox.manifest("trust:\n  - 2lab-ai/tap\nxbrew:\n  - somawork\n");

        let output = sandbox.xbrew_bootstrapping(&["bundle", &manifest.to_string_lossy()]);
        assert!(!output.status.success(), "bundle should have failed");
        assert_eq!(
            sandbox.bootstrap_runs(),
            1,
            "the trust list and the first brew-backed entry are one Homebrew \
             dependency — the installer must not run again for the second"
        );
        // Acquired to satisfy the manifest, not because the user asked for it,
        // so it is not recorded — the same rule the formula/cask bootstrap
        // follows.
        assert_eq!(sandbox.record("brew"), None);
        assert_eq!(sandbox.record("somawork"), None);
    }

    /// And the bundle gate reads the embedded recipe for the same reason the
    /// formula/cask gate does: nobody asked for `brew` here, so
    /// `~/.xbrew/recipes/brew.toml` does not get to choose what runs.
    #[test]
    fn a_bundle_bootstrap_ignores_a_user_brew_recipe_override() {
        let sandbox = Sandbox::new("bundle-override");
        sandbox.brew_bootstrap(Bootstrap::Succeeds);
        let canary = sandbox.canary();
        sandbox.user_recipe(
            "brew",
            &format!(
                "[script]\nprovides_bin = \"brew\"\ninstall = \"touch {}\"\n",
                canary.display()
            ),
        );
        let manifest = sandbox.manifest("trust:\n  - 2lab-ai/tap\nxbrew:\n  - somawork\n");

        let output = sandbox.xbrew_bootstrapping(&["bundle", &manifest.to_string_lossy()]);
        assert!(
            output.status.success(),
            "bundle failed: {}",
            stderr(&output)
        );
        assert!(
            !canary.exists(),
            "the bundle gate executed the user's ~/.xbrew/recipes/brew.toml"
        );
        assert_eq!(sandbox.bootstrap_runs(), 1);
        assert_eq!(sandbox.record("brew"), None);
        // Homebrew was reachable *before* the taps were registered, which is the
        // point of acquiring it at the gate: the first two calls are the ones
        // that used to be lost to a "failed to launch `brew`" warning.
        assert_calls(
            &sandbox,
            &[
                &["brew", "tap", "2lab-ai/tap"],
                &["brew", "trust", "--tap", "2lab-ai/tap"],
                &["brew", "list", "--formula", "--versions", STABLE_TOKEN],
                &["brew", "trust", "--tap", "2lab-ai/tap"],
                &["brew", "install", STABLE_TOKEN],
                &["brew", "list", "--versions", STABLE_TOKEN],
            ],
        );
        assert_eq!(
            sandbox.record("somawork"),
            Some(formula_record(STABLE_TOKEN))
        );
    }

    /// Explicit `xbrew install brew` on a machine that already has an off-PATH
    /// Homebrew: the adoption probe is a plain `command -v brew`, so it used to
    /// miss and re-run Homebrew's installer over a working install. Adoption is
    /// the whole point of that probe, and it has to see brew wherever brew
    /// actually is.
    #[test]
    fn an_explicit_brew_install_adopts_an_off_path_homebrew() {
        let sandbox = Sandbox::new("explicit-adopt");
        sandbox.brew_bootstrap(Bootstrap::Succeeds);
        sandbox.preinstall_brew_in_prefix();

        let output = sandbox.xbrew_bootstrapping(&["install", "brew"]);
        assert!(
            output.status.success(),
            "install failed: {}",
            stderr(&output)
        );
        assert_eq!(
            sandbox.bootstrap_runs(),
            0,
            "Homebrew was already installed — its installer must not run"
        );
        assert!(
            stdout(&output).contains("already present"),
            "brew should have been adopted, got: {}",
            stdout(&output)
        );
        assert_eq!(
            sandbox.record("brew"),
            Some(serde_json::json!({ "backend": "script", "reference": "brew" }))
        );
    }

    /// The no-recipe fallback's refusal is only correct when brew is genuinely
    /// absent. With an off-PATH Homebrew present it was telling the user to run
    /// `xbrew install brew` on a machine that already had it — the same
    /// PATH-blindness, one branch over.
    ///
    /// The known gap is deliberately left intact: this name still goes to brew
    /// straight from argv, unvalidated and untrusted.
    #[test]
    fn a_no_recipe_name_uses_an_off_path_homebrew_instead_of_refusing() {
        let sandbox = Sandbox::new("no-recipe-prefix");
        sandbox.brew_bootstrap(Bootstrap::Succeeds);
        sandbox.preinstall_brew_in_prefix();

        let output = sandbox.xbrew_bootstrapping(&["install", "nothing-declares-this"]);
        assert!(
            output.status.success(),
            "install failed: {}",
            stderr(&output)
        );
        assert_eq!(
            sandbox.bootstrap_runs(),
            0,
            "nothing declared Homebrew as this name's backend, and brew was \
             already there anyway"
        );
        assert_calls(
            &sandbox,
            &[
                &["brew", "info", "--formula", "nothing-declares-this"],
                &[
                    "brew",
                    "list",
                    "--formula",
                    "--versions",
                    "nothing-declares-this",
                ],
                &["brew", "install", "nothing-declares-this"],
            ],
        );
        assert_eq!(
            sandbox.record("nothing-declares-this"),
            Some(serde_json::json!({
                "backend": "brew",
                "reference": "nothing-declares-this",
                "kind": "formula",
            }))
        );
    }

    /// The other half of the override rule — and what keeps it a *rule* rather
    /// than a removal. `xbrew install brew` is the user naming both the package
    /// and the moment, so `~/.xbrew/recipes/brew.toml` still wins there; only
    /// the implicit bootstrap is pinned to the built-in. Without this, "the
    /// bootstrap ignores the override" could just as well have been implemented
    /// by dropping override support altogether.
    ///
    /// The sandbox `curl` is deliberately left unarmed, so this fails closed: if
    /// the explicit path ever started resolving through the *embedded* recipe,
    /// its `/bin/bash -c "$(curl …)"` would meet a curl that refuses rather than
    /// quietly installing something.
    ///
    /// Scope, deliberately narrow: explicit install goes through the ordinary
    /// script backend, which records on the installer's exit 0 and performs no
    /// post-install discovery. So this pins the user installer having run and
    /// the resulting record, and claims nothing about the bootstrap's PATH
    /// re-probe — that behaviour belongs to the tests above.
    #[test]
    fn an_explicit_brew_install_still_honours_a_user_recipe_override() {
        let sandbox = Sandbox::new("explicit-brew");
        let receipt = sandbox.user_installer_receipt();
        let installer = sandbox.root.join("user-brew-installer.sh");
        fs::write(
            &installer,
            format!(
                "printf 'ran\\n' >>'{}'\ncp '{}' '{}'\nexit 0\n",
                receipt.display(),
                sandbox.root.join("bin/brew").display(),
                sandbox.bootstrap_bin().join("brew").display()
            ),
        )
        .expect("write the user's brew installer");
        sandbox.user_recipe(
            "brew",
            &format!(
                "[script]\nprovides_bin = \"brew\"\ninstall = \"sh {}\"\n",
                installer.display()
            ),
        );

        let output = sandbox.xbrew_bootstrapping(&["install", "brew"]);
        assert!(
            output.status.success(),
            "install failed: {}",
            stderr(&output)
        );
        assert!(
            receipt.exists(),
            "the user's own ~/.xbrew/recipes/brew.toml installer never ran"
        );
        // brew is the package that was asked for, so — unlike the implicit
        // bootstrap, whose record is dropped — it is recorded. That is how
        // xbrew comes to own, and can later uninstall, Homebrew itself.
        assert_eq!(
            sandbox.record("brew"),
            Some(serde_json::json!({ "backend": "script", "reference": "brew" }))
        );
    }

    /// A name no recipe declares never named Homebrew as its backend, so a
    /// missing brew stays a refusal rather than becoming a dependency:
    /// installing a package manager to look up a name that may not exist is not
    /// what was asked. The bootstrap is armed here so that "it did not run"
    /// means something.
    #[test]
    fn a_name_with_no_recipe_refuses_instead_of_bootstrapping_homebrew() {
        let sandbox = Sandbox::new("bootstrap-no-recipe");
        sandbox.brew_bootstrap(Bootstrap::Succeeds);

        let output = sandbox.xbrew_bootstrapping(&["install", "nothing-declares-this"]);
        assert!(!output.status.success(), "install should have failed");
        assert!(
            stderr(&output).contains("xbrew install brew"),
            "the refusal must still name the explicit route, got: {}",
            stderr(&output)
        );
        assert_eq!(
            sandbox.bootstrap_runs(),
            0,
            "nothing declared Homebrew as this name's backend"
        );
        assert_eq!(sandbox.calls(), Vec::<Vec<String>>::new());
        assert_eq!(sandbox.record("nothing-declares-this"), None);
    }

    /// Validation stays in front of the bootstrap. A token that is not a
    /// Homebrew reference must not be able to trigger a Homebrew install — the
    /// installer is a shell script, and a token that got that far would have
    /// bought itself a whole new process tree to be mishandled by.
    #[test]
    fn an_invalid_formula_token_is_refused_before_homebrew_is_bootstrapped() {
        let sandbox = Sandbox::new("bootstrap-invalid");
        sandbox.brew_bootstrap(Bootstrap::Succeeds);
        let canary = sandbox.canary();
        let token = format!("somawork;touch {}", canary.display());
        sandbox.user_recipe(
            "hostile-no-brew",
            &format!("[macos]\nformula = \"{}\"\n", toml_escape(&token)),
        );

        let output = sandbox.xbrew_bootstrapping(&["install", "hostile-no-brew"]);
        assert!(!output.status.success(), "install should have failed");
        assert_eq!(
            sandbox.bootstrap_runs(),
            0,
            "an unusable token must not buy itself a Homebrew install"
        );
        assert_eq!(sandbox.calls(), Vec::<Vec<String>>::new());
        assert!(
            !canary.exists(),
            "the token executed its payload — the canary was created"
        );
        assert_eq!(sandbox.record("hostile-no-brew"), None);
    }

    /// Tokens that are not Homebrew references must be refused as *data*,
    /// before brew is ever handed one. These recipes declare a `macos.formula`
    /// and nothing else, so no probe runs ahead of the check either: what this
    /// pins is that brew is never invoked — the transcript stays empty — and
    /// that the payload never executes.
    #[test]
    fn hostile_formula_tokens_are_refused_before_brew_is_spawned() {
        // `{canary}` is substituted with a path inside that iteration's own
        // sandbox, so a regression writes somewhere we then read — and never
        // into a shared /tmp name.
        let hostile = [
            "",
            "   ",
            "somawork --build-from-source",
            "--formula",
            "-somawork",
            "somawork;touch {canary}",
            "somawork|touch {canary}",
            "$(touch {canary})",
            "`touch {canary}`",
            "somawork\ntouch {canary}",
            "../../../etc/passwd",
            "2lab-ai/tap/../../../etc/passwd",
            "/absolute/tap/somawork",
            "2lab-ai/tap",
            "2lab-ai/tap/somawork/extra",
            "2lab-ai/tap/.",
            "2lab-ai//somawork",
        ];

        for (index, template) in hostile.iter().enumerate() {
            let sandbox = Sandbox::new(&format!("hostile-{index}"));
            let canary = sandbox.canary();
            let token = template.replace("{canary}", &canary.to_string_lossy());
            let name = format!("hostile-{index}");
            sandbox.user_recipe(
                &name,
                &format!("[macos]\nformula = \"{}\"\n", toml_escape(&token)),
            );

            let output = sandbox.xbrew(&["install", &name]);
            assert!(
                !output.status.success(),
                "token {token:?} must be rejected, but install succeeded"
            );
            assert_eq!(
                sandbox.calls(),
                Vec::<Vec<String>>::new(),
                "token {token:?} reached brew"
            );
            assert_eq!(sandbox.record(&name), None, "token {token:?} was recorded");
            assert!(
                !canary.exists(),
                "token {token:?} executed its payload — the canary was created"
            );
        }
    }

    fn toml_escape(value: &str) -> String {
        let mut escaped = String::new();
        for character in value.chars() {
            match character {
                '"' => escaped.push_str("\\\""),
                '\\' => escaped.push_str("\\\\"),
                '\n' => escaped.push_str("\\n"),
                '\t' => escaped.push_str("\\t"),
                other => escaped.push(other),
            }
        }
        escaped
    }

    /// `xbrew search` on a Mac whose Homebrew is off-PATH.
    ///
    /// The brew section is most of what makes `search` worth running, and
    /// gating it on a plain `command -v brew` dropped it silently — no error,
    /// no "brew not found", just a shorter answer — on exactly the machines the
    /// rest of the off-PATH handling exists for. Silent omission is the failure
    /// mode to test for, so this asserts both the section header and the call
    /// that produced it.
    ///
    /// The bootstrap is armed and must stay unused: `search` reads, it does not
    /// acquire. Discovery is allowed to find Homebrew; it is not allowed to
    /// install one behind a read-only command.
    #[test]
    fn search_uses_an_off_path_homebrew_instead_of_skipping_the_brew_section() {
        let sandbox = Sandbox::new("search-prefix");
        sandbox.brew_bootstrap(Bootstrap::Succeeds);
        sandbox.preinstall_brew_in_prefix();

        let output = sandbox.xbrew_bootstrapping(&["search", "somawork"]);
        assert!(
            output.status.success(),
            "search failed: {}",
            stderr(&output)
        );
        let text = stdout(&output);
        assert!(
            text.contains("== brew =="),
            "the brew section was silently omitted: {text}"
        );
        assert_calls(&sandbox, &[&["brew", "search", "somawork"]]);
        assert_eq!(
            sandbox.bootstrap_runs(),
            0,
            "search must not install Homebrew"
        );
    }

    #[test]
    fn info_reports_both_channels_as_formula_recipes() {
        let sandbox = Sandbox::new("info");
        for (name, token) in [
            ("somawork", STABLE_TOKEN),
            ("somawork-preview", PREVIEW_TOKEN),
        ] {
            let output = sandbox.xbrew(&["info", name]);
            assert!(output.status.success(), "info failed: {}", stderr(&output));
            let text = stdout(&output);
            assert!(text.contains("recipe:  yes"), "{name}: {text}");
            assert!(
                text.contains(&format!("macos → formula: {token}")),
                "{name}: {text}"
            );
        }
    }
}
