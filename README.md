# nobrew

One `install` / `uninstall` across **macOS and Arch Linux** — you don't remember
which package manager did what, nobrew does.

```sh
nobrew install nomachine      # picks brew / pacman / AUR / flatpak / dmg for you
nobrew uninstall nomachine    # removes it via whatever backend installed it
nobrew list                   # what nobrew installed, and how
```

Think of it as an **"arch-cask"**: Homebrew's one-liner UX, but it also drives
`pacman`, builds AUR packages with `makepkg`, installs Flatpaks, and drops macOS
`.dmg` apps into `/Applications` — behind a single command.

## Why

Installing the same tool is different on every system: some things are in
`pacman`, some only in the AUR (which needs a helper), some are Flatpak-only,
some ship as a `.dmg`. nobrew hides that. It **remembers the backend it used**
for each package in `~/.nobrew/state.json`, so uninstall always knows the right
remover.

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/2lab-ai/nobrew/HEAD/install.sh | bash
```

(Local build: `cargo build --release`, binary at `target/release/nobrew`.)

## How it resolves a package

| Platform | Order |
|----------|-------|
| **macOS** | Homebrew formula/cask → recipe (cask override or `.dmg`) |
| **Arch**  | Homebrew (Linux) → `pacman` → recipe (`makepkg`/AUR or Flatpak) |

The first backend that can provide the package wins, and the choice is recorded.

## Uninstall routing

| Installed via | `nobrew uninstall` runs |
|---------------|-------------------------|
| brew          | `brew uninstall [--cask]` |
| pacman / AUR  | `sudo pacman -Rns` |
| flatpak       | `flatpak uninstall` |
| dmg recipe    | `rm -rf /Applications/<App>.app` |

## Recipes ("arch-casks")

Anything not in plain brew/pacman is a small TOML recipe. Built-ins live in
`recipes/`; drop your own in `~/.nobrew/recipes/*.toml` (they override built-ins).

```toml
name = "nomachine"
description = "NoMachine remote desktop"

[arch]
aur = "nomachine"          # or: flatpak = "org.example.App"

[macos]
cask = "nomachine"         # or: dmg = "https://.../App-arm64.dmg", app = "App.app"
```

## Commands

```
nobrew install <name>
nobrew uninstall <name>
nobrew list
nobrew info <name>
nobrew search <query>
```

## Status

Early MVP. Built-in recipes: `nomachine`, `rustdesk`, `sunshine`.
