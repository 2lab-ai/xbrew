use serde::Deserialize;
use std::collections::BTreeMap;

use crate::util;

/// An "arch-cask": how to install something that isn't a plain brew/pacman package.
#[derive(Deserialize, Clone, Debug, Default)]
pub struct Recipe {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub arch: ArchSpec,
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
    /// AUR package name -> git clone + makepkg -si
    pub aur: Option<String>,
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

/// Recipes compiled into the binary. User recipes in ~/.nobrew/recipes/*.toml override these.
const BUILTINS: &[&str] = &[
    include_str!("../recipes/brew.toml"),
    include_str!("../recipes/claude.toml"),
    include_str!("../recipes/nomachine.toml"),
    include_str!("../recipes/rustdesk.toml"),
    include_str!("../recipes/slack.toml"),
    include_str!("../recipes/sunshine.toml"),
];

pub fn registry() -> BTreeMap<String, Recipe> {
    let mut map = BTreeMap::new();
    for src in BUILTINS {
        if let Ok(r) = toml::from_str::<Recipe>(src) {
            map.insert(r.name.clone(), r);
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
    Ok(util::nobrew_dir()?.join("recipes"))
}

pub fn get(name: &str) -> Option<Recipe> {
    registry().remove(name)
}
