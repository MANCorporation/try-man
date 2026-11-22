#!/bin/bash
# Builds dist/Try-MAN-<version>-<arch>.AppImage with GTK, libadwaita and QEMU inside.
#
# Made for Ubuntu 24.04 (x86_64 or arm64), e.g. the GitHub Actions runners:
#   sudo packaging/linux/build-appimage.sh --install-deps
#   packaging/linux/build-appimage.sh
set -euo pipefail
cd "$(dirname "$0")/../.."

ARCH=$(uname -m)                       # x86_64 or aarch64
MULTIARCH=$(gcc -dumpmachine 2>/dev/null || echo "$ARCH-linux-gnu")

if [ "${1:-}" = "--install-deps" ]; then
    apt-get update
    QEMU_PKGS="qemu-system-x86 seabios ipxe-qemu"
    [ "$ARCH" = aarch64 ] && QEMU_PKGS="qemu-system-arm qemu-efi-aarch64 ipxe-qemu"
    DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
        build-essential pkg-config ca-certificates curl file patchelf desktop-file-utils squashfs-tools \
        libgtk-4-dev libadwaita-1-dev librsvg2-common adwaita-icon-theme shared-mime-info \
        qemu-utils qemu-system-gui qemu-system-modules-opengl $QEMU_PKGS
    exit 0
fi

VERSION=$(grep -m1 '^version' Cargo.toml | sed -E 's/.*"([0-9]+).*/\1/')
QEMU_SYSTEM=qemu-system-x86_64
[ "$ARCH" = aarch64 ] && QEMU_SYSTEM=qemu-system-aarch64
APPDIR=$PWD/build/AppDir
TOOLS=$PWD/build/tools
OUT=$PWD/dist/Try-MAN-$VERSION-$ARCH.AppImage
export APPIMAGE_EXTRACT_AND_RUN=1      # no FUSE needed (CI, containers)

cargo build --release

rm -rf "$APPDIR"
mkdir -p "$TOOLS" dist "$APPDIR/usr/share/qemu" "$APPDIR/usr/lib/qemu" "$APPDIR/usr/share/icons"

# linuxdeploy + its GTK plugin
LD=$TOOLS/linuxdeploy-$ARCH.AppImage
[ -x "$LD" ] || { curl -fsSL -o "$LD" \
    "https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-$ARCH.AppImage"; chmod +x "$LD"; }
GTK_PLUGIN=$TOOLS/linuxdeploy-plugin-gtk.sh
[ -x "$GTK_PLUGIN" ] || { curl -fsSL -o "$GTK_PLUGIN" \
    "https://raw.githubusercontent.com/linuxdeploy/linuxdeploy-plugin-gtk/master/linuxdeploy-plugin-gtk.sh"; chmod +x "$GTK_PLUGIN"; }
export PATH="$TOOLS:$PATH"

# QEMU firmware, BIOS and keyboard maps (Debian keeps many as symlinks)
for f in /usr/share/qemu/*; do
    [ -e "$f" ] && cp -rL "$f" "$APPDIR/usr/share/qemu/"
done
if [ "$ARCH" = aarch64 ]; then
    cp -L /usr/share/AAVMF/AAVMF_CODE.fd "$APPDIR/usr/share/qemu/edk2-aarch64-code.fd"
fi

# QEMU modules: virtual graphics cards, 3D (virgl) and the software CPU.
# Sound, SPICE, network storage and other windows aren't needed.
MODULES=()
for m in /usr/lib/$MULTIARCH/qemu/*.so; do
    case "$(basename "$m")" in
        accel-*|hw-display-virtio-*|ui-egl-headless.so|ui-opengl.so)
            cp -L "$m" "$APPDIR/usr/lib/qemu/"
            MODULES+=(--deploy-deps-only "$APPDIR/usr/lib/qemu/$(basename "$m")") ;;
    esac
done

# Our icon theme
cp -rL /usr/share/icons/Adwaita "$APPDIR/usr/share/icons/"
# File-type database: GTK uses it to recognise the SVG icons. Desktops have
# it, minimal systems may not. Only the compiled files are needed.
mkdir -p "$APPDIR/usr/share/mime"
find /usr/share/mime -maxdepth 1 -type f -exec cp {} "$APPDIR/usr/share/mime/" \;
mkdir -p "$APPDIR/usr/share/icons/hicolor"
cp -L /usr/share/icons/hicolor/index.theme "$APPDIR/usr/share/icons/hicolor/"

export DEPLOY_GTK_VERSION=4
"$LD" --appdir "$APPDIR" \
    --executable target/release/try-man \
    --executable "/usr/bin/$QEMU_SYSTEM" \
    --executable /usr/bin/qemu-img \
    "${MODULES[@]}" \
    --desktop-file data/io.github.mancorporation.TryMan.desktop \
    --icon-file data/icons/hicolor/256x256/apps/io.github.mancorporation.TryMan.png \
    --plugin gtk

# Modules live in usr/lib/qemu; their libraries are in usr/lib.
for m in "$APPDIR"/usr/lib/qemu/*.so; do
    patchelf --set-rpath '$ORIGIN/..' "$m"
done
# The image loader list from linuxdeploy has bare file names, which GTK looks
# up in the *system* folder. The app writes one with real paths at startup
# from this template (see setup_bundle in src/main.rs).
LOADERS=$APPDIR/usr/lib/gdk-pixbuf-2.0/2.10.0
# The loaders must find their libraries (librsvg...) in usr/lib.
for l in "$LOADERS"/loaders/*.so; do
    patchelf --set-rpath '$ORIGIN/../../..' "$l"
done
"/usr/lib/$MULTIARCH/gdk-pixbuf-2.0/gdk-pixbuf-query-loaders" "$LOADERS"/loaders/*.so \
    | grep -v '^#' | sed "s#$APPDIR/usr#@RESOURCES@#" > "$LOADERS/loaders.cache.in"
grep -q '@RESOURCES@.*svg' "$LOADERS/loaders.cache.in" || { echo "SVG image loader missing"; exit 1; }

for size in 48 64 128 256 512; do
    install -Dm644 "data/icons/hicolor/${size}x${size}/apps/io.github.mancorporation.TryMan.png" \
        "$APPDIR/usr/share/icons/hicolor/${size}x${size}/apps/io.github.mancorporation.TryMan.png"
done

rm -f "$OUT"
LDAI_OUTPUT="$OUT" "$LD" --appdir "$APPDIR" --output appimage
ls -lh "$OUT"
