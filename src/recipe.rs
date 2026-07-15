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
    /// Extra official-repo packages to install first (e.g. a build tool an AUR
    /// PKGBUILD uses but forgets to declare as makedepends).
    #[serde(default)]
    pub deps: Vec<String>,
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
    /// Direct .dmg URL (fallback when not in brew)
    pub dmg: Option<String>,
    /// App bundle name inside the dmg, e.g. "Sunshine.app" (needed to uninstall)
    pub app: Option<String>,
}

/// Every `recipes/*.toml` (embedded) plus user recipes in
/// `~/.xbrew/recipes/*.toml`, which override built-ins by name.
pub fn registry() -> BTreeMap<String, Recipe> {
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
                || r.debian.apt.is_some()
                || r.debian.flatpak.is_some()
                || r.rhel.dnf.is_some()
                || r.rhel.flatpak.is_some()
                || r.macos.cask.is_some()
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
    fn llmux_islands_builds_from_the_upstream_repo() {
        let r = get("llmux-islands").expect("llmux-islands recipe missing");
        assert_eq!(r.arch.pkg.as_deref(), Some("llmux-islands-git"));
        assert_eq!(
            r.arch.dir.as_deref(),
            Some("llmux-islands-linux/packaging/arch")
        );
        assert_eq!(r.macos.cask.as_deref(), Some("2lab-ai/tap/llmux-islands"));
    }
}
