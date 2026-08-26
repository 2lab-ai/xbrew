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
            for directory in ["home/.xbrew/recipes", "bin", "brew-state"] {
                fs::create_dir_all(root.join(directory)).expect("create sandbox directory");
            }
            let brew = root.join("bin/brew");
            fs::write(&brew, FAKE_BREW).expect("write fake brew");
            fs::set_permissions(&brew, fs::Permissions::from_mode(0o755))
                .expect("make fake brew executable");
            Sandbox {
                root,
                brew_env: Vec::new(),
            }
        }

        /// Point the fake at a different Homebrew generation (see `FAKE_BREW`).
        fn brew_env(&mut self, key: &str, value: &str) {
            self.brew_env.push((key.to_string(), value.to_string()));
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

        /// Run the real binary with no brew anywhere on PATH.
        fn xbrew_without_brew(&self, arguments: &[&str]) -> Output {
            self.spawn(arguments, "", "/usr/bin:/bin")
        }

        fn spawn(&self, arguments: &[&str], answers: &str, path: &str) -> Output {
            let mut child = Command::new(env!("CARGO_BIN_EXE_xbrew"))
                .args(arguments)
                .env_clear()
                .env("HOME", self.home())
                .env("PATH", path)
                .env("XBREW_TEST_BREW_LOG", self.log())
                .env("XBREW_TEST_BREW_STATE", self.root.join("brew-state"))
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

    #[test]
    fn a_formula_recipe_refuses_to_proceed_without_homebrew() {
        let sandbox = Sandbox::new("no-brew");
        let output = sandbox.xbrew_without_brew(&["install", "somawork"]);
        assert!(!output.status.success(), "install should have failed");
        assert!(
            stderr(&output).contains("installs as a Homebrew formula"),
            "expected the formula backend to name its Homebrew requirement, got: {}",
            stderr(&output)
        );
        assert_eq!(sandbox.record("somawork"), None);
    }

    /// Tokens that are not Homebrew references must be refused as *data*,
    /// before brew is ever handed one. (An `sh -c "command -v brew"` probe has
    /// already run by then, but it is a fixed literal that never sees the
    /// token; what this pins is that brew itself is never invoked — the
    /// transcript stays empty — and that the payload never executes.)
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
