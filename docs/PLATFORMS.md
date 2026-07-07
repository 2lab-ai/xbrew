# Platforms

xbrew detects the OS from `/etc/os-release` (`ID` / `ID_LIKE`), falling back to
whichever package manager is on `PATH`.

| Family | Detected as | Native pkg mgr | Adopt probe | Install | Uninstall |
|--------|-------------|----------------|-------------|---------|-----------|
| macOS | `MacOS` | Homebrew | `brew list` | `brew install [--cask]` | `brew uninstall [--cask]` |
| Arch (Arch, CachyOS, Manjaro, EndeavourOS) | `Arch` | pacman + AUR | `pacman -Qq` | `pacman -S` / AUR `makepkg -si` | `pacman -Rns` |
| Debian (Debian, Ubuntu, Mint, Pop!_OS) | `Debian` | apt | `dpkg -s` | `apt-get install -y` | `apt-get remove -y` |
| RHEL (Amazon Linux, Fedora, RHEL, Rocky, Alma) | `Rhel` | dnf / yum | `rpm -q` | `dnf install -y` | `dnf remove -y` |

## Resolution order (per platform)

A curated recipe is **authoritative** — if a recipe declares a backend for the
current platform, that wins. Otherwise:

- **macOS**: brew formula/cask → recipe (cask / dmg / script)
- **Arch**: recipe (pacman/aur/flatpak/script) → brew → pacman → any AUR pkg
- **Debian**: recipe (apt/flatpak/script) → brew → apt
- **RHEL**: recipe (dnf/flatpak/script) → brew → dnf/yum

Every backend first checks whether the package is **already installed** and, if
so, adopts it into `~/.xbrew/state.json` instead of reinstalling.

## Privilege

Package installs/removes run through `sudo` unless xbrew is already running as
root (containers, servers) — then `sudo` is skipped. See `util::run_priv`.

## Recipe fields per platform

```toml
[arch]
pacman = "telegram-desktop"   # official repo package (friendly-name -> real name)
aur    = "slack-desktop"      # AUR package, built with makepkg
flatpak = "org.example.App"

[debian]
apt = "telegram-desktop"
flatpak = "org.example.App"

[rhel]
dnf = "telegram-desktop"
flatpak = "org.example.App"

[macos]
cask = "telegram"
# or: dmg = "https://.../App-arm64.dmg", app = "App.app"

[script]                      # self-installing tools (brew, claude-code)
provides_bin = "claude"
install = "curl -fsSL https://.../install.sh | bash"
uninstall = "..."             # optional
```

## Testing

`.github/workflows/distro-smoke.yml` validates the Debian and RHEL paths on
real distros (Ubuntu runner + Amazon Linux 2023 container): detection,
`info`, adoption of a preinstalled package, and a real install/uninstall cycle.
Local dev happens on Arch/macOS; the container smoke test is the CI gate for the
apt/dnf backends.

## Status

- macOS — stable
- Arch / CachyOS — stable (daily-driven)
- Debian / Ubuntu — implemented, validated in CI
- RHEL / Amazon Linux — implemented, validated in CI (dnf; yum fallback untested)
