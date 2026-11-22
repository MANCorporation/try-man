#!/bin/bash
# Builds the Windows installer (dist/Try-MAN-<version>-windows-x64-setup.exe)
# and a portable zip, with GTK, libadwaita and QEMU inside.
#
# Works two ways:
#
#  * On Windows, in an "MSYS2 UCRT64" shell (https://www.msys2.org):
#      packaging/windows/build-windows.sh --install-deps
#      packaging/windows/build-windows.sh
#
#  * Cross-built on macOS or Linux. The Windows libraries are downloaded from
#    MSYS2 by fetch-msys2.py. Needs: rustup target add x86_64-pc-windows-gnu,
#    a mingw-w64 compiler (brew install mingw-w64), makensis, glib-compile-schemas,
#    Python 3.14+.
#      packaging/windows/build-windows.sh
set -euo pipefail
cd "$(dirname "$0")/../.."

case "$(uname -s)" in
    MINGW*|MSYS*) NATIVE=1 ;;
    *) NATIVE=0 ;;
esac

if [ "${1:-}" = "--install-deps" ]; then
    [ $NATIVE = 1 ] || { echo "--install-deps is for MSYS2 on Windows"; exit 1; }
    pacman -S --needed --noconfirm zip \
        mingw-w64-ucrt-x86_64-{rust,gcc,pkgconf,gtk4,libadwaita,librsvg,adwaita-icon-theme,qemu,nsis,python}
    exit 0
fi

VERSION=$(grep -m1 '^version' Cargo.toml | sed -E 's/.*"([0-9]+).*/\1/')
DIST=build/windows/TryMAN
LOADERS=lib/gdk-pixbuf-2.0/2.10.0

if [ $NATIVE = 1 ]; then
    P=/ucrt64
    OBJDUMP=objdump
    cargo build --release
    EXE=target/release/try-man.exe
else
    python3 packaging/windows/fetch-msys2.py build/msys2
    P=$PWD/build/msys2/root/ucrt64
    OBJDUMP=x86_64-w64-mingw32-objdump
    PKG_CONFIG_ALLOW_CROSS=1 \
    PKG_CONFIG_SYSROOT_DIR="$PWD/build/msys2/root" \
    PKG_CONFIG_LIBDIR="$P/lib/pkgconfig:$P/share/pkgconfig" \
        cargo build --release --target x86_64-pc-windows-gnu
    EXE=target/x86_64-pc-windows-gnu/release/try-man.exe
fi

rm -rf "$DIST"
mkdir -p "$DIST/bin" "$DIST/$LOADERS/loaders" "$DIST/share/glib-2.0/schemas" \
    "$DIST/share/icons/hicolor" "$DIST/share/qemu" dist

# Programs: the app and QEMU side by side
cp "$EXE" "$DIST/bin/"
cp "$P/bin/qemu-system-x86_64.exe" "$P/bin/qemu-img.exe" "$DIST/bin/"

# Image loaders: SVG (GTK's symbolic icons) and PNG
cp "$P/$LOADERS"/loaders/*svg*.dll "$P/$LOADERS/loaders/libpixbufloader-png.dll" "$DIST/$LOADERS/loaders/"
SVG_LOADER=$(basename "$(ls "$DIST/$LOADERS"/loaders/*svg*.dll)")
# The app fills in @RESOURCES@ with its real folder at startup (src/main.rs).
cat > "$DIST/$LOADERS/loaders.cache.in" <<EOF
"@RESOURCES@/$LOADERS/loaders/libpixbufloader-png.dll"
"png" 5 "gdk-pixbuf" "PNG" "LGPL"
"image/png" ""
"png" ""
"\211PNG\r\n\032\n" "" 100

"@RESOURCES@/$LOADERS/loaders/$SVG_LOADER"
"svg" 6 "gdk-pixbuf" "Scalable Vector Graphics" "LGPL"
"image/svg+xml" "image/svg" "image/svg-xml" "image/vnd.adobe.svg+xml" "text/xml-svg" "image/svg+xml-compressed" ""
"svg" "svgz" "svg.gz" ""
" <svg" "*    " 100
" <!DOCTYPE svg" "*             " 100

EOF

# Every DLL those programs need (and the DLLs those need...), from MSYS2.
# System DLLs (KERNEL32, api-ms-win-*...) aren't in MSYS2's bin and are skipped.
queue=("$DIST"/bin/*.exe "$DIST/$LOADERS"/loaders/*.dll)
while [ ${#queue[@]} -gt 0 ]; do
    file=${queue[0]}; queue=("${queue[@]:1}")
    for dll in $("$OBJDUMP" -p "$file" | sed -n 's/.*DLL Name: //p' | tr -d '\r'); do
        if [ -f "$P/bin/$dll" ] && [ ! -f "$DIST/bin/$dll" ]; then
            cp "$P/bin/$dll" "$DIST/bin/"
            queue+=("$DIST/bin/$dll")
        fi
    done
done

python3 packaging/windows/check-dlls.py "$DIST" "$OBJDUMP"

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

# Icon for the shortcuts and the installer (PNG images inside an .ico)
python3 packaging/windows/make-ico.py "$DIST/try-man.ico" \
    data/icons/hicolor/{256x256,128x128,64x64,48x48}/apps/io.github.mancorporation.TryMan.png

# Portable zip
rm -f "dist/Try-MAN-$VERSION-windows-x64.zip"
(cd build/windows && zip -qr "../../dist/Try-MAN-$VERSION-windows-x64.zip" TryMAN)

# Installer
if [ $NATIVE = 1 ]; then
    SRC=$(cygpath -w "$DIST"); OUTFILE=$(cygpath -w "$PWD/dist/Try-MAN-$VERSION-windows-x64-setup.exe")
else
    SRC=$PWD/$DIST; OUTFILE=$PWD/dist/Try-MAN-$VERSION-windows-x64-setup.exe
fi
# makensis crashes (bad_alloc) when no locale is set
LC_ALL=C.UTF-8 LANG=C.UTF-8 makensis -V2 -DVERSION="$VERSION" -DSRCDIR="$SRC" -DOUTFILE="$OUTFILE" packaging/windows/installer.nsi
ls -lh dist/Try-MAN-"$VERSION"-windows-*
