#!/usr/bin/env python3
"""Checks that every DLL the Windows package needs is either inside it or
part of Windows itself. Exits with an error listing anything missing.

    python3 check-dlls.py build/windows/TryMAN [objdump]
"""

import re
import subprocess
import sys
from pathlib import Path

# DLLs that come with Windows 10/11.
WINDOWS = {
    "advapi32", "avrt", "bcrypt", "bcryptprimitives", "cfgmgr32", "comctl32", "comdlg32",
    "credui", "crypt32", "d2d1", "d3d9", "d3d11", "d3d12", "d3dcompiler_47", "dbghelp",
    "dcomp", "dinput8", "dnsapi", "dwmapi", "dwrite", "dxgi", "dxva2", "gdi32", "gdiplus",
    "glu32", "hid", "imm32", "iphlpapi", "kernel32", "kernelbase", "ksuser", "mf", "mfplat",
    "mfreadwrite", "mfuuid", "mmdevapi", "mpr", "msimg32", "msvcrt", "mswsock", "ncrypt",
    "netapi32", "normaliz", "ntdll", "ole32", "oleacc", "oleaut32", "opengl32", "powrprof",
    "propsys", "psapi", "rpcrt4", "secur32", "setupapi", "shcore", "shell32", "shlwapi",
    "sspicli", "ucrtbase", "uiautomationcore", "user32", "userenv", "usp10", "uxtheme",
    "version", "virtdisk", "winhttp", "wininet", "winmm", "winspool", "wintrust", "winusb",
    "wldap32", "ws2_32", "wsock32", "wtsapi32", "xinput1_4", "winhvplatform", "winhvemulation",
}


def is_windows_dll(name: str) -> bool:
    lower = name.lower()
    stem = lower.rsplit(".", 1)[0]
    return lower.startswith(("api-ms-win-", "ext-ms-win-")) or stem in WINDOWS


def main():
    root = Path(sys.argv[1])
    objdump = sys.argv[2] if len(sys.argv) > 2 else "objdump"
    binaries = [p for p in root.rglob("*") if p.suffix.lower() in (".exe", ".dll")]
    bundled = {p.name.lower() for p in (root / "bin").glob("*.dll")}
    missing = {}
    for binary in binaries:
        out = subprocess.run([objdump, "-p", str(binary)], capture_output=True, text=True).stdout
        for dll in re.findall(r"DLL Name: (\S+)", out):
            if dll.lower() not in bundled and not is_windows_dll(dll):
                missing.setdefault(dll, []).append(binary.name)
    if missing:
        sys.exit("Missing DLLs:\n" + "\n".join(f"  {d}  (needed by {', '.join(u)})" for d, u in missing.items()))
    print(f"All DLLs present ({len(binaries)} Windows files checked).")


if __name__ == "__main__":
    main()
