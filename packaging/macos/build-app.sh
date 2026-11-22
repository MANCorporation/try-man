#!/bin/bash
# Builds dist/Try MAN.app and dist/Try-MAN-<version>-macos-<arch>.dmg
#
# For Macs with older macOS (recommended for sharing), first build the
# libraries from source, then the app:
#   brew install meson ninja pkg-config adwaita-icon-theme
#   packaging/macos/build-deps.sh          # once; macOS 11 and newer
#   packaging/macos/build-app.sh
#
# Without build-deps.sh, Homebrew's libraries are used instead
# (brew install gtk4 libadwaita adwaita-icon-theme librsvg qemu). The app then
# only runs on the macOS version of the build machine and newer.
#
# Signing: by default the app is signed "ad-hoc" (fine on your own Mac).
# To give it to other people, sign with your Apple Developer ID and notarize:
#   SIGN_IDENTITY="Developer ID Application: Your Name (TEAMID)" packaging/macos/build-app.sh
#   xcrun notarytool submit dist/*.dmg --keychain-profile <profile> --wait
#   xcrun stapler staple dist/*.dmg
set -euo pipefail
cd "$(dirname "$0")/../.."

MACOS_MIN=${MACOS_MIN:-11.0}
DEPS=${DEPS:-build/macos-deps-$MACOS_MIN}
if [ -f "$DEPS/lib/pkgconfig/libadwaita-1.pc" ]; then
    echo "Using libraries built from source in $DEPS (macOS $MACOS_MIN and newer)"
    DEPS=$(cd "$DEPS" && pwd)
    export MACOSX_DEPLOYMENT_TARGET=$MACOS_MIN
    export PKG_CONFIG_LIBDIR=$DEPS/lib/pkgconfig:$DEPS/share/pkgconfig
    unset PKG_CONFIG_PATH
    PREFIX_ARG=(--prefix "$DEPS")
    NEEDED=(adwaita-icon-theme)
else
    echo "Using Homebrew's libraries (run build-deps.sh first to support older macOS)"
    PREFIX_ARG=()
    NEEDED=(gtk4 libadwaita adwaita-icon-theme librsvg qemu)
fi
for pkg in "${NEEDED[@]}"; do
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
    --icon data/app-icon-1024.png --version "$VERSION" --out "$APP" "${PREFIX_ARG[@]}"
rm -f data/app-icon-1024.png

# Sign inside-out: libraries, image loader, QEMU (needs the hypervisor
# entitlement), the app program, then the whole app.
IDENTITY="${SIGN_IDENTITY:--}"
FLAGS=(--force --timestamp=none --sign "$IDENTITY")
if [ "$IDENTITY" != "-" ]; then FLAGS=(--force --timestamp --options runtime --sign "$IDENTITY"); fi
find "$APP/Contents/Frameworks" "$APP/Contents/Resources" -type f \( -name '*.dylib' -o -name '*.so' \) \
    -exec codesign "${FLAGS[@]}" {} \; 2>/dev/null
codesign "${FLAGS[@]}" --entitlements "$ENTITLEMENTS" "$APP/Contents/MacOS/qemu-system-$QEMU_ARCH" 2>/dev/null
codesign "${FLAGS[@]}" "$APP/Contents/MacOS/qemu-img" 2>/dev/null
codesign "${FLAGS[@]}" "$APP/Contents/MacOS/try-man" 2>/dev/null
codesign "${FLAGS[@]}" "$APP" 2>/dev/null
codesign --verify --deep --strict "$APP"

# Disk image with a shortcut to Applications
STAGE=$(mktemp -d)
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
rm -f "$DMG"
hdiutil create -quiet -volname "Try MAN" -srcfolder "$STAGE" -ov -format UDZO "$DMG"
rm -rf "$STAGE"

du -sh "$APP" "$DMG"
