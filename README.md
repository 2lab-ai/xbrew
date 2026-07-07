# nobrew

> 나는 패키지 매니저를 기억하기 싫다.
> 리눅스에서도 brew처럼 설치하고 삭제하겠다.
> 팩맨이고 뭐고 몰라. 묻지 마.
> 그냥 `nobrew install nomachine` 할 거야. **알아서 해줘.**

That's the whole idea. `nobrew install <thing>` — it figures out where the thing
lives (brew? pacman? AUR? flatpak? a `.dmg`?), installs it, and **remembers what
it used** so `nobrew uninstall <thing>` just works. You never think about a
package manager again. Same two commands on **macOS and Arch Linux**.

```sh
nobrew install nomachine
nobrew uninstall nomachine
nobrew list
```

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/2lab-ai/nobrew/HEAD/install.sh | bash
```

Prebuilt binary, no toolchain needed. Bleeding edge: append `NOBREW_CHANNEL=preview`.
Update later with `nobrew self-update`.

## What it actually does

| Platform | It tries, in order |
|----------|--------------------|
| **macOS** | Homebrew formula/cask → recipe (cask / `.dmg`) |
| **Arch**  | recipe (AUR `makepkg` / Flatpak) → Homebrew → `pacman` → **any AUR package** (`makepkg`) |

The first one that has your package wins, and the choice is written to
`~/.nobrew/state.json`. Uninstall reads that and calls the right remover
(`brew uninstall`, `pacman -Rns`, `flatpak uninstall`, or deletes the `.app`) —
so **you** don't have to remember which one installed it.

## Commands

```
nobrew install <name>
nobrew uninstall <name>
nobrew list            # what you installed, and how
nobrew info <name>
nobrew search <query>
nobrew self-update
```

## Recipes ("arch-cask")

Stuff that isn't a plain brew/pacman package is a tiny TOML recipe. A recipe is
**authoritative** — if one exists for a name, nobrew installs it that way instead
of guessing from a same-named brew/pacman package. Built-ins: `brew`, `claude`,
`claude-code`, `nomachine`, `rustdesk`, `slack`, `sunshine`, `telegram` — and
anything in the AUR works without a recipe at all. Add your own by dropping a
file in `recipes/` (shipped) or `~/.nobrew/recipes/*.toml` (local):

```toml
name = "nomachine"
description = "NoMachine remote desktop"

[arch]
aur = "nomachine"          # or: flatpak = "org.example.App"

[macos]
cask = "nomachine"         # or: dmg = "https://.../App-arm64.dmg", app = "App.app"
```

## Dev

`just check` (fmt + clippy + test) before every commit. See [CLAUDE.md](CLAUDE.md)
for architecture and the release flow. MIT licensed.
