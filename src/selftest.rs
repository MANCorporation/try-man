//! End-to-end check of QEMU + VNC + save/resume, without the GUI.
//! Run with:  cargo test -- --ignored --nocapture

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::system::Arch;
use crate::vm::{self, VmConfig, VmPaths};
use crate::vnc::VncClient;

fn wait_for_frame(sock: &crate::net::Endpoint) -> (u32, u32) {
    let deadline = Instant::now() + Duration::from_secs(60);
    let client = loop {
        if let Ok(c) = VncClient::connect(sock) {
            break c;
        }
        assert!(Instant::now() < deadline, "VNC never came up");
        std::thread::sleep(Duration::from_millis(200));
    };
    loop {
        if let Some((w, h, px)) = client.take_frame() {
            if px.iter().any(|b| *b != 0) {
                client.shutdown();
                return (w, h);
            }
        }
        assert!(Instant::now() < deadline, "no picture from VM");
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
#[ignore]
fn boot_save_resume() {
    let arch = Arch::host().expect("unsupported arch");
    let qemu = vm::find_qemu(arch).expect("qemu missing");
    println!("qemu {:?} version {:?} accel {:?} 3d {}", qemu.system, qemu.version, vm::accelerators(arch), qemu.gl);

    let base = std::env::temp_dir().join("try-man-selftest");
    let _ = std::fs::remove_dir_all(&base);
    let dir = base.join("vm");
    let paths = VmPaths {
        iso_dir: base.join("downloads"),
        config: dir.join("vm.json"),
        disk: dir.join("disk.qcow2"),
        efi_code: dir.join("efi-code.fd"),
        efi_vars: dir.join("efi-vars.fd"),
        state: dir.join("saved-state.bin"),
        settings: base.join("settings.json"),
        dir,
    };
    std::fs::create_dir_all(&paths.iso_dir).unwrap();
    let iso = std::env::var("TRYMAN_ISO").map(Into::into).unwrap_or_else(|_| {
        let p = paths.iso_dir.join("fake.iso");
        std::fs::File::create(&p).unwrap().set_len(1 << 20).unwrap();
        p
    });
    let cfg = VmConfig::new(arch, iso, "test".into());
    vm::create(&paths, &cfg, &qemu).unwrap();
    assert!(vm::load_config(&paths).is_ok());

    for resume in [false, true] {
        let sockets = vm::Sockets::new().unwrap();
        let args = vm::build_args(&cfg, &paths, &qemu, &sockets, resume, false);
        let mut child = Command::new(&qemu.system).args(&args).stderr(Stdio::inherit()).spawn().unwrap();
        if resume {
            assert!(vm::wait_until_running(&sockets.qmp, Duration::from_secs(60)), "resume failed");
            println!("resumed OK");
        }
        let (w, h) = wait_for_frame(&sockets.vnc);
        println!("resume={resume}: got screen {w}x{h}");
        if !resume {
            std::thread::sleep(Duration::from_secs(3));
            vm::save_state(&sockets.qmp, &qemu, &paths.state).expect("save failed");
            let status = child.wait().unwrap();
            println!("saved, qemu exited {status}, state {} bytes", std::fs::metadata(&paths.state).unwrap().len());
        } else {
            vm::quit(&sockets.qmp).unwrap();
            child.wait().unwrap();
        }
        sockets.cleanup();
    }
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
#[ignore]
fn finds_latest_release() {
    for arch in [Arch::X86_64, Arch::Aarch64] {
        let rel = crate::release::latest(arch).expect("no release");
        println!("{arch:?}: {} -> {} ({} bytes, sha: {})", rel.name, rel.iso_name, rel.iso_size, rel.sha256_url.is_some());
        assert!(rel.iso_name.ends_with(arch.iso_suffix()));
    }
}

#[test]
#[ignore]
fn download_resume_and_checksum() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let base = "https://github.com/MANCorporation/MAN/releases/download/v10-test9/DUHACH-1.1.0-mac.pkg";
    let rel = crate::release::Release {
        name: "test".into(),
        iso_name: "DUHACH-1.1.0-mac.pkg".into(),
        iso_url: base.into(),
        iso_size: 23429367,
        sha256_url: Some(format!("{base}.sha256")),
    };
    let dir = std::env::temp_dir().join("try-man-dltest");
    let _ = std::fs::remove_dir_all(&dir);
    // First attempt: cancel after ~5 MB.
    let cancel = AtomicBool::new(false);
    let r = crate::release::download(&rel, &dir, &cancel, |d, _| if d > 5_000_000 { cancel.store(true, Ordering::Relaxed) }, || {});
    assert_eq!(r.unwrap_err(), crate::release::CANCELLED);
    let partial = std::fs::metadata(dir.join("DUHACH-1.1.0-mac.pkg.part")).unwrap().len();
    println!("partial after cancel: {partial}");
    // Second attempt resumes and verifies.
    let mut first = None;
    let cancel = AtomicBool::new(false);
    let path = crate::release::download(&rel, &dir, &cancel, |d, _| { first.get_or_insert(d); }, || println!("verifying")).unwrap();
    println!("resumed from {:?}, final {:?} {}", first, path, std::fs::metadata(&path).unwrap().len());
    assert!(first.unwrap() >= partial);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Writes the striped app icon used by the .desktop file.
#[test]
#[ignore]
fn export_app_icon() {
    use gtk::prelude::*;
    let logo = crate::window::man_logo_with_tape().expect("couldn't draw logo");
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/data/app-icon-1024.png");
    logo.save_to_png(path).unwrap();
    println!("wrote {path}");
}

/// The QEMU options for accelerated 3D graphics (can't be run on macOS).
#[test]
fn gl_options() {
    use crate::net::Endpoint;
    let qemu = vm::Qemu { system: "qemu".into(), img: "qemu-img".into(), version: (11, 0), gl: true, data_dir: None, module_dir: None };
    let sockets = vm::Sockets { dir: "/tmp/t".into(), vnc: Endpoint::Unix("/tmp/t/vnc".into()), qmp: Endpoint::Unix("/tmp/t/qmp".into()) };
    for (arch, device) in [(Arch::X86_64, "virtio-vga-gl"), (Arch::Aarch64, "virtio-gpu-gl-pci")] {
        let cfg = VmConfig { arch, iso: "/a.iso".into(), version_name: "x".into(), memory_mb: 2048, cpus: 2 };
        let args = |gl| -> Vec<String> {
            vm::build_args(&cfg, &VmPaths::new(), &qemu, &sockets, false, gl)
                .into_iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect()
        };
        let on = args(true);
        assert!(on.windows(2).any(|w| w[0] == "-display" && w[1] == "egl-headless"));
        assert!(on.iter().any(|a| a == device));
        let off = args(false);
        assert!(off.windows(2).any(|w| w[0] == "-display" && w[1] == "none"));
        assert!(!off.iter().any(|a| a.contains("-gl")));
    }
}

#[test]
fn iso_import_helpers() {
    use crate::release::{check_iso, copy_iso, iso_kind, name_from_iso, IsoKind};
    use std::sync::atomic::AtomicBool;
    let dir = std::env::temp_dir().join("try-man-import-test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let good = dir.join("MAN-10-TEST-9-arm64.iso");
    let mut data = vec![0u8; 0x9000];
    data[0x8001..0x8006].copy_from_slice(b"CD001");
    std::fs::write(&good, &data).unwrap();
    let bad = dir.join("notes.iso");
    std::fs::write(&bad, b"hello").unwrap();

    assert!(check_iso(&good).is_ok());
    assert!(check_iso(&bad).is_err());
    assert_eq!(name_from_iso(&good), "MAN 10 TEST 9");
    assert!(matches!(iso_kind(&good), IsoKind::Arch(Arch::Aarch64)));
    assert!(matches!(iso_kind(std::path::Path::new("MAN-10-TEST-9-apple.iso")), IsoKind::AppleSilicon));
    assert!(matches!(iso_kind(std::path::Path::new("MAN-10-amd64.iso")), IsoKind::Arch(Arch::X86_64)));

    let dest_dir = dir.join("downloads");
    let copied = copy_iso(&good, &dest_dir, &AtomicBool::new(false), |_, _| {}).unwrap();
    assert_eq!(std::fs::read(&copied).unwrap(), data);
    // Importing the file that is already in place doesn't copy it again.
    assert_eq!(copy_iso(&copied, &dest_dir, &AtomicBool::new(false), |_, _| {}).unwrap(), copied);
    let _ = std::fs::remove_dir_all(&dir);
}
