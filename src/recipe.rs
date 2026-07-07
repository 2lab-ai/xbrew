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
    /// Flathub app id -> flatpak install
    pub flatpak: Option<String>,
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
