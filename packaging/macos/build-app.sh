#!/bin/bash
# Builds dist/Try MAN.app and dist/Try-MAN-<version>-macos-<arch>.dmg
#
# Needs: brew install gtk4 libadwaita adwaita-icon-theme librsvg qemu
#
# Signing: by default the app is signed "ad-hoc" (fine on your own Mac).
# To give it to other people, sign with your Apple Developer ID and notarize:
#   SIGN_IDENTITY="Developer ID Application: Your Name (TEAMID)" packaging/macos/build-app.sh
#   xcrun notarytool submit dist/*.dmg --keychain-profile <profile> --wait
#   xcrun stapler staple dist/*.dmg
set -euo pipefail
cd "$(dirname "$0")/../.."

for pkg in gtk4 libadwaita adwaita-icon-theme librsvg qemu; do
    brew list --versions "$pkg" >/dev/null || { echo "Please run: brew install $pkg"; exit 1; }
done

VERSION=$(grep -m1 '^version' Cargo.toml | sed -E 's/.*"([0-9]+).*/\1/')
case "$(uname -m)" in
    arm64) QEMU_ARCH=aarch64; ARCH=arm64 ;;
    x86_64) QEMU_ARCH=x86_64; ARCH=x86_64 ;;
esac
APP="dist/Try MAN.app"
DMG="dist/Try-MAN-$VERSION-macos-$ARCH.dmg"
ENTITLEMENTS=packaging/macos/qemu.entitlements

cargo build --release
cargo test --release export_app_icon -- --ignored --quiet >/dev/null
mkdir -p dist
python3 packaging/macos/bundle.py --binary target/release/try-man --qemu-arch "$QEMU_ARCH" \
    --icon data/app-icon-1024.png --version "$VERSION" --out "$APP"
rm -f data/app-icon-1024.png

# Sign inside-out: libraries, image loader, QEMU (needs the hypervisor
# entitlement), the app program, then the whole app.
IDENTITY="${SIGN_IDENTITY:--}"
FLAGS=(--force --timestamp=none --sign "$IDENTITY")
if [ "$IDENTITY" != "-" ]; then FLAGS=(--force --timestamp --options runtime --sign "$IDENTITY"); fi
find "$APP/Contents/Frameworks" "$APP/Contents/Resources" -type f \( -name '*.dylib' -o -name '*.so' \) \
    -exec codesign "${FLAGS[@]}" {} \;
codesign "${FLAGS[@]}" --entitlements "$ENTITLEMENTS" "$APP/Contents/MacOS/qemu-system-$QEMU_ARCH"
codesign "${FLAGS[@]}" "$APP/Contents/MacOS/qemu-img"
codesign "${FLAGS[@]}" "$APP/Contents/MacOS/try-man"
codesign "${FLAGS[@]}" "$APP"
codesign --verify --deep --strict "$APP"

# Disk image with a shortcut to Applications
STAGE=$(mktemp -d)
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
rm -f "$DMG"
hdiutil create -quiet -volname "Try MAN" -srcfolder "$STAGE" -ov -format UDZO "$DMG"
rm -rf "$STAGE"

du -sh "$APP" "$DMG"
