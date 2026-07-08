//! Tiny YAML-subset parser for setup-os style software manifests.
//!
//! Deliberately dependency-free (xbrew stays lean) — the schema is a flat
//! `key:` / `- item` shape, which is all the manifests use:
//!
//! ```yaml
//! trust:
//!   - 2lab-ai/tap
//! xbrew:
//!   - brew
//!   - telegram >= 6.9.3   # inline comments ok
//! ```

use anyhow::{Context, Result};
use std::path::Path;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Manifest {
    /// Homebrew taps to register before installing (so brew-backed formulae resolve).
    pub trust: Vec<String>,
    /// Packages to install via `xbrew install`, with optional version constraints.
    pub entries: Vec<Entry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    /// Comparison operator when a version constraint is present: == = >= > <= <
    pub op: Option<String>,
    pub version: Option<String>,
}

pub fn parse_file(path: &Path) -> Result<Manifest> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading manifest {}", path.display()))?;
    Ok(parse_str(&text))
}

pub fn parse_str(text: &str) -> Manifest {
    let mut m = Manifest::default();
    let mut section = Section::None;

    for raw in text.lines() {
        let line = strip_comment(raw);
        if line.trim().is_empty() {
            continue;
        }
        // A top-level key sits at column 0 (no leading space, not a list item).
        if !line.starts_with([' ', '\t', '-']) {
            section = match line.split_once(':').map(|(k, _)| k.trim()) {
                Some("trust") => Section::Trust,
                Some("xbrew") => Section::Xbrew,
                _ => Section::None,
            };
            continue;
        }
        // List item under the current section.
        if let Some(item) = line.trim_start().strip_prefix('-') {
            let item = item.trim();
            if item.is_empty() {
                continue;
            }
            match section {
                Section::Trust => m.trust.push(unquote(item)),
                Section::Xbrew => m.entries.push(parse_entry(item)),
                Section::None => {}
            }
        }
    }
    m
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    None,
    Trust,
    Xbrew,
}

fn strip_comment(line: &str) -> String {
    match line.find('#') {
        Some(i) => line[..i].to_string(),
        None => line.to_string(),
    }
}

fn unquote(s: &str) -> String {
    s.trim().trim_matches(['"', '\'']).trim().to_string()
}

fn parse_entry(item: &str) -> Entry {
    let mut it = item.split_whitespace();
    let name = unquote(it.next().unwrap_or_default());
    let op = it.next();
    let version = it.next().map(unquote);
    match op {
        Some(op @ ("==" | "=" | ">=" | ">" | "<=" | "<")) => Entry {
            name,
            op: Some(op.to_string()),
            version,
        },
        _ => Entry {
            name,
            op: None,
            version: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_flat_schema() {
        let m = parse_str(
            "# header\ntrust:\n  - 2lab-ai/tap\nxbrew:\n  - brew\n  - telegram >= 6.9.3  # app\n",
        );
        assert_eq!(m.trust, vec!["2lab-ai/tap".to_string()]);
        assert_eq!(m.entries.len(), 2);
        assert_eq!(m.entries[0].name, "brew");
        assert_eq!(m.entries[0].op, None);
        assert_eq!(m.entries[1].name, "telegram");
        assert_eq!(m.entries[1].op.as_deref(), Some(">="));
        assert_eq!(m.entries[1].version.as_deref(), Some("6.9.3"));
    }

    #[test]
    fn ignores_unknown_sections_and_quotes() {
        let m = parse_str("other:\n  - nope\nxbrew:\n  - \"slack\" == 4.50.143\n");
        assert_eq!(m.trust.len(), 0);
        assert_eq!(m.entries.len(), 1);
        assert_eq!(m.entries[0].name, "slack");
        assert_eq!(m.entries[0].op.as_deref(), Some("=="));
        assert_eq!(m.entries[0].version.as_deref(), Some("4.50.143"));
    }
}
