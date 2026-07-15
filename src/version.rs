//! Installed-version resolution and constraint checking.
//!
//! xbrew records which backend installed each package, so it is the natural
//! owner of "what version is actually installed" — this used to live in the
//! setup-os shell as backend-specific probing; now it is here.

use std::cmp::Ordering;

use crate::recipe;
use crate::state::State;
use crate::util;

/// The installed upstream version of a tracked package, resolved via the
/// backend xbrew recorded. Returns `None` if it can't be determined.
pub fn installed(name: &str) -> Option<String> {
    let state = State::load().ok()?;
    let (backend, reference) = match state.packages.get(name) {
        Some(r) => (r.backend.clone(), r.reference.clone()),
        None => (String::new(), name.to_string()),
    };

    match backend.as_str() {
        // pkgbuild builds land in the pacman DB under their pkgname, same as AUR.
        "pacman" | "aur" | "pkgbuild" => {
            // "telegram-desktop 6.9.3-7.1" -> "6.9.3"
            let out = util::capture("pacman", &["-Q", &reference]);
            out.split_whitespace().nth(1).map(clean_pkg_version)
        }
        "brew" => {
            // "telegram-desktop 6.9.3" -> "6.9.3"
            let out = util::capture("brew", &["list", "--versions", &reference]);
            out.split_whitespace().last().map(|s| s.to_string())
        }
        "apt" => {
            let out = util::capture("dpkg-query", &["-W", "-f=${Version}", &reference]);
            let out = out.trim();
            (!out.is_empty()).then(|| clean_pkg_version(out))
        }
        "dnf" => {
            let out = util::capture("rpm", &["-q", "--qf", "%{VERSION}", &reference]);
            let out = out.trim();
            (!out.is_empty()).then(|| out.to_string())
        }
        "flatpak" => {
            let out = util::capture("env", &["LC_ALL=C", "flatpak", "info", &reference]);
            out.lines()
                .find(|l| l.to_ascii_lowercase().contains("version"))
                .and_then(extract)
        }
        _ => {
            // script backend / not tracked: ask the tool itself.
            let bin = recipe::get(name)
                .and_then(|r| r.script.provides_bin)
                .unwrap_or_else(|| name.to_string());
            extract(&util::capture(&bin, &["--version"]))
        }
    }
}

/// Does `current` satisfy `op required`? Unknown ops are treated as "no constraint".
pub fn satisfies(current: &str, op: &str, required: &str) -> bool {
    let ord = compare(current, required);
    match op {
        "==" | "=" => ord == Ordering::Equal,
        ">=" => ord != Ordering::Less,
        ">" => ord == Ordering::Greater,
        "<=" => ord != Ordering::Greater,
        "<" => ord == Ordering::Less,
        _ => true,
    }
}

/// Numeric, component-wise version compare (shorter side padded with zeros).
pub fn compare(a: &str, b: &str) -> Ordering {
    let split = |s: &str| {
        s.split(['.', '-'])
            .filter(|x| !x.is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>()
    };
    let av = split(a);
    let bv = split(b);
    for i in 0..av.len().max(bv.len()) {
        let x = av.get(i).map(String::as_str).unwrap_or("0");
        let y = bv.get(i).map(String::as_str).unwrap_or("0");
        let ord = match (x.parse::<u64>(), y.parse::<u64>()) {
            (Ok(nx), Ok(ny)) => nx.cmp(&ny),
            _ => x.cmp(y),
        };
        if ord != Ordering::Equal {
            return ord;
        }
    }
    Ordering::Equal
}

/// First dotted numeric version in arbitrary text, e.g. "2.1.204 (Claude Code)" -> "2.1.204".
pub fn extract(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if !bytes[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let start = i;
        let mut saw_dot = false;
        while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
            saw_dot |= bytes[i] == b'.';
            i += 1;
        }
        let candidate = text[start..i].trim_end_matches('.');
        if saw_dot && candidate.contains('.') {
            return Some(candidate.to_string());
        }
    }
    None
}

/// Strip a pacman/dpkg `epoch:` prefix and `-pkgrel` suffix: "1:6.9.3-7.1" -> "6.9.3".
fn clean_pkg_version(v: &str) -> String {
    let v = v.split_once(':').map(|(_, r)| r).unwrap_or(v);
    let v = v.rsplit_once('-').map(|(l, _)| l).unwrap_or(v);
    v.to_string()
}

/// Best-effort "latest available version" for a tracked package, per the backend
/// xbrew recorded. Returns None when the backend has no queryable registry
/// (script/self-managed, or a `pkgbuild` recipe whose version is only settled by
/// building upstream HEAD) or the query fails/offline.
pub fn latest_available(name: &str) -> Option<String> {
    let state = State::load().ok()?;
    let rec = state.packages.get(name)?;
    let reference = rec.reference.clone();
    match rec.backend.as_str() {
        "brew" => {
            let is_cask = rec.kind.as_deref() == Some("cask");
            let flag = if is_cask { "--cask" } else { "--formula" };
            let out = util::capture("brew", &["info", "--json=v2", flag, &reference]);
            let v: serde_json::Value = serde_json::from_str(&out).ok()?;
            let raw = if is_cask {
                v["casks"][0]["version"].as_str()?
            } else {
                v["formulae"][0]["versions"]["stable"].as_str()?
            };
            // Casks can carry a ",build" suffix, e.g. "150.0.4078,abc123".
            Some(raw.split(',').next().unwrap_or(raw).to_string())
        }
        "pacman" => {
            // Force C locale — field labels ("Version") are otherwise localized.
            let out = util::capture("env", &["LC_ALL=C", "pacman", "-Si", &reference]);
            out.lines()
                .find(|l| l.trim_start().starts_with("Version"))
                .and_then(|l| l.split(':').nth(1))
                .map(|s| clean_pkg_version(s.trim()))
        }
        "aur" => {
            let url = format!("https://aur.archlinux.org/rpc/v5/info?arg[]={reference}");
            let out = util::capture("curl", &["-fsSL", &url]);
            let v: serde_json::Value = serde_json::from_str(&out).ok()?;
            v["results"][0]["Version"].as_str().map(clean_pkg_version)
        }
        "flatpak" => {
            let out = util::capture(
                "env",
                &[
                    "LC_ALL=C",
                    "flatpak",
                    "remote-info",
                    "--cached",
                    "flathub",
                    &reference,
                ],
            );
            out.lines()
                .find(|l| l.to_ascii_lowercase().contains("version"))
                .and_then(extract)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_numerically() {
        assert_eq!(compare("6.9.4", "6.9.3"), Ordering::Greater);
        assert_eq!(compare("6.9.3", "6.9.3"), Ordering::Equal);
        assert_eq!(compare("6.9", "6.9.0"), Ordering::Equal);
        assert_eq!(compare("4.50.143", "4.9.999"), Ordering::Greater); // 50 > 9 numerically
        assert_eq!(compare("0.2.15", "0.2.9"), Ordering::Greater);
    }

    #[test]
    fn constraint_ops() {
        assert!(satisfies("6.9.3", ">=", "6.9.3"));
        assert!(satisfies("6.9.4", ">=", "6.9.3"));
        assert!(!satisfies("6.9.2", ">=", "6.9.3"));
        assert!(satisfies("2.1.204", "==", "2.1.204"));
        assert!(!satisfies("2.1.205", "==", "2.1.204"));
        assert!(satisfies("anything", "weird", "1.0")); // unknown op = no constraint
    }

    #[test]
    fn extracts_version() {
        assert_eq!(extract("2.1.204 (Claude Code)").as_deref(), Some("2.1.204"));
        assert_eq!(extract("Homebrew 6.0.8").as_deref(), Some("6.0.8"));
        assert_eq!(
            extract("llmux 0.2.15 (stable v0.2.15-098)").as_deref(),
            Some("0.2.15")
        );
        assert_eq!(extract("no version here 42").as_deref(), None);
    }

    #[test]
    fn cleans_pkg_version() {
        assert_eq!(clean_pkg_version("6.9.3-7.1"), "6.9.3");
        assert_eq!(clean_pkg_version("1:6.9.3-1"), "6.9.3");
        assert_eq!(clean_pkg_version("9.7.3"), "9.7.3");
    }
}
