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
  resolve.rs    install/uninstall/bundle/update orchestration + backend helpers
  manifest.rs   YAML-subset parser for `xbrew bundle` manifests (dependency-free)
  version.rs    installed + latest-available version resolution (backend-aware) + constraints
  util.rs       command runners, which, paths
recipes/        built-in recipes, whole dir embedded via include_dir
build.rs        bakes XBREW_BUILD_ID into --version
```

## Backend resolution

A curated **recipe is authoritative** (wins over a same-named native package).
Otherwise, per platform:

| Platform | Order |
|----------|-------|
| macOS    | recipe → brew formula/cask |
| Arch     | recipe → brew → pacman → any AUR pkg (`makepkg`) |
| Debian   | recipe → brew → apt |
| RHEL     | recipe → brew → dnf/yum |

The choice lands in `state.json` as `{ backend, reference, kind?, artifacts? }`.
Each backend adopts an already-installed package instead of reinstalling.
Privileged commands go through `util::run_priv` (sudo unless root). See
[docs/PLATFORMS.md](docs/PLATFORMS.md).

## Uninstall routing

`brew` → `brew uninstall [--cask]`; `pacman`/`aur`/`pkgbuild` → `sudo pacman
-Rns` (makepkg registers its builds in the pacman DB, so this is clean);
`flatpak` → `flatpak uninstall`; `recipe-dmg` → `rm -rf` the recorded `.app`.

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
aur = "nomachine"        # or pacman = "pkg" / flatpak = "app.id"
[macos]
cask = "nomachine"       # or dmg = "https://.../App-arm64.dmg" + app = "App.app"
```

The file name must match `name` — it's the key users type, and a mismatch makes
the recipe unreachable. `registry()` **skips a recipe that fails to parse**, so
a typo would ship as a missing package rather than an error; the tests in
`recipe.rs` parse every embedded recipe to catch that at build time.

### `arch.pkgbuild` — upstreams that ship a PKGBUILD but never published it

`aur = "x"` clones `aur.archlinux.org/x.git`. When the PKGBUILD only lives in
the project's own repo, point at it directly:

```toml
[arch]
pkgbuild = "https://github.com/2lab-ai/llmux.git"
dir = "llmux-islands-linux/packaging/arch"   # where the PKGBUILD sits
pkg = "llmux-islands-git"                    # the pkgname it produces
```

All three are required. The build is the same `makepkg -si` as `aur`, so it
lands in the pacman DB and uninstall/`pacman -Q` route normally — it's recorded
under its own `pkgbuild` backend only so `update` rebuilds from the right remote.

These are typically VCS (`-git`) packages tracking upstream HEAD, which has no
queryable "latest": `latest_available` returns None, `xbrew update` shows them
as `git` rather than claiming `ok`, and a bare `xbrew update` won't churn on
them. `xbrew update <name>` forces the rebuild.
