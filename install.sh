#!/usr/bin/env bash
# nobrew installer — curl -fsSL <url>/install.sh | bash
set -euo pipefail

REPO="${NOBREW_REPO:-https://github.com/2lab-ai/nobrew.git}"
PREFIX="${NOBREW_PREFIX:-$HOME/.nobrew}"
BINDIR="$PREFIX/bin"

say() { printf '\033[1m==>\033[0m %s\n' "$*"; }

case "$(uname -s)" in
  Darwin) PLATFORM="macOS" ;;
  Linux)  PLATFORM="Linux" ;;
  *) echo "nobrew: unsupported OS: $(uname -s)" >&2; exit 1 ;;
esac
say "nobrew installer (platform: $PLATFORM)"

# 1. Rust toolchain
if ! command -v cargo >/dev/null 2>&1; then
  say "Rust toolchain not found — installing"
  if command -v brew >/dev/null 2>&1; then
    brew install rust
  else
    curl -fsSL https://sh.rustup.rs | sh -s -- -y
    # shellcheck disable=SC1091
    source "$HOME/.cargo/env" 2>/dev/null || true
  fi
fi

# 2. Fetch source
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
say "fetching source from $REPO"
git clone --depth 1 "$REPO" "$WORK/nobrew"

# 3. Build
say "building (cargo build --release) — first build takes a minute"
( cd "$WORK/nobrew" && cargo build --release )

# 4. Install binary
mkdir -p "$BINDIR"
install -m 0755 "$WORK/nobrew/target/release/nobrew" "$BINDIR/nobrew"
say "installed to $BINDIR/nobrew"

# 5. PATH hint
case ":$PATH:" in
  *":$BINDIR:"*) ;;
  *)
    echo
    echo "Add nobrew to your PATH:"
    echo "  fish:      fish_add_path $BINDIR"
    echo "  bash/zsh:  echo 'export PATH=\"$BINDIR:\$PATH\"' >> ~/.profile"
    ;;
esac
echo
say "done — try: nobrew --help"
