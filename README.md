# xbrew

> 나는 패키지 매니저를 기억하기 싫다.
> 리눅스에서도 brew처럼 설치하고 삭제하겠다.
> 팩맨이고 뭐고 몰라. 묻지 마.
> 그냥 `xbrew install nomachine` 할 거야. **알아서 해줘.**

That's the whole idea. `xbrew install <thing>` — it figures out where the thing
lives (brew? pacman? AUR? flatpak? a `.dmg`?), installs it, and **remembers what
it used** so `xbrew uninstall <thing>` just works. You never think about a
package manager again. Same commands on **macOS, Arch, Debian/Ubuntu, and
RHEL/Amazon Linux**.

```sh
xbrew install nomachine telegram slack   # one or many
xbrew uninstall nomachine
xbrew list
```

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/2lab-ai/xbrew/HEAD/install.sh | bash
```

Prebuilt binary, no toolchain needed. Bleeding edge: append `XBREW_CHANNEL=preview`.
Update later with `xbrew self-update`.

Install the approved dbotter Preview on Arch Linux (aarch64 or x86_64):

```sh
xbrew install dbotter
dbotter version --format json
```

xbrew downloads the immutable release asset, verifies its pinned SHA-256, and
installs it at `~/.xbrew/bin/dbotter` without `sudo` or a source build.

## What it actually does

| Platform | It tries, in order |
|----------|--------------------|
| **macOS** | Homebrew formula/cask → recipe (cask / `.dmg`) |
| **Arch**  | recipe → Homebrew → `pacman` → **any AUR package** (`makepkg`) |
| **Debian/Ubuntu** | recipe → Homebrew → `apt` |
| **RHEL/Amazon Linux** | recipe → Homebrew → `dnf`/`yum` |

(See [docs/PLATFORMS.md](docs/PLATFORMS.md) for detection and per-platform recipe fields.)

The first one that has your package wins, and the choice is written to
`~/.xbrew/state.json`. **Already installed it by hand?** `xbrew install <name>`
detects that and just adopts it into tracking — no rebuild, no reinstall. Uninstall reads that and calls the right remover
(`brew uninstall`, `pacman -Rns`, `flatpak uninstall`, or removes the tracked
app/binary artifact) —
so **you** don't have to remember which one installed it.

## Commands

```
xbrew install <name>
xbrew uninstall <name>
xbrew list                       # what you installed, and how
xbrew bundle <manifest.yaml>...  # install a whole manifest (Brewfile-style)
xbrew version <name>             # installed version of a tracked package
xbrew update [name...]           # installed vs latest for tracked pkgs; upgrade (y/n/all)
xbrew info <name>
xbrew search <query>
xbrew self-update
```

### Bundles

`xbrew bundle` installs everything declared in one or more YAML manifests —
merged in order (e.g. a common file + a per-OS file), duplicates collapsed. It
registers `trust:` taps first (so brew-backed formulae resolve), installs each
`xbrew:` entry, and verifies optional version constraints.

```yaml
trust:
  - 2lab-ai/tap
xbrew:
  - brew                     # no constraint = latest
  - claude-code >= 2.1.204   # verified after install; also == > <= <
  - telegram >= 6.9.3
```

```sh
xbrew bundle software.yaml software.arch.yaml
```

## Recipes ("arch-cask")

Stuff that isn't a plain brew/pacman package is a tiny TOML recipe. A recipe is
**authoritative** — if one exists for a name, xbrew installs it that way instead
of guessing from a same-named brew/pacman package. Built-ins: `brew`, `claude`,
`claude-code`, `dbotter`, `nomachine`, `rustdesk`, `slack`, `sunshine`, `telegram` — and
anything in the AUR works without a recipe at all. Add your own by dropping a
file in `recipes/` (shipped) or `~/.xbrew/recipes/*.toml` (local):

```toml
name = "nomachine"
description = "NoMachine remote desktop"

[arch]
aur = "nomachine"          # or: pacman = "telegram-desktop", flatpak = "org.example.App"

[macos]
cask = "nomachine"         # or: dmg = "https://.../App-arm64.dmg", app = "App.app"
```

## Dev

`just check` (fmt + clippy + test) before every commit. See [CLAUDE.md](CLAUDE.md)
for architecture and the release flow. MIT licensed.
