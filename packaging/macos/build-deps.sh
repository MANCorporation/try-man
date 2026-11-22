#!/bin/bash
# Builds GTK 4, libadwaita and QEMU (and the libraries they need) from source
# for an older macOS, so Try MAN.app runs on that macOS and newer.
# (Homebrew's ready-made libraries only run on the macOS they were built for.)
#
#   packaging/macos/build-deps.sh            # macOS 11 Big Sur and newer
#   MACOS_MIN=12.0 packaging/macos/build-deps.sh
#
# Result: build/macos-deps-<version>/ (used by build-app.sh). Takes a while.
# Needs (build tools only): brew install meson ninja pkg-config
set -euo pipefail
cd "$(dirname "$0")/../.."

MACOS_MIN=${MACOS_MIN:-11.0}
GTK=4.24.1
ADWAITA=1.10.0
SLIRP=4.9.1
QEMU=11.1.0

export MACOSX_DEPLOYMENT_TARGET=$MACOS_MIN
SRC=$PWD/build/macos-src
PREFIX=$PWD/build/macos-deps-$MACOS_MIN
BUILD=$PWD/build/macos-build-$MACOS_MIN
mkdir -p "$SRC" "$PREFIX" "$BUILD"

# Only libraries we build ourselves; never Homebrew's.
export PKG_CONFIG_LIBDIR=$PREFIX/lib/pkgconfig:$PREFIX/share/pkgconfig
unset PKG_CONFIG_PATH CPATH LIBRARY_PATH
export PATH="$PREFIX/bin:$PATH"

fetch() { # url
    local file=$SRC/$(basename "$1")
    [ -f "$file" ] || curl -fL --retry 3 -o "$file" "$1"
    tar -xf "$file" -C "$SRC"
}

# --- GTK and everything below it (glib, cairo, pango, harfbuzz, ...) ---------
if [ ! -f "$PREFIX/lib/pkgconfig/gtk4.pc" ]; then
    fetch "https://download.gnome.org/sources/gtk/${GTK%.*}/gtk-$GTK.tar.xz"
    meson setup "$BUILD/gtk" "$SRC/gtk-$GTK" --prefix="$PREFIX" --libdir=lib \
        --buildtype=release --wrap-mode=forcefallback \
        -Dmacos-backend=true -Dx11-backend=false -Dwayland-backend=false \
        -Dmedia-gstreamer=disabled -Dprint-cups=disabled -Dvulkan=disabled \
        -Dcloudproviders=disabled -Dsysprof=disabled -Daccesskit=disabled \
        -Dintrospection=disabled -Ddocumentation=false -Dman-pages=false \
        -Dbuild-demos=false -Dbuild-examples=false -Dbuild-tests=false -Dbuild-testsuite=false \
        -Dglib:tests=false -Dglib:introspection=disabled \
        -Dcairo:tests=disabled \
        -Dharfbuzz:tests=disabled -Dharfbuzz:docs=disabled \
        -Dpango:introspection=disabled \
        -Dgdk-pixbuf:tests=false -Dgdk-pixbuf:introspection=disabled -Dgdk-pixbuf:man=false \
        -Dgdk-pixbuf:others=disabled -Dgdk-pixbuf:tiff=disabled \
        -Dgraphene:tests=false -Dgraphene:introspection=disabled
    meson install -C "$BUILD/gtk"
fi

# --- libadwaita ---------------------------------------------------------------
if [ ! -f "$PREFIX/lib/pkgconfig/libadwaita-1.pc" ]; then
    fetch "https://download.gnome.org/sources/libadwaita/${ADWAITA%.*}/libadwaita-$ADWAITA.tar.xz"
    meson setup "$BUILD/libadwaita" "$SRC/libadwaita-$ADWAITA" --prefix="$PREFIX" --libdir=lib \
        --buildtype=release -Dintrospection=disabled -Dvapi=false -Ddocumentation=false \
        -Dexamples=false -Dtests=false
    meson install -C "$BUILD/libadwaita"
fi

# --- libslirp (QEMU's network) --------------------------------------------------
if [ ! -f "$PREFIX/lib/pkgconfig/slirp.pc" ]; then
    fetch "https://gitlab.freedesktop.org/slirp/libslirp/-/archive/v$SLIRP/libslirp-v$SLIRP.tar.gz"
    meson setup "$BUILD/slirp" "$SRC/libslirp-v$SLIRP" --prefix="$PREFIX" --libdir=lib --buildtype=release
    meson install -C "$BUILD/slirp"
fi

# --- QEMU (only what Try MAN uses) -----------------------------------------------
case "$(uname -m)" in arm64) TARGET=aarch64-softmmu ;; *) TARGET=x86_64-softmmu ;; esac
if [ ! -x "$PREFIX/bin/qemu-img" ]; then
    fetch "https://download.qemu.org/qemu-$QEMU.tar.xz"
    mkdir -p "$BUILD/qemu"
    (cd "$BUILD/qemu" && "$SRC/qemu-$QEMU/configure" --prefix="$PREFIX" \
        --target-list=$TARGET --enable-hvf --enable-slirp --enable-tools \
        --extra-cflags="-mmacosx-version-min=$MACOS_MIN" \
        --extra-ldflags="-mmacosx-version-min=$MACOS_MIN" \
        --disable-docs --disable-cocoa --disable-sdl --disable-gtk --disable-opengl \
        --disable-curses --disable-spice --disable-gnutls --disable-nettle --disable-gcrypt \
        --disable-curl --disable-libssh --disable-libusb --disable-usb-redir --disable-capstone \
        --disable-png --disable-vnc-jpeg --disable-vnc-sasl --disable-snappy --disable-lzo \
        --disable-zstd --disable-dmg --disable-guest-agent --disable-coreaudio --disable-vmnet \
        --disable-pvg)  # Apple paravirtualized graphics: newer macOS only, not used
    make -C "$BUILD/qemu" -j"$(sysctl -n hw.ncpu)"
    make -C "$BUILD/qemu" install
fi

echo "Done: $PREFIX (for macOS $MACOS_MIN and newer)"
