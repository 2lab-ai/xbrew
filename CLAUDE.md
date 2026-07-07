# xbrew — working notes

Cross-platform (macOS + Arch Linux) package wrapper. One `install` / `uninstall`
over brew, pacman, and recipes; the backend used per package is recorded so
uninstall always routes correctly.

## The gate (run before every commit)

```sh
just check      # cargo fmt --check + clippy -D warnings + test, all --locked
```

CI (`.github/workflows/ci.yml`) runs the exact same gate on macOS and Linux.
`--locked` means **Cargo.lock is committed** and must stay in sync.

## Layout

```
src/
  main.rs       CLI (clap) + dispatch
  platform.rs   macOS vs Arch detection
  state.rs      ~/.xbrew/state.json — package -> backend record (drives uninstall)
  recipe.rs     "arch-cask" TOML recipes (built-in + ~/.xbrew/recipes/*.toml)
  resolve.rs    install/uninstall orchestration + backend helpers
  util.rs       command runners, which, paths
recipes/        built-in recipes, whole dir embedded via include_dir
build.rs        bakes XBREW_BUILD_ID into --version
```

## Backend resolution

| Platform | Order |
|----------|-------|
| macOS    | brew formula/cask → recipe (cask / dmg) |
| Arch     | brew (Linux) → pacman → recipe (AUR `makepkg` / flatpak) |

First backend that can provide the package wins; the choice lands in
`state.json` as `{ backend, reference, kind?, artifacts? }`.

## Uninstall routing

`brew` → `brew uninstall [--cask]`; `pacman`/`aur` → `sudo pacman -Rns`
(AUR builds register in the pacman DB, so this is clean); `flatpak` →
`flatpak uninstall`; `recipe-dmg` → `rm -rf` the recorded `.app`.

## Versioning & release (mirrors llmux)

- `Cargo.toml` version is the source of truth. A stable release tag **`v<x.y.z>`
  must equal it** — `release.yml` fails the build otherwise.
- **Preview**: every push to `main` → `preview.yml` builds 4 targets and publishes
  a `preview-<YYYY-MM-DD-HHMM>-<sha12>` prerelease (keeps the newest 15).
- **Stable**: `just release` tags `v<version>` and pushes → `release.yml` builds
  the 4 targets and publishes the release. `install.sh` pulls from it.
- Targets: `xbrew-macos-aarch64`, `xbrew-macos-x86_64`, `xbrew-linux-x86_64`,
  `xbrew-linux-aarch64`. Each release also carries `SHA256SUMS`.

## Distribution

`curl -fsSL .../install.sh | bash` downloads the matching prebuilt binary and
verifies it against `SHA256SUMS`. No Homebrew tap — xbrew is not distributed
through a package manager (that would be the bootstrap paradox). `xbrew
self-update` just re-runs the installer.

## Recipes ("arch-casks")

Anything not in plain brew/pacman is a small TOML file in `recipes/`
(embedded) or `~/.xbrew/recipes/*.toml` (user, overrides built-ins):

```toml
name = "nomachine"
description = "..."
[arch]
aur = "nomachine"        # or flatpak = "app.id"
[macos]
cask = "nomachine"       # or dmg = "https://.../App-arm64.dmg" + app = "App.app"
```
