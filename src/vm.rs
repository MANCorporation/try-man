//! Creating, starting, saving and stopping the QEMU virtual machine.

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use gtk::glib;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::net::{Endpoint, Stream};
use crate::system::{self, Arch};

const DISK_SIZE: &str = "40G";
const FLASH_SIZE: u64 = 64 * 1024 * 1024;

// ---------------------------------------------------------------------------
// Where things live on disk

#[derive(Clone, Debug)]
pub struct VmPaths {
    pub iso_dir: PathBuf,
    pub dir: PathBuf,
    pub config: PathBuf,
    pub disk: PathBuf,
    pub efi_code: PathBuf,
    pub efi_vars: PathBuf,
    /// Saved memory state ("Save" in the close dialog).
    pub state: PathBuf,
    /// The person's choices (kept when MAN is deleted).
    pub settings: PathBuf,
}

impl VmPaths {
    pub fn new() -> Self {
        let base = glib::user_data_dir().join("try-man");
        let dir = base.join("vm");
        VmPaths {
            iso_dir: base.join("downloads"),
            config: dir.join("vm.json"),
            disk: dir.join("disk.qcow2"),
            efi_code: dir.join("efi-code.fd"),
            efi_vars: dir.join("efi-vars.fd"),
            state: dir.join("saved-state.bin"),
            settings: base.join("settings.json"),
            dir,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Use the computer's graphics card for MAN's graphics (3D).
    pub fast_graphics: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { fast_graphics: true }
    }
}

pub fn load_settings(paths: &VmPaths) -> Settings {
    fs::read_to_string(&paths.settings)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

pub fn save_settings(paths: &VmPaths, settings: &Settings) {
    if let Some(dir) = paths.settings.parent() {
        let _ = fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_string_pretty(settings) {
        let _ = fs::write(&paths.settings, json);
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VmConfig {
    pub arch: Arch,
    pub iso: PathBuf,
    pub version_name: String,
    pub memory_mb: u64,
    pub cpus: u32,
}

impl VmConfig {
    pub fn new(arch: Arch, iso: PathBuf, version_name: String) -> Self {
        let total = system::total_memory_mb().unwrap_or(8192);
        let memory_mb = (total / 2).clamp(1536, 4096);
        let cpus = (system::cpu_count() / 2).clamp(1, 4).max(2.min(system::cpu_count()));
        VmConfig { arch, iso, version_name, memory_mb, cpus }
    }
}

pub fn load_config(paths: &VmPaths) -> Result<VmConfig, String> {
    let text = fs::read_to_string(&paths.config).map_err(|e| e.to_string())?;
    let cfg: VmConfig = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    if !paths.disk.is_file() || !cfg.iso.is_file() {
        return Err("virtual machine files are missing".into());
    }
    Ok(cfg)
}

// ---------------------------------------------------------------------------
// QEMU programs

#[derive(Clone, Debug)]
pub struct Qemu {
    pub system: PathBuf,
    pub img: PathBuf,
    pub version: (u32, u32),
    /// QEMU and the graphics card can do accelerated 3D graphics (virgl).
    pub gl: bool,
    /// Firmware/BIOS folder of a QEMU that is bundled inside Try MAN.
    pub data_dir: Option<PathBuf>,
}

impl Qemu {
    /// QEMU 9.0+ can write saved state straight to a file.
    fn has_file_migration(&self) -> bool {
        self.version >= (9, 0)
    }
}

pub fn find_qemu(arch: Arch) -> Option<Qemu> {
    let system = system::find_program(arch.qemu_program())?;
    let img = system
        .parent()
        .map(|d| d.join(format!("qemu-img{}", std::env::consts::EXE_SUFFIX)))
        .filter(|p| p.is_file())
        .or_else(|| system::find_program("qemu-img"))?;
    let version = system::command(&system)
        .arg("--version")
        .output()
        .ok()
        .and_then(|o| parse_version(&String::from_utf8_lossy(&o.stdout)))
        .unwrap_or((0, 0));
    let gl = detect_gl(&system, arch);
    let data_dir = bundled_data_dir(&system);
    Some(Qemu { system, img, version, gl, data_dir })
}

/// When QEMU ships inside Try MAN (.app, AppImage, Windows folder), tell it
/// where its BIOS and keyboard files are.
fn bundled_data_dir(system: &Path) -> Option<PathBuf> {
    let bin = system.parent()?;
    let ours = std::env::current_exe().ok()?.parent()?.to_path_buf();
    if bin != ours.as_path() {
        return None; // a QEMU installed on the system finds its own files
    }
    [
        bin.join("../share/qemu"),             // AppImage, Windows
        bin.join("../Resources/qemu/share/qemu"), // macOS .app
    ]
    .into_iter()
    .find(|d| d.join("keymaps").is_dir())
}

/// The virtual graphics card that passes 3D work to the real graphics card.
fn gl_device(arch: Arch) -> &'static str {
    match arch {
        Arch::X86_64 => "virtio-vga-gl",
        Arch::Aarch64 => "virtio-gpu-gl-pci",
    }
}

/// Accelerated graphics need Linux, a usable GPU render node, and a QEMU
/// built with OpenGL/virglrenderer. (Homebrew's macOS QEMU has no virgl.)
fn detect_gl(system: &Path, arch: Arch) -> bool {
    if !cfg!(target_os = "linux") {
        return false;
    }
    let render_node = fs::read_dir("/dev/dri")
        .map(|dir| {
            dir.flatten().any(|e| {
                e.file_name().to_string_lossy().starts_with("renderD")
                    && OpenOptions::new().read(true).write(true).open(e.path()).is_ok()
            })
        })
        .unwrap_or(false);
    if !render_node {
        return false;
    }
    let help = |what: &str| {
        system::command(system)
            .args([what, "help"])
            .output()
            .map(|o| {
                let mut text = String::from_utf8_lossy(&o.stdout).into_owned();
                text.push_str(&String::from_utf8_lossy(&o.stderr));
                text
            })
            .unwrap_or_default()
    };
    help("-display").contains("egl-headless")
        && help("-device").contains(&format!("\"{}\"", gl_device(arch)))
}

fn parse_version(text: &str) -> Option<(u32, u32)> {
    let word = text.split("version").nth(1)?.split_whitespace().next()?;
    let mut parts = word.split('.').map(|p| p.parse::<u32>().ok());
    Some((parts.next()??, parts.next().flatten().unwrap_or(0)))
}

/// UEFI firmware is needed for ARM virtual machines.
fn find_aarch64_firmware(qemu: &Qemu) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(data) = &qemu.data_dir {
        candidates.push(data.join("edk2-aarch64-code.fd"));
    }
    if let Some(prefix) = qemu.system.parent().and_then(|bin| bin.parent()) {
        candidates.push(prefix.join("share/qemu/edk2-aarch64-code.fd"));
    }
    for p in [
        "/opt/homebrew/share/qemu/edk2-aarch64-code.fd",
        "/usr/local/share/qemu/edk2-aarch64-code.fd",
        "/usr/share/qemu/edk2-aarch64-code.fd",
        "/usr/share/AAVMF/AAVMF_CODE.fd",
        "/usr/share/qemu-efi-aarch64/QEMU_EFI.fd",
        "/usr/share/edk2/aarch64/QEMU_EFI-pflash.raw",
        "/usr/share/edk2/aarch64/QEMU_EFI.fd",
        "/usr/share/edk2-armvirt/aarch64/QEMU_EFI.fd",
        "/usr/share/edk2/aarch64/QEMU_CODE.fd",
    ] {
        candidates.push(PathBuf::from(p));
    }
    candidates.into_iter().find(|p| p.is_file())
}

/// Speeds the VM up with the computer's built-in virtualization when we can.
/// Returns (accelerators to try in order, cpu model).
pub fn accelerators(arch: Arch) -> (Vec<&'static str>, &'static str) {
    if Arch::host() == Some(arch) {
        if cfg!(target_os = "macos") {
            return (vec!["hvf"], "host");
        }
        if cfg!(target_os = "linux")
            && OpenOptions::new().read(true).write(true).open("/dev/kvm").is_ok()
        {
            return (vec!["kvm"], "host");
        }
        if cfg!(windows) {
            // Windows Hypervisor Platform; QEMU falls back to the next one
            // if it isn't switched on in Windows Features.
            return (vec!["whpx,kernel-irqchip=off", "tcg"], "max");
        }
    }
    (vec!["tcg"], "max")
}

/// True when MAN will run without hardware help (much slower).
pub fn is_slow(arch: Arch) -> bool {
    accelerators(arch).0 == ["tcg"]
}

// ---------------------------------------------------------------------------
// Creating the VM

pub fn create(paths: &VmPaths, cfg: &VmConfig, qemu: &Qemu) -> Result<(), String> {
    let io_err = |e: std::io::Error| format!("Couldn't create the virtual computer: {e}");
    let _ = fs::remove_dir_all(&paths.dir);
    fs::create_dir_all(&paths.dir).map_err(io_err)?;

    let out = system::command(&qemu.img)
        .args(["create", "-q", "-f", "qcow2"])
        .arg(&paths.disk)
        .arg(DISK_SIZE)
        .output()
        .map_err(io_err)?;
    if !out.status.success() {
        return Err(format!(
            "Couldn't create the virtual disk:\n{}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }

    if cfg.arch == Arch::Aarch64 {
        let firmware = find_aarch64_firmware(qemu).ok_or_else(|| {
            format!(
                "The UEFI firmware for ARM virtual computers is missing.\nInstall it with:\n\n{}",
                system::qemu_install_command()
            )
        })?;
        fs::copy(&firmware, &paths.efi_code).map_err(io_err)?;
        let code = OpenOptions::new().write(true).open(&paths.efi_code).map_err(io_err)?;
        let size = code.metadata().map_err(io_err)?.len().max(FLASH_SIZE);
        code.set_len(size).map_err(io_err)?;
        // An empty variable store; the firmware formats it on first boot.
        File::create(&paths.efi_vars)
            .and_then(|f| f.set_len(size))
            .map_err(io_err)?;
    }

    let json = serde_json::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    fs::write(&paths.config, json).map_err(io_err)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Running the VM

/// The control connections of one running VM.
pub struct Sockets {
    pub dir: PathBuf,
    pub vnc: Endpoint,
    pub qmp: Endpoint,
}

fn sockets_base() -> PathBuf {
    let base = glib::user_runtime_dir();
    // Unix socket paths must be short.
    if cfg!(unix) && base.as_os_str().len() > 70 {
        PathBuf::from("/tmp")
    } else {
        base
    }
}

/// How to reach the QMP server of the VM whose files are in `dir`.
fn qmp_endpoint_in(dir: &Path) -> Option<Endpoint> {
    #[cfg(unix)]
    if dir.join("qmp.sock").exists() {
        return Some(Endpoint::Unix(dir.join("qmp.sock")));
    }
    let port = fs::read_to_string(dir.join("qmp.port")).ok()?;
    port.trim().parse().ok().map(Endpoint::Tcp)
}

/// If Try MAN crashed earlier, its VM may still be running in the
/// background. Turn such leftovers off.
pub fn stop_leftover_vms() {
    let me = format!("try-man-{}", std::process::id());
    let Ok(entries) = fs::read_dir(sockets_base()) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("try-man-") || name == me {
            continue;
        }
        if qmp_endpoint_in(&entry.path()).is_some_and(|qmp| quit(&qmp).is_ok()) {
            // Give QEMU a moment to release the disk.
            std::thread::sleep(Duration::from_secs(1));
        }
        let _ = fs::remove_dir_all(entry.path());
    }
}

impl Sockets {
    pub fn new() -> std::io::Result<Self> {
        let dir = sockets_base().join(format!("try-man-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir)?;
        #[cfg(unix)]
        let (vnc, qmp) = (
            Endpoint::Unix(dir.join("vnc.sock")),
            Endpoint::Unix(dir.join("qmp.sock")),
        );
        #[cfg(not(unix))]
        let (vnc, qmp) = {
            let busy = || std::io::Error::other("no free local port for QEMU");
            let vnc = crate::net::free_port(5950..6100).ok_or_else(busy)?;
            let qmp = crate::net::free_port(44440..44600).ok_or_else(busy)?;
            // Lets a later run find and stop this VM if Try MAN crashes.
            fs::write(dir.join("qmp.port"), qmp.to_string())?;
            (Endpoint::Tcp(vnc), Endpoint::Tcp(qmp))
        };
        Ok(Sockets { vnc, qmp, dir })
    }

    pub fn cleanup(&self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// QEMU option values use "," as separator, so it must be doubled.
fn opt_path(p: &Path) -> String {
    p.to_string_lossy().replace(',', ",,")
}

fn shell_quote(p: &Path) -> String {
    format!("'{}'", p.to_string_lossy().replace('\'', "'\\''"))
}

fn state_uri(qemu: &Qemu, path: &Path, writing: bool) -> String {
    if qemu.has_file_migration() {
        format!("file:{}", path.to_string_lossy())
    } else if writing {
        format!("exec:cat > {}", shell_quote(path))
    } else {
        format!("exec:cat {}", shell_quote(path))
    }
}

pub fn build_args(
    cfg: &VmConfig,
    paths: &VmPaths,
    qemu: &Qemu,
    sockets: &Sockets,
    resume: bool,
    gl: bool,
) -> Vec<OsString> {
    let (accels, cpu) = accelerators(cfg.arch);
    let mut a: Vec<String> = vec!["-name".into(), "MAN".into()];
    if let Some(data) = &qemu.data_dir {
        a.extend(["-L".into(), data.to_string_lossy().into_owned()]);
    }
    for accel in accels {
        a.extend(["-accel".into(), accel.into()]);
    }

    match cfg.arch {
        Arch::X86_64 => {
            a.extend(["-machine".into(), "q35".into()]);
        }
        Arch::Aarch64 => {
            a.extend(["-machine".into(), "virt".into()]);
            a.extend([
                "-drive".into(),
                format!("if=pflash,format=raw,readonly=on,file={}", opt_path(&paths.efi_code)),
                "-drive".into(),
                format!("if=pflash,format=raw,file={}", opt_path(&paths.efi_vars)),
            ]);
        }
    }

    a.extend([
        "-cpu".into(),
        cpu.into(),
        "-smp".into(),
        cfg.cpus.to_string(),
        "-m".into(),
        format!("{}M", cfg.memory_mb),
        // Hard disk (tried first, so MAN boots from it once installed)
        "-drive".into(),
        format!("if=none,id=hd0,format=qcow2,discard=unmap,file={}", opt_path(&paths.disk)),
        "-device".into(),
        "virtio-blk-pci,drive=hd0,bootindex=0".into(),
        // The MAN ISO as a CD
        "-device".into(),
        "virtio-scsi-pci,id=scsi0".into(),
        "-drive".into(),
        format!("if=none,id=cd0,media=cdrom,readonly=on,format=raw,file={}", opt_path(&cfg.iso)),
        "-device".into(),
        "scsi-cd,drive=cd0,bus=scsi0.0,bootindex=1".into(),
        // Screen, keyboard, mouse, network
        "-device".into(),
        match (gl, cfg.arch) {
            (true, arch) => gl_device(arch).into(),
            (false, Arch::X86_64) => "virtio-vga".into(),
            (false, Arch::Aarch64) => "virtio-gpu-pci".into(),
        },
        "-device".into(),
        "qemu-xhci".into(),
        "-device".into(),
        "usb-tablet".into(),
        "-device".into(),
        "usb-kbd".into(),
        "-nic".into(),
        "user,model=virtio-net-pci".into(),
        "-device".into(),
        "virtio-rng-pci".into(),
        // We draw the screen ourselves, through a private VNC socket.
        // With 3D on, the GPU renders off-screen and VNC gets the result.
        "-display".into(),
        if gl { "egl-headless" } else { "none" }.into(),
        "-vnc".into(),
        sockets.vnc.vnc_arg(),
        "-qmp".into(),
        sockets.qmp.qmp_arg(),
        "-monitor".into(),
        "none".into(),
        "-serial".into(),
        "none".into(),
        "-parallel".into(),
        "none".into(),
    ]);

    if resume {
        a.extend(["-incoming".into(), state_uri(qemu, &paths.state, false)]);
    }

    a.into_iter().map(OsString::from).collect()
}

// ---------------------------------------------------------------------------
// QMP: QEMU's control protocol

struct Qmp {
    reader: BufReader<Stream>,
    writer: Stream,
}

impl Qmp {
    fn connect(endpoint: &Endpoint) -> Result<Self, String> {
        let stream = endpoint.connect().map_err(|e| e.to_string())?;
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .map_err(|e| e.to_string())?;
        let writer = stream.try_clone().map_err(|e| e.to_string())?;
        let mut qmp = Qmp { reader: BufReader::new(stream), writer };
        qmp.read()?; // greeting
        qmp.execute("qmp_capabilities", None)?;
        Ok(qmp)
    }

    fn read(&mut self) -> Result<Value, String> {
        let mut line = String::new();
        loop {
            line.clear();
            let n = self.reader.read_line(&mut line).map_err(|e| e.to_string())?;
            if n == 0 {
                return Err("QEMU closed the connection".into());
            }
            if !line.trim().is_empty() {
                return serde_json::from_str(&line).map_err(|e| e.to_string());
            }
        }
    }

    fn execute(&mut self, cmd: &str, args: Option<Value>) -> Result<Value, String> {
        let mut msg = json!({ "execute": cmd });
        if let Some(args) = args {
            msg["arguments"] = args;
        }
        writeln!(self.writer, "{msg}").map_err(|e| e.to_string())?;
        loop {
            let reply = self.read()?;
            if reply.get("event").is_some() {
                continue;
            }
            if let Some(ret) = reply.get("return") {
                return Ok(ret.clone());
            }
            if let Some(err) = reply.get("error") {
                return Err(err["desc"].as_str().unwrap_or("unknown QEMU error").to_string());
            }
        }
    }
}

/// Turns the VM off immediately.
pub fn quit(qmp_path: &Endpoint) -> Result<(), String> {
    let mut qmp = Qmp::connect(qmp_path)?;
    let _ = qmp.execute("quit", None);
    Ok(())
}

/// Saves the whole running VM (memory and devices) to `state`, then quits.
pub fn save_state(qmp_path: &Endpoint, qemu: &Qemu, state: &Path) -> Result<(), String> {
    let part = state.with_extension("part");
    let _ = fs::remove_file(&part);

    let mut qmp = Qmp::connect(qmp_path)?;
    let uri = state_uri(qemu, &part, true);
    qmp.execute("migrate", Some(json!({ "uri": uri })))?;

    let deadline = Instant::now() + Duration::from_secs(600);
    loop {
        std::thread::sleep(Duration::from_millis(200));
        let info = qmp.execute("query-migrate", None)?;
        match info["status"].as_str().unwrap_or("") {
            "completed" => break,
            "failed" | "cancelled" => {
                let _ = fs::remove_file(&part);
                return Err(info["error-desc"]
                    .as_str()
                    .unwrap_or("QEMU couldn't save the virtual computer")
                    .to_string());
            }
            _ => {}
        }
        if Instant::now() > deadline {
            let _ = qmp.execute("migrate_cancel", None);
            let _ = fs::remove_file(&part);
            return Err("Saving took too long.".into());
        }
    }

    if !qemu.has_file_migration() {
        // Give `cat` a moment to finish writing.
        std::thread::sleep(Duration::from_millis(500));
    }
    fs::rename(&part, state).map_err(|e| e.to_string())?;
    let _ = qmp.execute("quit", None);
    Ok(())
}

/// Waits until a VM that is restoring a saved state is running again.
pub fn wait_until_running(qmp_path: &Endpoint, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Ok(mut qmp) = Qmp::connect(qmp_path) {
            while Instant::now() < deadline {
                match qmp.execute("query-status", None) {
                    Ok(v) if v["status"] == "running" => return true,
                    Ok(_) => std::thread::sleep(Duration::from_millis(300)),
                    Err(_) => break,
                }
            }
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    false
}
