#!/bin/sh
# Builds CashCraft.app and the release files for macOS:
#   dist/cashcraft-<ver>-macos-<arch>.dmg       the app, for ArtCraft Launcher and manual installs
#   dist/cashcraft-cli-<ver>-macos-<arch>.zip   the command line / MCP server on its own
#
# Usage: packaging/macos/package.sh [--arch universal|native] [--install]
#
# Signing: with APPLE_SIGNING_IDENTITY set ("Developer ID Application: Name (TEAMID)") the app
# and the disk image are signed with the hardened runtime; with APPLE_ID, APPLE_TEAM_ID and
# APPLE_APP_PASSWORD also set they are notarized and stapled. Without them the build is signed
# ad hoc: it runs on this Mac, but ArtCraft Launcher only installs Developer ID signed apps.
set -eu
cd "$(dirname "$0")/../.."
[ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"

ARCH=universal
INSTALL=0
while [ $# -gt 0 ]; do
    case "$1" in
        --arch) ARCH="$2"; shift 2 ;;
        --install) INSTALL=1; shift ;;
        *) echo "unknown option: $1" >&2; exit 2 ;;
    esac
done

VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)
DIST=dist
APP="$DIST/CashCraft.app"
mkdir -p "$DIST"

echo "→ Building CashCraft $VERSION ($ARCH)"
case "$ARCH" in
    universal)
        rustup target add aarch64-apple-darwin x86_64-apple-darwin >/dev/null
        for t in aarch64-apple-darwin x86_64-apple-darwin; do
            MACOSX_DEPLOYMENT_TARGET=11.0 cargo build --release --bins --target "$t"
        done
        BIN_DIR="target/universal-apple-darwin/release"
        mkdir -p "$BIN_DIR"
        for b in cashcraft cashcraft-cli; do
            lipo -create -output "$BIN_DIR/$b" \
                "target/aarch64-apple-darwin/release/$b" "target/x86_64-apple-darwin/release/$b"
        done
        ;;
    native)
        MACOSX_DEPLOYMENT_TARGET=11.0 cargo build --release --bins
        BIN_DIR="target/release"
        case "$(uname -m)" in arm64) ARCH=aarch64 ;; *) ARCH=x86_64 ;; esac
        ;;
    *) echo "--arch must be universal or native" >&2; exit 2 ;;
esac

echo "→ Assembling $APP"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN_DIR/cashcraft" "$BIN_DIR/cashcraft-cli" "$APP/Contents/MacOS/"

ICONSET="$DIST/CashCraft.iconset"
rm -rf "$ICONSET"
mkdir -p "$ICONSET"
SRC=assets/icons/1024x1024.png
for s in 16 32 128 256 512; do
    sips -z $s $s "$SRC" --out "$ICONSET/icon_${s}x${s}.png" >/dev/null
    d=$((s * 2))
    sips -z $d $d "$SRC" --out "$ICONSET/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/CashCraft.icns"
rm -rf "$ICONSET"

cat > "$APP/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>CashCraft</string>
    <key>CFBundleDisplayName</key><string>CashCraft</string>
    <key>CFBundleIdentifier</key><string>io.github.mmazzieri.cashcraft</string>
    <key>CFBundleExecutable</key><string>cashcraft</string>
    <key>CFBundleIconFile</key><string>CashCraft</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>$VERSION</string>
    <key>CFBundleVersion</key><string>$VERSION</string>
    <key>LSApplicationCategoryType</key><string>public.app-category.finance</string>
    <key>LSMinimumSystemVersion</key><string>11.0</string>
    <key>NSHighResolutionCapable</key><true/>
    <key>NSHumanReadableCopyright</key><string>CashCraft contributors. MIT OR Apache-2.0.</string>
</dict>
</plist>
EOF
cp LICENSE-MIT LICENSE-APACHE NOTICE "$APP/Contents/Resources/"
xattr -cr "$APP" 2>/dev/null || true

sign() {
    if [ -n "${APPLE_SIGNING_IDENTITY:-}" ]; then
        codesign --force --options runtime --timestamp --sign "$APPLE_SIGNING_IDENTITY" "$@"
    else
        codesign --force --sign - "$@"
    fi
}
notarize() {
    if [ -n "${APPLE_SIGNING_IDENTITY:-}" ] && [ -n "${APPLE_ID:-}" ] && [ -n "${APPLE_TEAM_ID:-}" ] && [ -n "${APPLE_APP_PASSWORD:-}" ]; then
        echo "→ Notarizing $1"
        xcrun notarytool submit "$1" --apple-id "$APPLE_ID" --team-id "$APPLE_TEAM_ID" \
            --password "$APPLE_APP_PASSWORD" --wait
        xcrun stapler staple "$1"
    fi
}

echo "→ Signing"
sign "$APP/Contents/MacOS/cashcraft-cli"
sign "$APP"
if [ -z "${APPLE_SIGNING_IDENTITY:-}" ]; then
    echo "  warning: signed ad hoc (no APPLE_SIGNING_IDENTITY): fine locally, not for ArtCraft Launcher"
fi

DMG="$DIST/cashcraft-$VERSION-macos-$ARCH.dmg"
echo "→ Creating $DMG"
STAGE="$DIST/dmg-stage"
rm -rf "$STAGE" "$DMG"
mkdir -p "$STAGE"
ditto "$APP" "$STAGE/CashCraft.app"
ln -s /Applications "$STAGE/Applications"
hdiutil create -quiet -volname "CashCraft $VERSION" -srcfolder "$STAGE" -ov -format UDZO "$DMG"
rm -rf "$STAGE"
sign "$DMG"
notarize "$DMG"

ZIP="$DIST/cashcraft-cli-$VERSION-macos-$ARCH.zip"
echo "→ Creating $ZIP"
rm -f "$ZIP"
ditto -c -k --keepParent "$APP/Contents/MacOS/cashcraft-cli" "$ZIP"

echo "✔ $DMG"
echo "✔ $ZIP"

if [ "$INSTALL" = 1 ]; then
    if [ -w /Applications ]; then DEST=/Applications; else DEST="$HOME/Applications"; mkdir -p "$DEST"; fi
    echo "→ Installing into $DEST"
    rm -rf "$DEST/CashCraft.app"
    ditto "$APP" "$DEST/CashCraft.app"
    echo "✔ Open Launchpad and look for CashCraft."
    echo "  Terminal and MCP: \"$DEST/CashCraft.app/Contents/MacOS/cashcraft-cli\" --help"
fi
