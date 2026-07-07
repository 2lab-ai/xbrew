#!/usr/bin/env bash
# nobrew installer — downloads a prebuilt binary, no toolchain needed.
#   curl -fsSL https://raw.githubusercontent.com/2lab-ai/nobrew/HEAD/install.sh | bash
#   ... | NOBREW_CHANNEL=preview bash    # bleeding-edge preview build
set -euo pipefail

REPO="${NOBREW_REPO_SLUG:-2lab-ai/nobrew}"
CHANNEL="${NOBREW_CHANNEL:-stable}"
PREFIX="${NOBREW_PREFIX:-$HOME/.nobrew}"
BINDIR="$PREFIX/bin"

say()  { printf '\033[1m==>\033[0m %s\n' "$*"; }
die()  { printf '\033[31merror:\033[0m %s\n' "$*" >&2; exit 1; }

# 1. Detect platform
case "$(uname -s)" in
  Darwin) OSNAME=macos ;;
  Linux)  OSNAME=linux ;;
  *) die "unsupported OS: $(uname -s) (nobrew targets macOS and Linux)" ;;
esac
case "$(uname -m)" in
  arm64|aarch64) ARCHNAME=aarch64 ;;
  x86_64|amd64)  ARCHNAME=x86_64 ;;
  *) die "unsupported arch: $(uname -m)" ;;
esac
ASSET="nobrew-${OSNAME}-${ARCHNAME}"
say "nobrew installer — ${OSNAME}/${ARCHNAME}, channel: ${CHANNEL}"

# 2. Resolve the release tag
if [ "$CHANNEL" = "preview" ]; then
  TAG="$(curl -fsSL "https://api.github.com/repos/${REPO}/releases?per_page=30" \
    | grep -o '"tag_name": *"preview-[^"]*"' | head -1 | sed 's/.*"\(preview-[^"]*\)"/\1/')"
  [ -n "$TAG" ] || die "no preview release found for ${REPO}"
else
  # /releases/latest redirects to /tag/<latest stable>; read it without jq.
  TAG="$(curl -fsSLI -o /dev/null -w '%{url_effective}' \
    "https://github.com/${REPO}/releases/latest" | sed 's#.*/tag/##')"
  case "$TAG" in
    ""|*/*) die "no stable release yet for ${REPO} — try NOBREW_CHANNEL=preview" ;;
  esac
fi
say "release: ${TAG}"

# 3. Download binary + checksums
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
BASE="https://github.com/${REPO}/releases/download/${TAG}"
say "downloading ${ASSET}"
curl -fsSL -o "$TMP/$ASSET" "${BASE}/${ASSET}" || die "download failed: ${BASE}/${ASSET}"
curl -fsSL -o "$TMP/SHA256SUMS" "${BASE}/SHA256SUMS" || die "missing SHA256SUMS on ${TAG}"

# 4. Verify checksum
say "verifying checksum"
EXPECTED="$(grep " ${ASSET}\$" "$TMP/SHA256SUMS" | awk '{print $1}')"
[ -n "$EXPECTED" ] || die "no checksum for ${ASSET} in SHA256SUMS"
if command -v sha256sum >/dev/null 2>&1; then
  ACTUAL="$(sha256sum "$TMP/$ASSET" | awk '{print $1}')"
else
  ACTUAL="$(shasum -a 256 "$TMP/$ASSET" | awk '{print $1}')"
fi
[ "$EXPECTED" = "$ACTUAL" ] || die "checksum mismatch (expected $EXPECTED, got $ACTUAL)"

# 5. Install
mkdir -p "$BINDIR"
install -m 0755 "$TMP/$ASSET" "$BINDIR/nobrew"
say "installed to ${BINDIR}/nobrew"

# 6. PATH hint
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
say "done — try: nobrew install nomachine"
