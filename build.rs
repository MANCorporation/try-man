//! On Windows, puts the striped MAN icon into try-man.exe, so Explorer and the
//! taskbar show it.

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let sizes = ["256x256", "128x128", "64x64", "48x48"];

    // An .ico file holding the PNG icons (Windows Vista and newer read these).
    let pngs: Vec<Vec<u8>> = sizes
        .iter()
        .map(|s| {
            let path = format!("data/icons/hicolor/{s}/apps/io.github.mancorporation.TryMan.png");
            println!("cargo:rerun-if-changed={path}");
            fs::read(&path).expect("app icon PNG missing")
        })
        .collect();
    let mut ico = vec![0, 0, 1, 0];
    ico.extend((pngs.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * pngs.len() as u32;
    for (png, size) in pngs.iter().zip(sizes) {
        let px: u32 = size.split('x').next().unwrap().parse().unwrap();
        ico.extend([(px % 256) as u8, (px % 256) as u8, 0, 0]);
        ico.extend(1u16.to_le_bytes());
        ico.extend(32u16.to_le_bytes());
        ico.extend((png.len() as u32).to_le_bytes());
        ico.extend(offset.to_le_bytes());
        offset += png.len() as u32;
    }
    for png in &pngs {
        ico.extend(png);
    }
    fs::write(out.join("try-man.ico"), ico).unwrap();
    fs::write(out.join("try-man.rc"), "1 ICON \"try-man.ico\"\n").unwrap();

    // Compile the resource with windres (MSYS2) or x86_64-w64-mingw32-windres (cross).
    let res = out.join("try-man-res.o");
    let ok = ["windres", "x86_64-w64-mingw32-windres"].iter().any(|windres| {
        Command::new(windres)
            .current_dir(&out)
            .args(["try-man.rc", "-O", "coff", "-o"])
            .arg(&res)
            .status()
            .is_ok_and(|s| s.success())
    });
    if ok {
        println!("cargo:rustc-link-arg-bins={}", res.display());
    } else {
        println!("cargo:warning=windres not found; try-man.exe will have no icon");
    }
}
