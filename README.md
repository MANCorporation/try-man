# Try MAN

A very simple GNOME/Adwaita app that lets anyone try the
[MAN](https://github.com/MANCorporation/MAN) distribution in a virtual computer.
It is made for people who don't know much about computers: press **Start**, and MAN runs.

## What it does

- Detects the processor (Intel/AMD 64-bit → `*-amd64.iso`, ARM 64-bit → `*-arm64.iso`).
- Downloads the newest ISO from the GitHub releases (pre-releases included), with a
  progress bar, time left, resume after cancel, and a SHA-256 check.
- **Use a MAN ISO file you already have**: the link on the front page, the menu, or
  just drag an `.iso` onto the window. It checks that the file really is an ISO and
  warns if it's made for a different processor (or is the Apple Silicon install ISO).
- Runs it with QEMU, sped up by the computer's own virtualization: KVM (Linux),
  Hypervisor.framework (macOS) or Windows Hypervisor Platform (Windows).
- **Fast 3D graphics** on Linux: MAN's graphics run on the real graphics card
  (virgl, `virtio-vga-gl` + `egl-headless`). Turned on automatically when the computer
  and QEMU support it; if it fails to start, MAN restarts with normal graphics.
  Can be switched off in the menu (*Fast 3D Graphics*). QEMU can't save a VM that uses
  3D, so while it's on the close dialog offers Turn Off / Delete but not Save.
  macOS and Windows QEMU have no 3D support, so they use normal graphics.
- Shows the VM screen **inside the app window** (no separate QEMU window).
- **Start** and **Stop** buttons in the header bar.
- Closing the window while MAN runs asks: **Save** (continue exactly where you left off
  next time), **Turn Off**, **Delete**, or **Cancel**.
- A banner offers new MAN versions when they are released.
- Menu: *Use a MAN ISO File*, *Fast 3D Graphics*, *Delete MAN From This Computer*, *About*.

## Standalone packages

Each package contains everything: the app, GTK, libadwaita, icons **and QEMU** with
its firmware. Nothing else needs to be installed.

| System | Package | Build it with |
| --- | --- | --- |
| macOS (Apple Silicon / Intel) | `Try-MAN-10-macos-<arch>.dmg` | `packaging/macos/build-app.sh` |
| Linux (x86_64 / arm64) | `Try-MAN-10-<arch>.AppImage` | `packaging/linux/build-appimage.sh` |
| Windows (x64) | `Try-MAN-10-windows-x64-setup.exe` + portable `.zip` | `packaging/windows/build-windows.sh` |

Each package has to be built on its own system. The easiest way is GitHub Actions:
[.github/workflows/packages.yml](.github/workflows/packages.yml) builds all of them on
every push, and pushing a tag publishes them as a release:

```bash
git tag v10
```

```bash
git push origin v10
```

### macOS

```bash
brew install gtk4 libadwaita adwaita-icon-theme librsvg qemu
```

```bash
packaging/macos/build-app.sh
```

Makes `dist/Try MAN.app` and the `.dmg`. Every library is copied into the app and
relinked, so it doesn't need Homebrew. Two things to know:

- **macOS version:** Homebrew builds its libraries for the macOS version it runs on, so the
  app works on *that macOS version and newer* (the script prints it and puts it in
  `Info.plist`). Build on an older Mac (or the GitHub runner) to support older Macs.
- **Signing:** by default the app is signed ad-hoc, which works on your own Mac. To give it
  to other people without Gatekeeper warnings, sign with your Apple Developer ID and notarize:

```bash
SIGN_IDENTITY="Developer ID Application: Your Name (TEAMID)" packaging/macos/build-app.sh
```

```bash
xcrun notarytool submit dist/Try-MAN-10-macos-arm64.dmg --keychain-profile <profile> --wait
```

```bash
xcrun stapler staple dist/Try-MAN-10-macos-arm64.dmg
```

### Linux (AppImage)

On Ubuntu 24.04 (x86_64 or arm64):

```bash
sudo packaging/linux/build-appimage.sh --install-deps
```

```bash
packaging/linux/build-appimage.sh
```

Bundles QEMU with its BIOS files and its modules (virtual graphics cards, 3D, software CPU).
The host's graphics drivers are used for 3D. KVM works from the AppImage as long as the
person can use `/dev/kvm` (normal on most desktop distros).

### Windows

In an **MSYS2 UCRT64** shell (<https://www.msys2.org>):

```bash
packaging/windows/build-windows.sh --install-deps
```

```bash
packaging/windows/build-windows.sh
```

Makes an installer (per-user, no administrator password, Start menu and desktop
shortcuts, uninstaller) and a portable zip. For speed, people should switch on
**Windows Hypervisor Platform** in *Turn Windows features on or off*; without it MAN
still runs, just slower.

## Development build

Needs Rust, GTK 4.14+, libadwaita 1.5+ and QEMU.

```bash
sudo apt install cargo libgtk-4-dev libadwaita-1-dev qemu-system-x86 qemu-utils
```

```bash
cargo run
```

For fast 3D graphics, QEMU's OpenGL parts must be installed too:

- Debian / Ubuntu: `qemu-system-gui qemu-system-modules-opengl`
- Fedora: `qemu-ui-egl-headless qemu-device-display-virtio-vga-gl qemu-device-display-virtio-gpu-pci-gl`
- Arch: included in `qemu-full`

The app icon is the MAN logo wrapped in GNOME-style hazard stripes (it's a test
version). `cargo test export_app_icon -- --ignored` writes `data/app-icon-1024.png` from
`data/man-logo.png`; the `data/icons` sizes are made from that.

## Where files go

- Downloaded or imported ISO: `<user data>/try-man/downloads/`
- Virtual computer (40 GB sparse disk, saved state): `<user data>/try-man/vm/`
- Settings: `<user data>/try-man/settings.json`

`<user data>` is `~/.local/share` on Linux and macOS, and `AppData\Local` on Windows.

## Tests

```bash
cargo test
```

These also boot a real (empty) VM, check the screen, save and resume it, look up the
newest release and test download resume:

```bash
cargo test -- --ignored --nocapture
```

## How it works

| File | What it does |
| --- | --- |
| `src/window.rs` | The Adwaita window, pages, dialogs, Start/Stop, ISO import |
| `src/release.rs` | GitHub releases lookup, download, checksum, ISO checks and copying |
| `src/vm.rs` | Creating the disk, QEMU command line, QMP control, save/resume, 3D detection |
| `src/vnc.rs` | Small VNC client that draws the VM screen into the window |
| `src/net.rs` | Private connections to QEMU: Unix sockets, or local TCP on Windows |
| `src/system.rs` | CPU architecture, memory, finding the bundled or installed QEMU |
| `packaging/` | Build scripts for the macOS app, Linux AppImage and Windows installer |

"Save" uses QEMU migration to a file, and the VM is started with `-incoming` from it
next time. The saved state is removed once MAN is running again, so it is used only once.
