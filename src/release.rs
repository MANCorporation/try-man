//! Finding and downloading the newest MAN ISO from GitHub.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::system::Arch;

const RELEASES_API: &str = "https://api.github.com/repos/MANCorporation/MAN/releases?per_page=30";
const USER_AGENT: &str = concat!("TryMAN/", env!("CARGO_PKG_VERSION_MAJOR"));

/// Returned when the person pressed Cancel.
pub const CANCELLED: &str = "cancelled";

#[derive(Clone, Debug)]
pub struct Release {
    pub name: String,
    pub iso_name: String,
    pub iso_url: String,
    pub iso_size: u64,
    pub sha256_url: Option<String>,
}

#[derive(Deserialize)]
struct GhRelease {
    tag_name: String,
    name: Option<String>,
    draft: bool,
    published_at: Option<String>,
    assets: Vec<GhAsset>,
}

#[derive(Deserialize)]
struct GhAsset {
    name: String,
    size: u64,
    browser_download_url: String,
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(20)))
        .timeout_recv_response(Some(Duration::from_secs(60)))
        .build()
        .into()
}

fn network_error(e: impl std::fmt::Display) -> String {
    format!("Couldn't connect to the internet. Please check your connection and try again.\n\n({e})")
}

/// The newest release (pre-releases included) that has an ISO for `arch`.
pub fn latest(arch: Arch) -> Result<Release, String> {
    let mut resp = agent()
        .get(RELEASES_API)
        .header("User-Agent", USER_AGENT)
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(network_error)?;
    let body = resp.body_mut().read_to_string().map_err(network_error)?;
    let mut releases: Vec<GhRelease> =
        serde_json::from_str(&body).map_err(|e| format!("GitHub sent an unexpected answer: {e}"))?;

    // Newest first. ISO-8601 dates sort correctly as text.
    releases.sort_by(|a, b| b.published_at.cmp(&a.published_at));

    for rel in releases.into_iter().filter(|r| !r.draft) {
        let Some(iso) = rel.assets.iter().find(|a| a.name.ends_with(arch.iso_suffix())) else {
            continue;
        };
        let sha_name = format!("{}.sha256", iso.name);
        let sha256_url = rel
            .assets
            .iter()
            .find(|a| a.name == sha_name)
            .map(|a| a.browser_download_url.clone());
        return Ok(Release {
            name: rel
                .name
                .filter(|n| !n.trim().is_empty())
                .unwrap_or(rel.tag_name),
            iso_name: iso.name.clone(),
            iso_url: iso.browser_download_url.clone(),
            iso_size: iso.size,
            sha256_url,
        });
    }
    Err(format!(
        "There is no MAN download for your computer yet ({}).",
        arch.label()
    ))
}

/// Downloads the ISO into `dir` (resuming an earlier partial download if
/// there is one), checks it, and returns its path.
pub fn download(
    rel: &Release,
    dir: &Path,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(u64, u64),
    mut on_verify: impl FnMut(),
) -> Result<PathBuf, String> {
    let io_err = |e: std::io::Error| format!("Couldn't save the download: {e}");
    fs::create_dir_all(dir).map_err(io_err)?;

    let dest = dir.join(&rel.iso_name);
    if fs::metadata(&dest).map(|m| m.len() == rel.iso_size).unwrap_or(false) {
        return Ok(dest);
    }

    let part = dir.join(format!("{}.part", rel.iso_name));
    let mut have = fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    if have > rel.iso_size {
        let _ = fs::remove_file(&part);
        have = 0;
    }

    if have < rel.iso_size {
        let mut req = agent().get(&rel.iso_url).header("User-Agent", USER_AGENT);
        if have > 0 {
            req = req.header("Range", format!("bytes={have}-"));
        }
        let resp = req.call().map_err(network_error)?;

        let mut file = if have > 0 && resp.status().as_u16() == 206 {
            OpenOptions::new().append(true).open(&part).map_err(io_err)?
        } else {
            have = 0;
            File::create(&part).map_err(io_err)?
        };

        let mut reader = resp.into_body().into_reader();
        let mut buf = vec![0u8; 1 << 20];
        let mut last_report = Instant::now();
        on_progress(have, rel.iso_size);
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(CANCELLED.into());
            }
            let n = reader.read(&mut buf).map_err(network_error)?;
            if n == 0 {
                break;
            }
            file.write_all(&buf[..n]).map_err(io_err)?;
            have += n as u64;
            if last_report.elapsed() > Duration::from_millis(150) {
                on_progress(have, rel.iso_size);
                last_report = Instant::now();
            }
        }
        file.flush().map_err(io_err)?;
        on_progress(have, rel.iso_size);
    }

    if have != rel.iso_size {
        return Err("The download didn't finish. Please try again.".into());
    }

    if let Some(url) = &rel.sha256_url {
        on_verify();
        let mut resp = agent()
            .get(url)
            .header("User-Agent", USER_AGENT)
            .call()
            .map_err(network_error)?;
        let text = resp.body_mut().read_to_string().map_err(network_error)?;
        let expected = text.split_whitespace().next().unwrap_or("").to_lowercase();
        let actual = sha256_file(&part, cancel)?;
        if expected.len() == 64 && actual != expected {
            let _ = fs::remove_file(&part);
            return Err("The download was damaged. Please press Try Again to download it again.".into());
        }
    }

    fs::rename(&part, &dest).map_err(io_err)?;
    Ok(dest)
}

fn sha256_file(path: &Path, cancel: &AtomicBool) -> Result<String, String> {
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(CANCELLED.into());
        }
        let n = file.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Removes every other file from the download folder, so old versions
/// don't pile up.
pub fn remove_others(dir: &Path, keep: &Path) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path != keep && path.is_file() {
            let _ = fs::remove_file(path);
        }
    }
}

/// A previously downloaded ISO, for when there is no internet.
pub fn local_iso(dir: &Path, arch: Arch) -> Option<PathBuf> {
    fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(arch.iso_suffix()))
        })
        .max_by_key(|p| fs::metadata(p).and_then(|m| m.modified()).ok())
}

/// "MAN-10-TEST-9-arm64.iso" -> "MAN 10 TEST 9"
pub fn name_from_iso(iso: &Path) -> String {
    let stem = iso.file_stem().and_then(|n| n.to_str()).unwrap_or("MAN");
    let mut name = stem.replace(['-', '_'], " ");
    for suffix in [" amd64", " arm64", " x86 64", " aarch64", " x64"] {
        if name.to_lowercase().ends_with(suffix) {
            name.truncate(name.len() - suffix.len());
            break;
        }
    }
    name
}

/// Checks that a file really is an ISO 9660 disc image.
pub fn check_iso(path: &Path) -> Result<(), String> {
    use std::io::{Seek, SeekFrom};
    let mut file = File::open(path).map_err(|e| format!("The file couldn't be opened: {e}"))?;
    let mut magic = [0u8; 5];
    file.seek(SeekFrom::Start(0x8001))
        .and_then(|_| file.read_exact(&mut magic))
        .ok()
        .filter(|_| &magic == b"CD001")
        .ok_or_else(|| "This file isn't an ISO disc image. Please choose a MAN .iso file.".to_string())
}

pub enum IsoKind {
    Arch(Arch),
    /// For installing on Apple Silicon Macs directly, not for VMs.
    AppleSilicon,
    Unknown,
}

/// Guesses what an ISO is for from its file name.
pub fn iso_kind(path: &Path) -> IsoKind {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if name.contains("apple") {
        IsoKind::AppleSilicon
    } else if name.contains("amd64") || name.contains("x86_64") || name.contains("x64") {
        IsoKind::Arch(Arch::X86_64)
    } else if name.contains("arm64") || name.contains("aarch64") {
        IsoKind::Arch(Arch::Aarch64)
    } else {
        IsoKind::Unknown
    }
}

/// Copies an ISO the person chose into the downloads folder.
pub fn copy_iso(
    src: &Path,
    dir: &Path,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(u64, u64),
) -> Result<PathBuf, String> {
    let io_err = |e: std::io::Error| format!("Couldn't copy the ISO file: {e}");
    fs::create_dir_all(dir).map_err(io_err)?;
    let name = src.file_name().ok_or("That isn't a file.")?;
    let dest = dir.join(name);
    if dest.exists() && fs::canonicalize(src).ok() == fs::canonicalize(&dest).ok() {
        return Ok(dest);
    }

    let part = dir.join(format!("{}.part", name.to_string_lossy()));
    let total = fs::metadata(src).map_err(io_err)?.len();
    let mut input = File::open(src).map_err(io_err)?;
    let mut output = File::create(&part).map_err(io_err)?;
    let mut buf = vec![0u8; 4 << 20];
    let mut done = 0u64;
    let mut last_report = Instant::now();
    loop {
        if cancel.load(Ordering::Relaxed) {
            drop(output);
            let _ = fs::remove_file(&part);
            return Err(CANCELLED.into());
        }
        let n = input.read(&mut buf).map_err(io_err)?;
        if n == 0 {
            break;
        }
        output.write_all(&buf[..n]).map_err(io_err)?;
        done += n as u64;
        if last_report.elapsed() > Duration::from_millis(150) {
            on_progress(done, total);
            last_report = Instant::now();
        }
    }
    output.flush().map_err(io_err)?;
    on_progress(done, total);
    drop(output);
    fs::rename(&part, &dest).map_err(io_err)?;
    Ok(dest)
}
