#!/bin/sh
# Builds the Linux release files for this machine's processor:
#   dist/cashcraft-<ver>-linux-<arch>.tar.gz    both programs, desktop entry, icon, licences
#   dist/cashcraft-<ver>-linux-<arch>.deb       Debian/Ubuntu package (cargo-packager)
#   dist/cashcraft-<ver>-linux-<arch>.AppImage  single-file app (cargo-packager)
#
# Usage: packaging/linux/package.sh        (needs: cargo install cargo-packager --locked)
set -eu
cd "$(dirname "$0")/../.."

VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)
case "$(uname -m)" in
    x86_64 | amd64) ARCH=x86_64 ;;
    aarch64 | arm64) ARCH=aarch64 ;;
    riscv64) ARCH=riscv64 ;;
    *) echo "unsupported processor: $(uname -m)" >&2; exit 2 ;;
esac
ID=io.github.mmazzieri.cashcraft
DIST=dist
mkdir -p "$DIST"

echo "→ Building CashCraft $VERSION ($ARCH)"
cargo build --release --bins

NAME="cashcraft-$VERSION-linux-$ARCH"
STAGE="$DIST/$NAME"
echo "→ Creating $DIST/$NAME.tar.gz"
rm -rf "$STAGE"
mkdir -p "$STAGE/bin" "$STAGE/share/applications" "$STAGE/share/icons/hicolor/256x256/apps" \
    "$STAGE/share/icons/hicolor/512x512/apps"
cp target/release/cashcraft target/release/cashcraft-cli "$STAGE/bin/"
cp packaging/linux/$ID.desktop "$STAGE/share/applications/"
cp assets/icons/256x256.png "$STAGE/share/icons/hicolor/256x256/apps/$ID.png"
cp assets/icons/512x512.png "$STAGE/share/icons/hicolor/512x512/apps/$ID.png"
cp LICENSE-MIT LICENSE-APACHE NOTICE README.md "$STAGE/"
tar -C "$DIST" -czf "$DIST/$NAME.tar.gz" "$NAME"
rm -rf "$STAGE"

echo "→ Creating .deb and .AppImage"
rm -rf target/packager
APPIMAGE_EXTRACT_AND_RUN=1 cargo packager --release --formats deb,appimage
for f in target/packager/*.deb; do cp "$f" "$DIST/$NAME.deb"; done
for f in target/packager/*.AppImage; do cp "$f" "$DIST/$NAME.AppImage"; chmod +x "$DIST/$NAME.AppImage"; done

ls -l "$DIST/$NAME".*
