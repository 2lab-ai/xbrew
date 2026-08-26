use include_dir::{include_dir, Dir};
use serde::Deserialize;
use std::collections::BTreeMap;

use crate::util;

/// The whole recipes/ directory, embedded at compile time. Drop a new
/// `<name>.toml` in there and it ships automatically — no source edit.
static BUILTIN_RECIPES: Dir = include_dir!("$CARGO_MANIFEST_DIR/recipes");

/// An "arch-cask": how to install something that isn't a plain brew/pacman package.
#[derive(Deserialize, Clone, Debug, Default)]
pub struct Recipe {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub arch: ArchSpec,
    #[serde(default)]
    pub debian: DebianSpec,
    #[serde(default)]
    pub rhel: RhelSpec,
    #[serde(default)]
    pub macos: MacSpec,
    /// Self-installing tools (brew, claude) that ship their own curl|bash
    /// installer — platform-independent, tried as a last resort.
    #[serde(default)]
    pub script: ScriptSpec,
}

#[derive(Deserialize, Clone, Debug, Default)]
pub struct ScriptSpec {
    /// shell command that installs the tool (run via `sh -c`)
    pub install: Option<String>,
    /// shell command that removes it (optional)
    pub uninstall: Option<String>,
    /// if this binary is already on PATH, treat as already installed
    pub provides_bin: Option<String>,
}

#[derive(Deserialize, Clone, Debug, Default)]
pub struct ArchSpec {
    /// Official repo package name (maps a friendly name -> the real pkg), via pacman
    pub pacman: Option<String>,
    /// AUR package name -> git clone + makepkg -si
    pub aur: Option<String>,
    /// Git repo that carries its own PKGBUILD in-tree, for upstreams that ship
    /// an Arch package but never published it to the AUR -> git clone + makepkg
    /// -si, same as `aur` but from an arbitrary remote. Needs `dir` and `pkg`.
    pub pkgbuild: Option<String>,
    /// Path to the directory holding the PKGBUILD, relative to the repo root.
    pub dir: Option<String>,
    /// The `pkgname` the PKGBUILD produces. makepkg registers it in the pacman
    /// DB under this name, so it is what uninstall/version query.
    pub pkg: Option<String>,
    /// Flathub app id -> flatpak install
    pub flatpak: Option<String>,
    /// A prebuilt executable downloaded by xbrew itself and pinned by SHA-256.
    /// This is the Arch equivalent of a cask for upstreams that publish Linux
    /// binaries but do not maintain a pacman package or AUR recipe.
    pub binary: Option<BinarySpec>,
    /// Extra official-repo packages to install first (e.g. a build tool an AUR
    /// PKGBUILD uses but forgets to declare as makedepends).
    #[serde(default)]
    pub deps: Vec<String>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct BinarySpec {
    /// Executable name under ~/.xbrew/bin.
    pub name: String,
    /// Immutable artifact for each xbrew release architecture.
    pub aarch64: BinaryArtifact,
    pub x86_64: BinaryArtifact,
}

#[derive(Deserialize, Clone, Debug)]
pub struct BinaryArtifact {
    pub url: String,
    pub sha256: String,
}

#[derive(Deserialize, Clone, Debug, Default)]
pub struct DebianSpec {
    /// apt package name (Debian/Ubuntu)
    pub apt: Option<String>,
    /// Flathub app id -> flatpak install
    pub flatpak: Option<String>,
}

#[derive(Deserialize, Clone, Debug, Default)]
pub struct RhelSpec {
    /// dnf/yum package name (Amazon Linux/Fedora/RHEL)
    pub dnf: Option<String>,
    /// Flathub app id -> flatpak install
    pub flatpak: Option<String>,
}

#[derive(Deserialize, Clone, Debug, Default)]
pub struct MacSpec {
    /// Homebrew cask name
    pub cask: Option<String>,
    /// Homebrew formula name, for tools that ship as a formula rather than an
    /// app bundle. Either a plain name or a fully-qualified `user/tap/formula`
    /// token, which auto-taps on install. It is data, not a shell word: see
    /// `resolve::validate_formula_token`, which vets it before brew is ever
    /// handed the token.
    pub formula: Option<String>,
    /// Direct .dmg URL (fallback when not in brew)
    pub dmg: Option<String>,
    /// App bundle name inside the dmg, e.g. "Sunshine.app" (needed to uninstall)
    pub app: Option<String>,
}

/// Only the recipes xbrew ships — `~/.xbrew/recipes` deliberately not
/// consulted. The reviewable set, and the one xbrew resolves against when it is
/// acting on its own behalf rather than on a name the user typed.
fn builtin_registry() -> BTreeMap<String, Recipe> {
    let mut map = BTreeMap::new();
    for file in BUILTIN_RECIPES.files() {
        if file.path().extension().and_then(|s| s.to_str()) != Some("toml") {
            continue;
        }
        if let Some(txt) = file.contents_utf8() {
            if let Ok(r) = toml::from_str::<Recipe>(txt) {
                map.insert(r.name.clone(), r);
            }
        }
    }
    map
}

/// Every `recipes/*.toml` (embedded) plus user recipes in
/// `~/.xbrew/recipes/*.toml`, which override built-ins by name.
pub fn registry() -> BTreeMap<String, Recipe> {
    let mut map = builtin_registry();
    if let Ok(dir) = user_recipe_dir() {
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for e in entries.flatten() {
                let p = e.path();
                if p.extension().and_then(|s| s.to_str()) == Some("toml") {
                    if let Ok(txt) = std::fs::read_to_string(&p) {
                        if let Ok(r) = toml::from_str::<Recipe>(&txt) {
                            map.insert(r.name.clone(), r);
                        }
                    }
                }
            }
        }
    }
    map
}

fn user_recipe_dir() -> anyhow::Result<std::path::PathBuf> {
    Ok(util::xbrew_dir()?.join("recipes"))
}

pub fn get(name: &str) -> Option<Recipe> {
    registry().remove(name)
}

/// A **built-in** recipe by name, ignoring `~/.xbrew/recipes` entirely.
///
/// `get()` is the lookup for a package the *user* named, where an override is a
/// feature. This one is for the recipes xbrew runs on its own initiative — the
/// implicit Homebrew bootstrap — where it would not be: asking for an unrelated
/// formula would otherwise execute whatever anything able to write the user's
/// home had left in `~/.xbrew/recipes/brew.toml`. `xbrew install brew` still
/// goes through `get()` and still honours an override, because there the user
/// chose both the recipe and the moment.
pub fn builtin(name: &str) -> Option<Recipe> {
    builtin_registry().remove(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every embedded recipe, parsed strictly. `registry()` drops a recipe that
    /// fails to parse, so without this a typo would ship as a silently missing
    /// package instead of a build error.
    fn builtins() -> Vec<(String, Recipe)> {
        BUILTIN_RECIPES
            .files()
            .filter(|f| f.path().extension().and_then(|s| s.to_str()) == Some("toml"))
            .map(|f| {
                let path = f.path().display().to_string();
                let txt = f
                    .contents_utf8()
                    .unwrap_or_else(|| panic!("{path}: not utf-8"));
                let r: Recipe =
                    toml::from_str(txt).unwrap_or_else(|e| panic!("{path}: does not parse: {e}"));
                (path, r)
            })
            .collect()
    }

    /// A single built-in by name. Deliberately **not** `get()`/`registry()`:
    /// those merge `~/.xbrew/recipes/*.toml`, so a developer's local override
    /// would silently become the thing under test and a shipped-recipe contract
    /// would pass or fail on machine state. Runtime override behaviour is
    /// covered separately, against an isolated HOME, in
    /// `tests/somawork_recipe_contract.rs`.
    fn builtin(name: &str) -> Recipe {
        builtins()
            .into_iter()
            .map(|(_, r)| r)
            .find(|r| r.name == name)
            .unwrap_or_else(|| panic!("no built-in recipe named '{name}'"))
    }

    #[test]
    fn builtin_recipes_parse_and_are_named() {
        let all = builtins();
        assert!(!all.is_empty(), "no recipes embedded");
        for (path, r) in &all {
            assert!(!r.name.is_empty(), "{path}: empty name");
            // The file name is the lookup key users type; a mismatch with the
            // `name` field would make the recipe unreachable.
            let stem = path.trim_end_matches(".toml");
            assert_eq!(&r.name, stem, "{path}: name does not match file name");
        }
    }

    #[test]
    fn builtin_recipes_declare_a_backend() {
        for (path, r) in builtins() {
            let has_backend = r.arch.pacman.is_some()
                || r.arch.aur.is_some()
                || r.arch.pkgbuild.is_some()
                || r.arch.flatpak.is_some()
                || r.arch.binary.is_some()
                || r.debian.apt.is_some()
                || r.debian.flatpak.is_some()
                || r.rhel.dnf.is_some()
                || r.rhel.flatpak.is_some()
                || r.macos.cask.is_some()
                || r.macos.formula.is_some()
                || r.macos.dmg.is_some()
                || r.script.install.is_some();
            assert!(has_backend, "{path}: no backend on any platform");
        }
    }

    /// `pkgbuild` alone can't be acted on: `dir` says what to build and `pkg`
    /// is the pacman name uninstall/version resolve against. Unknown TOML keys
    /// are ignored silently, so a misspelled `pkg` would only surface here.
    #[test]
    fn pkgbuild_recipes_carry_dir_and_pkg() {
        for (path, r) in builtins() {
            if r.arch.pkgbuild.is_some() {
                assert!(r.arch.dir.is_some(), "{path}: pkgbuild without `dir`");
                assert!(r.arch.pkg.is_some(), "{path}: pkgbuild without `pkg`");
            }
        }
    }

    #[test]
    fn binary_recipes_are_https_and_sha256_pinned() {
        for (path, r) in builtins() {
            let Some(binary) = r.arch.binary else {
                continue;
            };
            assert!(
                !binary.name.is_empty() && !binary.name.contains('/'),
                "{path}: invalid binary name"
            );
            for artifact in [&binary.aarch64, &binary.x86_64] {
                assert!(
                    artifact.url.starts_with("https://"),
                    "{path}: binary URL must use HTTPS"
                );
                assert_eq!(artifact.sha256.len(), 64, "{path}: invalid SHA-256 length");
                assert!(
                    artifact
                        .sha256
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
                    "{path}: SHA-256 must be lowercase hexadecimal"
                );
            }
        }
    }

    /// A formula token is passed straight to brew as one argv item, so a
    /// built-in recipe carrying one that isn't a Homebrew reference would be a
    /// shipped defect rather than a runtime surprise.
    #[test]
    fn builtin_formula_tokens_are_valid_homebrew_references() {
        for (path, r) in builtins() {
            if let Some(formula) = &r.macos.formula {
                crate::resolve::validate_formula_token(formula)
                    .unwrap_or_else(|e| panic!("{path}: {e:#}"));
            }
        }
    }

    /// Same for a cask token: it is one argv item handed to brew, so a
    /// built-in carrying something outside Homebrew's grammar would be a
    /// shipped defect — and now an install-time refusal rather than a
    /// mishandled string.
    #[test]
    fn builtin_cask_tokens_are_valid_homebrew_references() {
        for (path, r) in builtins() {
            if let Some(cask) = &r.macos.cask {
                crate::resolve::validate_cask_token(cask)
                    .unwrap_or_else(|e| panic!("{path}: {e:#}"));
            }
        }
    }

    /// Both SomaWork channels install from 2lab-ai's tap and nowhere else.
    /// macOS ARM64 is v1 scope: no Linux backend, no shell installer.
    #[test]
    fn somawork_channels_install_the_tapped_formulae() {
        for (name, token, channel) in [
            ("somawork", "2lab-ai/tap/somawork", "production runtime"),
            (
                "somawork-preview",
                "2lab-ai/tap/somawork-preview",
                "preview runtime",
            ),
        ] {
            let r = builtin(name);
            assert_eq!(r.macos.formula.as_deref(), Some(token));
            assert!(
                r.macos.cask.is_none() && r.macos.dmg.is_none(),
                "{name}: the formula is the only supported macOS backend"
            );
            assert!(
                r.script.install.is_none(),
                "{name}: SomaWork installs through Homebrew, never a shell installer"
            );
            assert!(
                r.arch.pacman.is_none()
                    && r.arch.aur.is_none()
                    && r.arch.pkgbuild.is_none()
                    && r.arch.flatpak.is_none()
                    && r.arch.binary.is_none()
                    && r.debian.apt.is_none()
                    && r.debian.flatpak.is_none()
                    && r.rhel.dnf.is_none()
                    && r.rhel.flatpak.is_none(),
                "{name}: macOS ARM64 is v1 scope — do not imply Linux support"
            );
            let description = r.description.to_lowercase();
            assert!(
                description.contains(channel) && description.contains("controller onboarding"),
                "{name}: description must name the {channel} and controller onboarding"
            );
        }
    }

    /// Compile and run `build.rs` exactly as cargo would, and return the
    /// directives it emits. Built fresh into a temp file rather than hunting
    /// for `target/*/build/xbrew-*/build-script-build`: several of those exist
    /// at once and the stale ones answer for an older build.rs.
    fn build_script_directives() -> String {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let binary = std::env::temp_dir().join(format!(
            "xbrew-build-script-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let compile = std::process::Command::new("rustc")
            .args(["--edition=2021", "-o"])
            .arg(&binary)
            .arg(manifest.join("build.rs"))
            .output()
            .expect("compile build.rs with rustc");
        assert!(
            compile.status.success(),
            "build.rs does not compile: {}",
            String::from_utf8_lossy(&compile.stderr)
        );
        let output = std::process::Command::new(&binary)
            .current_dir(manifest)
            .output()
            .expect("run the compiled build script");
        let _ = std::fs::remove_file(&binary);
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    /// `include_dir!` embeds only the files that exist at compile time and
    /// registers no dependency on the directory itself, so cargo learns about a
    /// *newly added* recipe only if build.rs asks it to watch `recipes/`.
    /// Emitting any `rerun-if-*` directive also disables cargo's
    /// rerun-on-any-package-change default, which is why the directory watch and
    /// the env watch have to coexist: drop the first and `cargo test` goes green
    /// against a binary that does not contain the new recipe.
    #[test]
    fn the_build_script_makes_cargo_watch_the_recipes_directory() {
        let stdout = build_script_directives();
        let emits = |directive: &str| stdout.lines().any(|line| line == directive);

        assert!(
            emits("cargo:rerun-if-changed=recipes"),
            "build.rs must make cargo watch recipes/, else a new recipe ships \
             unembedded behind a green test run. Emitted:\n{stdout}"
        );
        assert!(
            emits("cargo:rerun-if-env-changed=XBREW_BUILD_ID"),
            "build.rs must still watch XBREW_BUILD_ID. Emitted:\n{stdout}"
        );
        assert!(
            stdout
                .lines()
                .any(|line| line.starts_with("cargo:rustc-env=XBREW_BUILD_ID=")),
            "build.rs must still bake a build id. Emitted:\n{stdout}"
        );
    }

    #[test]
    fn llmux_islands_builds_from_the_upstream_repo() {
        let r = builtin("llmux-islands");
        assert_eq!(r.arch.pkg.as_deref(), Some("llmux-islands-git"));
        assert_eq!(
            r.arch.dir.as_deref(),
            Some("llmux-islands-linux/packaging/arch")
        );
        assert_eq!(r.macos.cask.as_deref(), Some("2lab-ai/tap/llmux-islands"));
    }
}
