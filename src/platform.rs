use crate::util;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Platform {
    MacOS,
    Arch,
    Other,
}

/// macOS, or Arch-family Linux (detected by the presence of `pacman`).
pub fn detect() -> Platform {
    if cfg!(target_os = "macos") {
        Platform::MacOS
    } else if cfg!(target_os = "linux") && util::which("pacman") {
        Platform::Arch
    } else {
        Platform::Other
    }
}
