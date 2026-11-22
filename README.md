# Try MAN

A very simple GNOME/Adwaita app that lets anyone try the
[MAN](https://github.com/MANCorporation/MAN) distribution in a virtual computer.

- Detects the processor (Intel/AMD 64-bit → `*-amd64.iso`, ARM 64-bit → `*-arm64.iso`).
- Downloads the newest ISO from the GitHub releases (pre-releases included), with a
  progress bar, time left, resume after cancel, and a SHA-256 check.
- Runs it with QEMU, using KVM (Linux) or Hypervisor.framework (macOS) when available.
- **Fast 3D graphics** on Linux: MAN's graphics run on the real graphics card
  (virgl, `virtio-vga-gl` + `egl-headless`). Turned on automatically when the computer
  and QEMU support it; if it fails to start, MAN restarts with normal graphics.
  Can be switched off in the menu (*Fast 3D Graphics*). QEMU can't save a VM that uses
  3D, so while it's on the close dialog offers Turn Off / Delete but not Save.
  Homebrew QEMU on macOS has no 3D support, so macOS uses normal graphics.
- Shows the VM screen **inside the app window** (no separate QEMU window).
- **Start** and **Stop** buttons in the header bar.
- Closing the window while MAN runs asks: **Save** (continue exactly where you left off
  next time), **Turn Off**, **Delete**, or **Cancel**.
- A banner offers new MAN versions when they are released.
- Menu: *Fast 3D Graphics*, *Delete MAN From This Computer* and *About*.
- If QEMU is missing, it shows the exact install command for the person's distro.

## Build

Needs Rust, GTK 4.14+, libadwaita 1.5+ and QEMU.

Debian / Ubuntu:

```bash
sudo apt install cargo libgtk-4-dev libadwaita-1-dev qemu-system-x86 qemu-utils
```

Fedora:

```bash
sudo dnf install cargo gtk4-devel libadwaita-devel qemu-kvm qemu-img
```

Then:

```bash
cargo build --release
```

The app is `target/release/try-man`. To add it to the app menu:

```bash
sudo install -Dm755 target/release/try-man /usr/local/bin/try-man
```

```bash
sudo install -Dm644 data/io.github.mancorporation.TryMan.desktop /usr/local/share/applications/io.github.mancorporation.TryMan.desktop
```

```bash
sudo cp -r data/icons /usr/local/share/
```

The app icon is the MAN logo wrapped in GNOME-style hazard stripes (it's a test
version). The icon PNGs in `data/icons` are made from `data/man-logo.png` by:

```bash
cargo test export_app_icon -- --ignored
```

That writes `data/app-icon-1024.png`; resize it into the `data/icons/hicolor/*/apps/` sizes.

On macOS (for development): `brew install gtk4 libadwaita adwaita-icon-theme qemu`.

For fast 3D graphics, QEMU's OpenGL parts must be installed too:

- Debian / Ubuntu: `qemu-system-gui qemu-system-modules-opengl`
- Fedora: `qemu-ui-egl-headless qemu-device-display-virtio-vga-gl qemu-device-display-virtio-gpu-pci-gl`
- Arch: included in `qemu-full`

## Where files go

- Downloaded ISO: `~/.local/share/try-man/downloads/`
- Virtual computer (40 GB sparse disk, saved state): `~/.local/share/try-man/vm/`
- Settings: `~/.local/share/try-man/settings.json`

## Self-tests

These boot a real (empty) VM, check the screen, save and resume it, look up the
newest release and test download resume:

```bash
cargo test -- --ignored --nocapture
```

## How it works

| File | What it does |
| --- | --- |
| `src/window.rs` | The Adwaita window, pages, dialogs, Start/Stop |
| `src/release.rs` | GitHub releases lookup, download, checksum |
| `src/vm.rs` | Creating the disk, QEMU command line, QMP control, save/resume |
| `src/vnc.rs` | Small VNC client that draws the VM screen into the window |
| `src/system.rs` | CPU architecture, memory, finding QEMU |

"Save" uses QEMU migration to a file, and the VM is started with `-incoming` from it
next time. The saved state is removed once MAN is running again, so it is used only once.
