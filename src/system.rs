//! Facts about the computer we are running on.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Arch {
    X86_64,
    Aarch64,
}

impl Arch {
    /// The CPU architecture of this computer.
    pub fn host() -> Option<Arch> {
        #[allow(unused_mut)]
        let mut arch = std::env::consts::ARCH;

        // An Intel build running under Rosetta on Apple Silicon should still
        // pick the ARM image, because that is what the hardware can run fast.
        #[cfg(target_os = "macos")]
        if arch == "x86_64" {
            let translated = Command::new("sysctl")
                .args(["-n", "sysctl.proc_translated"])
                .output()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "1")
                .unwrap_or(false);
            if translated {
                arch = "aarch64";
            }
        }

        match arch {
            "x86_64" => Some(Arch::X86_64),
            "aarch64" => Some(Arch::Aarch64),
            _ => None,
        }
    }

    /// The end of the ISO file name in the GitHub releases.
    pub fn iso_suffix(self) -> &'static str {
        match self {
            Arch::X86_64 => "-amd64.iso",
            Arch::Aarch64 => "-arm64.iso",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Arch::X86_64 => "Intel or AMD processor (64-bit)",
            Arch::Aarch64 => "ARM processor (64-bit)",
        }
    }

    pub fn qemu_program(self) -> &'static str {
        match self {
            Arch::X86_64 => "qemu-system-x86_64",
            Arch::Aarch64 => "qemu-system-aarch64",
        }
    }
}

/// Total memory of this computer in MiB.
pub fn total_memory_mb() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let info = std::fs::read_to_string("/proc/meminfo").ok()?;
        let line = info.lines().find(|l| l.starts_with("MemTotal:"))?;
        let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
        Some(kb / 1024)
    }
    #[cfg(target_os = "macos")]
    {
        let out = Command::new("sysctl").args(["-n", "hw.memsize"]).output().ok()?;
        let bytes: u64 = String::from_utf8_lossy(&out.stdout).trim().parse().ok()?;
        Some(bytes / 1024 / 1024)
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
        let mut status: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
        status.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        if unsafe { GlobalMemoryStatusEx(&mut status) } != 0 {
            Some(status.ullTotalPhys / 1024 / 1024)
        } else {
            None
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        None
    }
}

/// A `Command` that never flashes a console window on Windows.
pub fn command(program: &Path) -> Command {
    #[allow(unused_mut)]
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// Folders where a QEMU bundled with Try MAN lives:
/// next to our program (AppImage `usr/bin`, Windows `bin`, macOS `Contents/MacOS`).
fn bundle_dirs() -> Vec<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
        .into_iter()
        .collect()
}

pub fn cpu_count() -> u32 {
    std::thread::available_parallelism()
        .map(|n| n.get() as u32)
        .unwrap_or(2)
}

/// Looks for a program in PATH and in the usual install locations
/// (apps started from a desktop menu often get a very short PATH).
/// A bundled copy (shipped inside Try MAN) is preferred over an installed one.
pub fn find_program(name: &str) -> Option<PathBuf> {
    let file = format!("{name}{}", std::env::consts::EXE_SUFFIX);
    let mut dirs = bundle_dirs();
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    if cfg!(unix) {
        for extra in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/usr/libexec"] {
            dirs.push(PathBuf::from(extra));
        }
    }
    dirs.into_iter().map(|d| d.join(&file)).find(|p| p.is_file())
}

/// The command a person should type to install QEMU on this system.
pub fn qemu_install_command() -> String {
    if cfg!(windows) {
        return "QEMU is included with Try MAN. Please reinstall Try MAN.".into();
    }
    if cfg!(target_os = "macos") {
        return "brew install qemu".into();
    }
    let os_release = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
    let mut ids = String::new();
    for line in os_release.lines() {
        if let Some(v) = line.strip_prefix("ID=").or_else(|| line.strip_prefix("ID_LIKE=")) {
            ids.push(' ');
            ids.push_str(&v.trim_matches('"').to_lowercase());
        }
    }
    let has = |name: &str| ids.split_whitespace().any(|id| id == name);
    let arm = Arch::host() == Some(Arch::Aarch64);

    if has("fedora") || has("rhel") || has("centos") {
        if arm {
            "sudo dnf install qemu-system-aarch64 qemu-img edk2-aarch64".into()
        } else {
            "sudo dnf install qemu-kvm qemu-img".into()
        }
    } else if has("arch") {
        "sudo pacman -S qemu-full".into()
    } else if has("suse") || has("opensuse") {
        "sudo zypper install qemu".into()
    } else if arm {
        "sudo apt install qemu-system-arm qemu-utils qemu-efi-aarch64".into()
    } else {
        "sudo apt install qemu-system-x86 qemu-utils".into()
    }
}
