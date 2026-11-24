#!/usr/bin/env bash
# Build and package CashCraft for Linux (<arch> is x86_64 or aarch64):
#
#   $DIST/cashcraft-<version>-linux-<arch>.deb       Debian, Ubuntu, Mint, ...
#   $DIST/cashcraft-<version>-linux-<arch>.rpm       Fedora, openSUSE, RHEL, ...
#   $DIST/cashcraft-<version>-linux-<arch>.tar.gz    plain tree (bin/, share/)
#
# No AppImage: its runtime links libfuse (LGPL-2.1), which the license policy keeps out of releases.
#
# Usage: packaging/linux/package.sh [--skip-build] [--formats "deb rpm tar"]
#
# Needs: cargo; nfpm for deb/rpm. Build on an
# old distro (CI: Ubuntu 22.04, glibc 2.35) so the binary runs on newer ones.
# Adapted from storytold/craft-launcher (packaging/linux/package.sh, MIT OR Apache-2.0).
set -euo pipefail
# shellcheck source=../env.sh
. "$(dirname "${BASH_SOURCE[0]}")/../env.sh"
HERE="$ROOT/packaging/linux"

SKIP_BUILD=0
FORMATS="deb rpm tar"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --skip-build) SKIP_BUILD=1; shift ;;
    --formats) FORMATS="$(printf %s "$2" | tr ',' ' ')"; shift 2 ;;
    -h | --help) sed -n '2,18p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

ARCH="$(uname -m)"
case "$ARCH" in
  x86_64) DEB_ARCH=amd64 ;;
  aarch64 | arm64) ARCH=aarch64; DEB_ARCH=arm64 ;;
  *) echo "unsupported architecture $ARCH" >&2; exit 2 ;;
esac
BASENAME="$PKG-$VERSION-linux-$ARCH"
has() { case " $FORMATS " in *" $1 "*) return 0 ;; *) return 1 ;; esac; }

echo "==> $APP_NAME $VERSION for Linux $ARCH ($FORMATS)"
if [[ "$SKIP_BUILD" = 0 ]]; then
  (cd "$ROOT" && cargo build --release --locked -p "$PKG" -p "$PKG-cli")
fi
BIN="$CARGO_TARGET_DIR/release"
WORK="$CARGO_TARGET_DIR/linux-package"
STAGE="$WORK/root"
rm -rf "$WORK"

# ---- FHS tree shared by every format -------------------------------------------------------
for b in "$PKG" "$PKG-cli"; do
  install -Dm755 "$BIN/$b" "$STAGE/usr/bin/$b"
  strip "$STAGE/usr/bin/$b" 2>/dev/null || true
done
install -Dm644 "$HERE/$APP_ID.desktop" "$STAGE/usr/share/applications/$APP_ID.desktop"
mkdir -p "$STAGE/usr/share/icons"
cp -R "$ROOT/assets/app-icon/hicolor" "$STAGE/usr/share/icons/"
mkdir -p "$STAGE/usr/share/doc/$PKG"
copy_docs "$STAGE/usr/share/doc/$PKG"
if command -v desktop-file-validate >/dev/null; then
  desktop-file-validate "$STAGE/usr/share/applications/$APP_ID.desktop"
fi

# ---- .tar.gz -------------------------------------------------------------------------------
if has tar; then
  mkdir -p "$WORK/tar"
  cp -R "$STAGE/usr" "$WORK/tar/$BASENAME"
  tar -C "$WORK/tar" -czf "$DIST/$BASENAME.tar.gz" "$BASENAME"
  echo "wrote $DIST/$BASENAME.tar.gz"
fi

# ---- .deb / .rpm ---------------------------------------------------------------------------
if has deb || has rpm; then
  command -v nfpm >/dev/null || { echo "error: nfpm not found (https://nfpm.goreleaser.com/install/)" >&2; exit 1; }
  export NFPM_ARCH="$DEB_ARCH"
  export GLIBC_FLOOR="${GLIBC_FLOOR:-2.35}"
  sed "s|\${STAGE}|$STAGE|g" "$HERE/nfpm.yaml" >"$WORK/nfpm.yaml"
  for fmt in deb rpm; do
    if has "$fmt"; then (cd "$ROOT" && nfpm package -f "$WORK/nfpm.yaml" -p "$fmt" -t "$DIST/$BASENAME.$fmt"); fi
  done
fi

echo "==> done"
ls -lh "$DIST"
