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

When a macOS recipe declares more than one backend the order is fixed —
**cask → formula → dmg → script**, first one declared wins (`resolve::macos_backend`).

The choice lands in `state.json` as `{ backend, reference, kind?, artifacts? }`.
Each backend adopts an already-installed package instead of reinstalling.
Privileged commands go through `util::run_priv` (sudo unless root). See
[docs/PLATFORMS.md](docs/PLATFORMS.md).

## Uninstall routing

`brew` → `brew uninstall [--cask]`; `pacman`/`aur`/`pkgbuild` → `sudo pacman
-Rns` (makepkg registers its builds in the pacman DB, so this is clean);
`flatpak` → `flatpak uninstall`; `recipe-dmg` → remove the recorded `.app`;
`recipe-binary` → remove only its validated `~/.xbrew/bin/<name>` artifact.

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

A tool that ships as a **formula** rather than an app bundle uses `formula`
instead of `cask` — plain name or fully-qualified, which auto-taps on install:

```toml
[macos]
formula = "2lab-ai/tap/somawork"
```

It records as `{ backend = "brew", kind = "formula" }`, so uninstall/update/
version route through the same generic brew paths a native formula does. The
token is data handed to `brew` as exactly one argv item and never through a
shell: `resolve::validate_formula_token` rejects anything outside Homebrew's
grammar (whitespace, a leading dash, `.`/`..`, extra or empty path components,
shell metacharacters) *before brew is ever handed the token* — and, on the
formula and cask recipe paths, before the Homebrew bootstrap and the PATH probe
it opens with, so an unusable token buys no subprocess at all. `macos.cask` tokens
get `resolve::validate_cask_token`, which is the *same* function body under a
second name (`validate_reference_token`), so a cask can never be allowed
something a formula is not.

#### Homebrew is bootstrapped, not demanded

A recipe that declares `macos.cask`/`macos.formula` names Homebrew as its
backend, so on a machine without brew that is a **dependency to resolve**, not a
refusal: `resolve::ensure_brew` runs the `recipes/brew.toml` installer (via
`recipe::builtin("brew")` — the installer command exists in exactly one place)
and then looks for what it produced. It runs only when `brew` is genuinely
unreachable, only *after* the recipe's cask/formula token has been validated,
and its record is dropped: an implicit dependency is not a package the user
asked for, so `state.json` gains no `brew` entry — `xbrew install brew` is still
how xbrew comes to own Homebrew itself.

Three things make that sentence load-bearing rather than decorative:

- **The built-in recipe, not the user's.** `recipe::builtin` skips
  `~/.xbrew/recipes` entirely. `recipe::get` merges user recipes over built-ins,
  so with it, asking for an unrelated *formula* would execute whatever anything
  able to write the user's home had left in `~/.xbrew/recipes/brew.toml`. An
  override is a choice about a package you asked for, not a licence to run
  during someone else's install — so explicit `xbrew install brew` still goes
  through `get()` and still honours an override.
- **Reachable ≠ `command -v brew`.** `resolve::brew_reachable` probes PATH, then
  the prefixes in order: `$HOMEBREW_PREFIX/bin/brew` when that variable is set,
  then `/opt/homebrew/bin/brew`, then `/usr/local/bin/brew`, duplicates
  collapsed. The declared prefix goes **first, not instead** — first hit wins, so
  a correct `HOMEBREW_PREFIX` is still the only path ever examined, while a stale
  one (a prefix that moved or was removed) falls through instead of reporting a
  Homebrew-bearing machine as brew-less. A candidate counts only as a regular
  executable file, and its verified parent directory is prepended to the current
  process's PATH — a **process-global** change every later child inherits, so
  that directory gains ordinary PATH precedence for *all* subsequent commands,
  not only `brew`. That is a trust decision about the discovered prefix, and the
  same trust that running its `brew` already implies (brew executes that
  prefix's own Ruby and formula code); what the prepend widens is the scope, not
  the trust. This runs **both** before deciding to install and again
  after the installer returns, which covers the two real conditions: Homebrew's
  installer exits 0 into a prefix the already-running process's PATH does not
  carry, and `/opt/homebrew/bin/brew` is off-PATH in any process whose
  environment never went through `brew shellenv`. In the second case the
  bootstrap must run **zero** times — the Homebrew is adopted, not re-installed
  over. `resolve::brew_present` is the single answer every macOS
  brew-presence decision shares, so no two of them can disagree about whether
  this machine has Homebrew: the `provides_bin` adoption check (`xbrew install
  brew` adopts an off-PATH Homebrew instead of reinstalling), the no-recipe
  fallback (which used to demand `xbrew install brew` on a machine that had it),
  `bundle`'s trust gate, and `search`'s `== brew ==` section (which a plain PATH
  probe dropped *silently* — no error, just a shorter answer). Scoped to macOS:
  off macOS `brew_present` is a plain `which`, and `provides_bin` for every
  binary other than `brew` always was.

  What deliberately does **not** ask, and is the remaining PATH-blind spot:
  `uninstall`, `upgrade` and `version::installed`/`latest_available` invoke
  `brew` directly with no presence gate at all, so on a Mac whose Homebrew is
  off-PATH they fail with ``failed to launch `brew` `` rather than discovering
  it. That is a different shape of bug — a missing call, not a wrong gate — and
  it is untouched here.

  Because those two fallbacks are absolute paths that no env var can redirect,
  the lifecycle tests launch xbrew through `sandbox-exec` with a profile that
  denies reads of them (`HIDE_HOST_HOMEBREW`). Without it, every "brew was not
  found" test on a Mac that *has* Homebrew would adopt the real one and go on to
  run `brew install` for real. The denial is measured, not assumed, by
  `the_seatbelt_profile_actually_hides_the_host_homebrew`.
- **At most one attempt per invocation.** A `BrewBootstrap`
  (`NotAttempted | Succeeded | Failed(String)`) is threaded `install_many →
  install → install_macos → ensure_brew`. `bundle` shares one too, and its
  `trust:` gate acquires brew through the *same* `ensure_brew_for` rather than an
  explicit `install("brew")` — that route left the context untouched, so the
  manifest's first brew-backed entry would install Homebrew a second time (and
  the gate would honour a user override for a dependency nobody asked for). A
  second brew-backed name on the same command line reuses the first attempt's
  outcome
  — the failure is cached as its *rendered* chain, so package two reports `sh`
  exited with `exit status: 3` rather than a fresh, uninformative "brew isn't
  installed". Invocation-local by hand rather than a static, which would leak
  across tests and library calls.

Names with **no recipe** keep the old refusal: nothing declared Homebrew as
their backend, so installing a package manager to look up a name that may not
exist isn't what was asked.

#### Tap trust — why a fully-qualified install needs one extra call

Homebrew refuses to load formulae from an untrusted third-party tap. Installing
`user/tap/formula` grants **implicit trust to that one named formula and to
nothing else** — Homebrew's `cmd/install.rb` passes only `args.named` to
`Trust.trust_fully_qualified_items!`, and that helper records a `:formula`
entry, never the tap. So the install succeeds and then dies on the first
dependency the formula pulls from its *own* tap: "Refusing to load formula … from
untrusted tap …". Trusting the tap covers every item in it
(`Homebrew::Trust.trusted?` short-circuits on `trusted_tap?`).

`brew upgrade` has the identical gap — Homebrew's `cmd/upgrade.rb` also calls
`Trust.trust_fully_qualified_items!(args.named)` and only then resolves the
formulae — so both paths carry the trust hop.

Install of a **non-installed fully-qualified** `macos.formula`:

```
brew list --formula --versions user/tap/formula   # already installed?
brew trust --tap user/tap                         # ← the fix
brew install user/tap/formula
```

Upgrade of a tracked `{ backend = "brew", kind = "formula" }` record whose
reference is fully-qualified:

```
brew list --versions user/tap/formula             # installed version
brew info --json=v2 --formula user/tap/formula    # latest version
brew trust --tap user/tap                         # ← the fix
brew upgrade user/tap/formula
```

The upgrade hop is not redundant with the install hop: **adoption trusts
nothing**, so every machine that installed one of these formulae before this
change has a record and an untrusted tap, and its first `xbrew update` is where
it would otherwise break.

Order is load-bearing (trust after the install/upgrade is too late for the
dependency it resolves), and the grant is tap-level on purpose: xbrew cannot know
what a formula requires, so it does not enumerate dependencies. The tap is
derived from the token's first two components (`resolve::tap_of_formula`) and
re-checked by `resolve::validate_tap_token` before brew sees it.

Three things are deliberately *not* done:

- **No `brew tap`.** `brew install` already calls `tap&.ensure_installed!` on a
  fully-qualified name, so tapping first would only duplicate it. (The manifest
  `trust:` path is different — see below.)
- **Nothing on adoption.** An already-installed formula resolves no
  dependencies, so it mutates no trust state. The upgrade hop is what covers
  those records later.
- **Plain names and casks are untouched.** A bare name belongs to no tap, and
  trust scope here is `macos.formula` only.

`brew trust --tap` is best-effort: a Homebrew old enough to lack the `trust`
subcommand has no trust gate to satisfy either, and a newer one that fails to
record the grant refuses the install a moment later with its own instructions.
Failing hard would break the first case to warn about the second. The warning
names the tap, so a user on a Homebrew that *does* enforce trust knows what to
run by hand. An unusable *tap token* is still a hard error — best-effort covers
the subprocess, not the validation.

#### Known gaps in tap trust

Both are real and neither is fixed here; do not assume otherwise from the above.

- **Casks.** `trusted?` treats casks identically (`trustedcasks` in
  `trust.json`), so a fully-qualified cask with a same-tap dependency hits the
  same refusal. The cask path gets no trust call. (Token *validation* it now has
  — `validate_cask_token`, before the bootstrap and before brew — so this gap is
  the tap grant alone.)
- **The no-recipe fallback takes a user-typed token.** When no recipe matches,
  `install_macos` passes the name **straight from argv** to `brew_provides` and
  `brew_install`. That name is not restricted to bare names: `xbrew install
  vendor/tap/thing` runs `brew info --formula vendor/tap/thing` then `brew
  install vendor/tap/thing`, records it as a formula, and trusts nothing — it
  routes around both `validate_formula_token` and the tap trust. (Measured, not
  inferred.) Its *upgrade* is covered, because `upgrade` derives the tap from
  the recorded reference like any other formula record.

`xbrew bundle`'s manifest `trust:` list needs **both** calls — `brew tap <tap>`
*and* `brew trust --tap <tap>`, in that order, before any entry is installed.
Tapping is what makes a plain formula name from that tap resolve at all;
trusting is what lets Homebrew load it. This path used to run only `brew tap`,
which read like trust and wasn't. Each declared tap is validated up front, so a
manifest that can't express a tap fails closed instead of installing its entries
against whatever did register.

An Arch-only prebuilt executable can instead use `[arch.binary]` with a safe
`name` and required `aarch64`/`x86_64` HTTPS URL + SHA-256 tables. xbrew
verifies the selected immutable asset before atomically installing it under
`~/.xbrew/bin`; this backend never invokes a shell installer.

The file name must match `name` — it's the key users type, and a mismatch makes
the recipe unreachable. `registry()` **skips a recipe that fails to parse**, so
a typo would ship as a missing package rather than an error; the tests in
`recipe.rs` parse every embedded recipe to catch that at build time. Those tests
read `BUILTIN_RECIPES` directly, never `get()`/`registry()`, which would merge
the developer's own `~/.xbrew/recipes` into a shipped-recipe assertion.

`include_dir!` only embeds files that existed at compile time, and emitting any
`rerun-if-*` directive turns off cargo's rerun-on-any-change default — so
`build.rs` must keep `cargo:rerun-if-changed=recipes`. Without it, adding a
recipe recompiles nothing and the suite goes green against a binary that does
not contain it.

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
