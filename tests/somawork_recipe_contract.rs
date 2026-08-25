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

case "$subcommand" in
  list)
    if [ -n "$token" ] && [ -f "$state/$key" ]; then
      printf '%s %s\n' "$token" "$(cat "$state/$key")"
    fi
    ;;
  install)
    printf '%s' "${XBREW_TEST_BREW_VERSION:-1.0.0}" >"$state/$key"
    ;;
  upgrade)
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
            Sandbox { root }
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

        // Exactly two calls: the adoption probe, then the install. No
        // `brew info --formula somawork` — the recipe outranks native lookup.
        assert_calls(
            &sandbox,
            &[
                &["brew", "list", "--formula", "--versions", STABLE_TOKEN],
                &["brew", "install", STABLE_TOKEN],
            ],
        );
        assert!(sandbox.is_installed(STABLE_TOKEN));
        assert_eq!(
            sandbox.record("somawork"),
            Some(formula_record(STABLE_TOKEN))
        );
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
        assert_calls(
            &sandbox,
            &[&["brew", "list", "--formula", "--versions", STABLE_TOKEN]],
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
        assert_calls(
            &sandbox,
            &[
                &["brew", "list", "--versions", STABLE_TOKEN],
                &["brew", "info", "--json=v2", "--formula", STABLE_TOKEN],
                &["brew", "upgrade", STABLE_TOKEN],
            ],
        );
        assert!(sandbox.xbrew(&["version", "somawork"]).status.success());
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
