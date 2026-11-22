#!/bin/bash
# Builds the Windows installer (dist/Try-MAN-<version>-windows-x64-setup.exe)
# and a portable zip, with GTK, libadwaita and QEMU inside.
#
# Run in an "MSYS2 UCRT64" shell on Windows (https://www.msys2.org):
#   packaging/windows/build-windows.sh --install-deps
#   packaging/windows/build-windows.sh
set -euo pipefail
cd "$(dirname "$0")/../.."

P=/ucrt64
if [ "${1:-}" = "--install-deps" ]; then
    pacman -S --needed --noconfirm zip \
        mingw-w64-ucrt-x86_64-{rust,gcc,pkgconf,gtk4,libadwaita,librsvg,adwaita-icon-theme,qemu,nsis,imagemagick}
    exit 0
fi

VERSION=$(grep -m1 '^version' Cargo.toml | sed -E 's/.*"([0-9]+).*/\1/')
DIST=build/windows/TryMAN
LOADERS=lib/gdk-pixbuf-2.0/2.10.0

cargo build --release

rm -rf "$DIST"
mkdir -p "$DIST/bin" "$DIST/$LOADERS/loaders" "$DIST/share/glib-2.0/schemas" \
    "$DIST/share/icons/hicolor" "$DIST/share/qemu" dist

# Programs: the app and QEMU side by side
cp target/release/try-man.exe "$DIST/bin/"
cp "$P/bin/qemu-system-x86_64.exe" "$P/bin/qemu-img.exe" "$DIST/bin/"

# SVG image loader (GTK's symbolic icons are SVG)
cp "$P/$LOADERS"/loaders/*svg*.dll "$DIST/$LOADERS/loaders/"
"$P/bin/gdk-pixbuf-query-loaders.exe" "$P/$LOADERS"/loaders/*svg*.dll \
    | grep -v '^#' \
    | sed -E 's#^"[^"]*[/\\]+lib[/\\]+gdk-pixbuf-2\.0[/\\]+2\.10\.0[/\\]+loaders[/\\]+#"@RESOURCES@/lib/gdk-pixbuf-2.0/2.10.0/loaders/#' \
    > "$DIST/$LOADERS/loaders.cache.in"
grep -q '@RESOURCES@' "$DIST/$LOADERS/loaders.cache.in" || { echo "loader cache failed"; exit 1; }

# Every DLL those programs need, from MSYS2
for f in "$DIST"/bin/*.exe "$DIST/$LOADERS"/loaders/*.dll; do
    ldd "$f" | awk '{print $3}' | grep -i "^$P/bin/" || true
done | sort -u | while read -r dll; do cp -n "$dll" "$DIST/bin/"; done

# GTK settings schemas, icons
cp "$P"/share/glib-2.0/schemas/org.gtk.gtk4.*.gschema.xml "$DIST/share/glib-2.0/schemas/"
glib-compile-schemas "$DIST/share/glib-2.0/schemas"
cp -r "$P/share/icons/Adwaita" "$DIST/share/icons/"
cp "$P/share/icons/hicolor/index.theme" "$DIST/share/icons/hicolor/"
find "$DIST/share/icons" -name icon-theme.cache -delete

# QEMU BIOS, option ROMs and keyboard maps
cp -r "$P/share/qemu/keymaps" "$DIST/share/qemu/"
for f in bios-256k.bin bios.bin qboot.rom kvmvapic.bin linuxboot_dma.bin multiboot_dma.bin pvh.bin \
         vgabios.bin vgabios-stdvga.bin vgabios-virtio.bin efi-virtio.rom pxe-virtio.rom; do
    [ -e "$P/share/qemu/$f" ] && cp "$P/share/qemu/$f" "$DIST/share/qemu/"
done

# Icon for shortcuts and the installer
magick data/icons/hicolor/512x512/apps/io.github.mancorporation.TryMan.png \
    -define icon:auto-resize=256,128,64,48,32,16 "$DIST/try-man.ico"

# Portable zip
rm -f "dist/Try-MAN-$VERSION-windows-x64.zip"
(cd build/windows && zip -qr "../../dist/Try-MAN-$VERSION-windows-x64.zip" TryMAN)

# Installer
makensis -V2 -DVERSION="$VERSION" -DSRCDIR="$(cygpath -w "$DIST")" \
    -DOUTFILE="$(cygpath -w "$PWD/dist/Try-MAN-$VERSION-windows-x64-setup.exe")" \
    packaging/windows/installer.nsi
ls -lh dist/Try-MAN-"$VERSION"-windows-*
