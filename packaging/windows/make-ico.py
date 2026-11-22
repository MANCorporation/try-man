#!/usr/bin/env python3
"""Packs PNG files into a Windows .ico (Windows Vista and newer read PNG icons).

    python3 make-ico.py out.ico icon-256.png icon-128.png ...
"""

import struct
import sys

out, pngs = sys.argv[1], sys.argv[2:]
images = []
for path in pngs:
    data = open(path, "rb").read()
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        sys.exit(f"{path} is not a PNG")
    width, height = struct.unpack(">II", data[16:24])
    images.append((width, height, data))

header = struct.pack("<HHH", 0, 1, len(images))
offset = 6 + 16 * len(images)
entries, blobs = b"", b""
for width, height, data in images:
    # 256 is written as 0 in the directory entry
    entries += struct.pack("<BBBBHHII", width % 256, height % 256, 0, 0, 1, 32, len(data), offset)
    blobs += data
    offset += len(data)

with open(out, "wb") as f:
    f.write(header + entries + blobs)
