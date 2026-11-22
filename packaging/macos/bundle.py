#!/usr/bin/env python3
"""Builds a standalone "Try MAN.app" from Homebrew's GTK, libadwaita and QEMU.

Every non-system library is copied into Contents/Frameworks and every
reference is rewritten to @rpath, so the app runs on Macs without Homebrew.
Run it through build-app.sh.
"""

import argparse
import os
import plistlib
import re
import shutil
import subprocess
import sys
from pathlib import Path

APP_ID = "io.github.mancorporation.TryMan"
SYSTEM_PREFIXES = ("/usr/lib/", "/System/")

# QEMU data files a MAN VM needs (BIOS, option ROMs, keyboard maps).
QEMU_DATA = {
    "aarch64": ["edk2-aarch64-code.fd", "efi-virtio.rom"],
    "x86_64": [
        "bios-256k.bin", "bios.bin", "qboot.rom", "kvmvapic.bin", "linuxboot_dma.bin",
        "multiboot_dma.bin", "pvh.bin", "vgabios.bin", "vgabios-stdvga.bin",
        "vgabios-virtio.bin", "efi-virtio.rom", "pxe-virtio.rom",
    ],
}


def sh(*cmd, capture=False):
    result = subprocess.run(cmd, check=True, text=True, capture_output=capture)
    return result.stdout if capture else None


def is_macho(path: Path) -> bool:
    try:
        with open(path, "rb") as f:
            magic = f.read(4)
    except OSError:
        return False
    return magic in (b"\xcf\xfa\xed\xfe", b"\xca\xfe\xba\xbe", b"\xce\xfa\xed\xfe")


def own_id(path: Path):
    lines = sh("otool", "-D", str(path), capture=True).splitlines()
    return lines[1].strip() if len(lines) > 1 else None


def dependencies(path: Path):
    lines = sh("otool", "-L", str(path), capture=True).splitlines()[1:]
    deps = [line.strip().split(" (")[0] for line in lines if line.strip()]
    me = own_id(path)
    return [d for d in deps if d != me]


def rpaths(path: Path):
    out = sh("otool", "-l", str(path), capture=True)
    return re.findall(r"cmd LC_RPATH\n\s+cmdsize \d+\n\s+path (.+?) \(offset", out)


class Bundler:
    def __init__(self, app: Path, brew: Path):
        self.app = app
        self.brew = brew
        self.frameworks = app / "Contents/Frameworks"
        self.frameworks.mkdir(parents=True, exist_ok=True)
        self.copied = {}  # library file name -> bundled path

    def resolve(self, dep: str, original: Path):
        """Finds the real file behind a library reference."""
        if dep.startswith("@loader_path/"):
            return (original.parent / dep[len("@loader_path/"):]).resolve()
        if dep.startswith("@rpath/"):
            name = dep[len("@rpath/"):]
            for rp in rpaths(original):
                rp = rp.replace("@loader_path", str(original.parent))
                candidate = Path(rp) / name
                if candidate.exists():
                    return candidate.resolve()
            candidate = self.brew / "lib" / name
            return candidate.resolve() if candidate.exists() else None
        path = Path(dep)
        return path.resolve() if path.exists() else None

    def fix(self, bundled: Path, original: Path, rpath: str):
        """Points `bundled` at bundled copies of its libraries."""
        os.chmod(bundled, 0o755)
        for old in rpaths(bundled):
            sh("install_name_tool", "-delete_rpath", old, str(bundled))
        sh("install_name_tool", "-add_rpath", rpath, str(bundled))

        for dep in dependencies(bundled):
            if dep.startswith(SYSTEM_PREFIXES):
                continue
            name = Path(dep).name
            if name not in self.copied:
                real = self.resolve(dep, original)
                if real is None:
                    sys.exit(f"error: can't find {dep} (needed by {original})")
                dest = self.frameworks / name
                shutil.copy2(real, dest)
                os.chmod(dest, 0o755)
                self.copied[name] = dest
                sh("install_name_tool", "-id", f"@rpath/{name}", str(dest))
                self.fix(dest, real, "@loader_path")
            sh("install_name_tool", "-change", dep, f"@rpath/{name}", str(bundled))
        # Editing a Mach-O breaks its signature; Apple Silicon refuses to load it unsigned.
        sh("codesign", "--force", "--sign", "-", str(bundled), capture=True)

    def add(self, source: Path, dest: Path, rpath: str):
        dest.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source.resolve(), dest)
        self.fix(dest, source.resolve(), rpath)


def make_icns(png: Path, out: Path, tmp: Path):
    iconset = tmp / "AppIcon.iconset"
    shutil.rmtree(iconset, ignore_errors=True)
    iconset.mkdir(parents=True)
    for size in (16, 32, 128, 256, 512):
        for scale in (1, 2):
            px = size * scale
            name = f"icon_{size}x{size}{'@2x' if scale == 2 else ''}.png"
            sh("sips", "-z", str(px), str(px), str(png), "--out", str(iconset / name), capture=True)
    sh("iconutil", "-c", "icns", str(iconset), "-o", str(out))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--binary", required=True)
    ap.add_argument("--qemu-arch", required=True, choices=["aarch64", "x86_64"])
    ap.add_argument("--icon", required=True, help="1024x1024 PNG")
    ap.add_argument("--version", required=True)
    ap.add_argument("--out", required=True)
    args = ap.parse_args()

    brew = Path(sh("brew", "--prefix", capture=True).strip())
    app = Path(args.out)
    shutil.rmtree(app, ignore_errors=True)
    contents = app / "Contents"
    macos = contents / "MacOS"
    res = contents / "Resources"
    b = Bundler(app, brew)

    # Programs: the app and the QEMU it uses, side by side.
    to_frameworks = "@executable_path/../Frameworks"
    b.add(Path(args.binary), macos / "try-man", to_frameworks)
    b.add(brew / f"bin/qemu-system-{args.qemu_arch}", macos / f"qemu-system-{args.qemu_arch}", to_frameworks)
    b.add(brew / "bin/qemu-img", macos / "qemu-img", to_frameworks)

    # QEMU firmware and keyboard maps
    qemu_src = (brew / "share/qemu").resolve()
    qemu_dst = res / "qemu/share/qemu"
    qemu_dst.mkdir(parents=True)
    shutil.copytree(qemu_src / "keymaps", qemu_dst / "keymaps")
    for name in QEMU_DATA[args.qemu_arch] + ["edk2-licenses.txt"]:
        if (qemu_src / name).exists():
            shutil.copy2(qemu_src / name, qemu_dst / name)

    # SVG image loader (GTK's symbolic icons are SVG)
    loaders_rel = Path("lib/gdk-pixbuf-2.0/2.10.0")
    loader_src = brew / loaders_rel / "loaders/libpixbufloader_svg.so"
    loader_dst = res / loaders_rel / "loaders/libpixbufloader_svg.so"
    b.add(loader_src, loader_dst, "@loader_path/../../../../../Frameworks")
    cache = sh(str(brew / "bin/gdk-pixbuf-query-loaders"), str(loader_dst.resolve()), capture=True)
    cache = "\n".join(l for l in cache.splitlines() if not l.startswith("#"))
    if "svg" not in cache:
        sys.exit("error: the SVG image loader didn't load")
    (res / loaders_rel / "loaders.cache.in").write_text(cache.replace(str(res.resolve()), "@RESOURCES@") + "\n")

    # GTK settings schemas
    schemas = res / "share/glib-2.0/schemas"
    schemas.mkdir(parents=True)
    for xml in (brew / "share/glib-2.0/schemas").glob("org.gtk.gtk4.*.gschema.xml"):
        shutil.copy2(xml, schemas)
    sh(str(brew / "bin/glib-compile-schemas"), str(schemas))

    # Icon themes
    icons = res / "share/icons"
    shutil.copytree((brew / "share/icons/Adwaita").resolve(), icons / "Adwaita", symlinks=False)
    (icons / "hicolor").mkdir(parents=True)
    shutil.copy2((brew / "share/icons/hicolor/index.theme").resolve(), icons / "hicolor/index.theme")
    for f in icons.rglob("icon-theme.cache"):
        f.unlink()

    # Fonts: if fontconfig is used, point it at the Mac's own fonts.
    if (b.frameworks / "libfontconfig.1.dylib").exists():
        fonts = res / "etc/fonts"
        fonts.mkdir(parents=True)
        (fonts / "fonts.conf").write_text(
            '<?xml version="1.0"?>\n<!DOCTYPE fontconfig SYSTEM "fonts.dtd">\n<fontconfig>\n'
            "  <dir>/System/Library/Fonts</dir>\n  <dir>/Library/Fonts</dir>\n"
            "  <dir>~/Library/Fonts</dir>\n  <cachedir>~/Library/Caches/try-man-fontconfig</cachedir>\n"
            "</fontconfig>\n"
        )

    make_icns(Path(args.icon), res / "AppIcon.icns", Path(args.out).parent / ".icon-tmp")
    shutil.rmtree(Path(args.out).parent / ".icon-tmp", ignore_errors=True)

    with open(contents / "Info.plist", "wb") as f:
        plistlib.dump({
            "CFBundleName": "Try MAN",
            "CFBundleDisplayName": "Try MAN",
            "CFBundleIdentifier": APP_ID,
            "CFBundleExecutable": "try-man",
            "CFBundleIconFile": "AppIcon",
            "CFBundlePackageType": "APPL",
            "CFBundleShortVersionString": args.version,
            "CFBundleVersion": args.version,
            "LSMinimumSystemVersion": "12.0",
            "LSApplicationCategoryType": "public.app-category.utilities",
            "NSHighResolutionCapable": True,
        }, f)

    # Make sure nothing still points into Homebrew.
    leftovers = []
    for path in app.rglob("*"):
        if path.is_file() and is_macho(path):
            refs = dependencies(path) + rpaths(path)
            leftovers += [f"{path.relative_to(app)} -> {r}" for r in refs if str(brew) in r or "/usr/local/" in r]
    if leftovers:
        sys.exit("error: still linked to Homebrew:\n  " + "\n  ".join(leftovers))
    print(f"bundled {len(b.copied)} libraries into {app}")


if __name__ == "__main__":
    main()
