#!/usr/bin/env python3
"""Downloads the Windows (MSYS2 UCRT64) builds of GTK4, libadwaita, QEMU and
everything they need, for cross-building Try MAN for Windows on macOS/Linux.

Packages come from the official MSYS2 server; each file is checked against
the SHA-256 in the package database before it is unpacked.

    python3 packaging/windows/fetch-msys2.py build/msys2
"""

import hashlib
import io
import re
import sys
import tarfile
import urllib.request
from pathlib import Path

try:
    from compression import zstd  # Python 3.14+
except ImportError:
    sys.exit("Python 3.14 or newer is needed (for zstd).")

REPO = "https://repo.msys2.org/mingw/ucrt64"
PREFIX = "mingw-w64-ucrt-x86_64-"
WANTED = ["gtk4", "libadwaita", "qemu", "librsvg", "adwaita-icon-theme", "gdk-pixbuf2",
          "ncurses"]  # QEMU needs ncurses but its package forgets to say so
# Not needed to build or run Try MAN; keeps the download small.
SKIP_DEPS = {"python", "gcc-libgfortran", "tcl", "tk", "perl", "ca-certificates"}
SKIP_PATHS = re.compile(r"^ucrt64/(include|share/(doc|man|info|gtk-doc|locale|help|gir-1\.0|vala|installed-tests)|lib/.*\.a$(?<!\.dll\.a$)|lib/cmake|lib/python)")


def fetch(url: str) -> bytes:
    req = urllib.request.Request(url, headers={"User-Agent": "try-man-build"})
    with urllib.request.urlopen(req, timeout=120) as r:
        return r.read()


def read_db():
    data = zstd.decompress(fetch(f"{REPO}/ucrt64.db"))
    packages, provides = {}, {}
    with tarfile.open(fileobj=io.BytesIO(data)) as tar:
        for member in tar.getmembers():
            if not member.name.endswith("/desc"):
                continue
            fields, key = {}, None
            for line in tar.extractfile(member).read().decode().splitlines():
                if line.startswith("%") and line.endswith("%"):
                    key = line.strip("%")
                    fields[key] = []
                elif line and key:
                    fields[key].append(line)
            name = fields["NAME"][0]
            packages[name] = fields
            for p in fields.get("PROVIDES", []):
                provides[re.split(r"[<>=]", p)[0]] = name
    return packages, provides


def resolve(packages, provides):
    todo = [PREFIX + w for w in WANTED]
    found = {}
    while todo:
        name = re.split(r"[<>=]", todo.pop())[0]
        name = name if name in packages else provides.get(name, name)
        if name in found or name not in packages:
            continue
        if name.removeprefix(PREFIX) in SKIP_DEPS:
            continue
        found[name] = packages[name]
        todo.extend(packages[name].get("DEPENDS", []))
    return found


def main():
    dest = Path(sys.argv[1] if len(sys.argv) > 1 else "build/msys2")
    cache = dest.parent / "msys2-cache"
    cache.mkdir(parents=True, exist_ok=True)
    root = dest / "root"
    root.mkdir(parents=True, exist_ok=True)

    packages, provides = read_db()
    needed = resolve(packages, provides)
    print(f"{len(needed)} packages")
    for name, fields in sorted(needed.items()):
        filename = fields["FILENAME"][0]
        path = cache / filename
        if not path.exists():
            path.write_bytes(fetch(f"{REPO}/{filename}"))
        digest = hashlib.sha256(path.read_bytes()).hexdigest()
        if digest != fields["SHA256SUM"][0]:
            path.unlink()
            sys.exit(f"checksum mismatch for {filename}")
        stamp = root / ".installed" / filename
        if stamp.exists():
            continue
        raw = zstd.decompress(path.read_bytes()) if filename.endswith(".zst") else path.read_bytes()
        with tarfile.open(fileobj=io.BytesIO(raw)) as tar:
            members = [m for m in tar.getmembers()
                       if m.name.startswith("ucrt64/") and not SKIP_PATHS.match(m.name)]
            tar.extractall(root, members=members, filter="data")
        stamp.parent.mkdir(parents=True, exist_ok=True)
        stamp.touch()
    print(f"unpacked into {root}/ucrt64")


if __name__ == "__main__":
    main()
