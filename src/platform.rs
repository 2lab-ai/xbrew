use crate::util;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Platform {
    MacOS,
    /// Arch family (pacman): Arch, CachyOS, Manjaro, EndeavourOS, …
    Arch,
    /// Debian family (apt/dpkg): Debian, Ubuntu, Mint, …
    Debian,
    /// RHEL family (dnf/yum/rpm): Amazon Linux, Fedora, RHEL, Rocky, …
    Rhel,
    Other,
}

/// Detect the platform: macOS, or a Linux family identified from
/// `/etc/os-release` (ID / ID_LIKE), falling back to whichever package
/// manager is present.
pub fn detect() -> Platform {
    if cfg!(target_os = "macos") {
        return Platform::MacOS;
    }
    if !cfg!(target_os = "linux") {
        return Platform::Other;
    }

    let ids = os_release_ids();
    let has = |needle: &str| ids.iter().any(|id| id == needle);

    if has("arch") || has("cachyos") || has("manjaro") || has("endeavouros") {
        return Platform::Arch;
    }
    if has("debian") || has("ubuntu") || has("linuxmint") || has("pop") {
        return Platform::Debian;
    }
    if has("rhel")
        || has("fedora")
        || has("amzn")
        || has("centos")
        || has("rocky")
        || has("almalinux")
    {
        return Platform::Rhel;
    }

    if util::which("pacman") {
        Platform::Arch
    } else if util::which("apt-get") {
        Platform::Debian
    } else if util::which("dnf") || util::which("yum") {
        Platform::Rhel
    } else {
        Platform::Other
    }
}

/// All ID-ish tokens from /etc/os-release: the `ID` plus each word of `ID_LIKE`.
fn os_release_ids() -> Vec<String> {
    let mut ids = Vec::new();
    let text = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
    for line in text.lines() {
        let Some((key, val)) = line.split_once('=') else {
            continue;
        };
        if key == "ID" || key == "ID_LIKE" {
            let val = val.trim().trim_matches('"');
            for token in val.split_whitespace() {
                ids.push(token.to_ascii_lowercase());
            }
        }
    }
    ids
}
