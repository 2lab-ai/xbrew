use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::util;

/// One installed package, remembering which backend put it there so
/// `xbrew uninstall` can route to the right remover.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Record {
    /// backend: brew | pacman | aur | flatpak | recipe-dmg
    pub backend: String,
    /// what to hand back to that backend on uninstall
    /// (brew formula/cask, pacman/aur pkg name, flatpak app id, or .app path)
    pub reference: String,
    /// brew kind: "formula" or "cask"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// filesystem artifacts to remove (recipe-dmg installs)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug, Default)]
pub struct State {
    #[serde(default)]
    pub packages: BTreeMap<String, Record>,
}

impl State {
    pub fn path() -> Result<PathBuf> {
        Ok(util::xbrew_dir()?.join("state.json"))
    }

    pub fn load() -> Result<State> {
        let p = Self::path()?;
        if !p.exists() {
            return Ok(State::default());
        }
        let data =
            std::fs::read_to_string(&p).with_context(|| format!("reading {}", p.display()))?;
        let state =
            serde_json::from_str(&data).with_context(|| format!("parsing {}", p.display()))?;
        Ok(state)
    }

    pub fn save(&self) -> Result<()> {
        let p = Self::path()?;
        let data = serde_json::to_string_pretty(self)?;
        std::fs::write(&p, data).with_context(|| format!("writing {}", p.display()))?;
        Ok(())
    }
}
