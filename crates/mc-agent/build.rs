//! Embed the application icon into the Windows executable.
//!
//! Two paths are needed:
//!
//! * **GNU toolchain** (`x86_64-pc-windows-gnu`): `windres` compiles the icon
//!   into a COFF object, which is linked in *directly*. Linking it as a static
//!   archive does not work, because Rust links with `--gc-sections` and the
//!   linker then drops the unreferenced `.rsrc` section.
//! * **MSVC toolchain** (what CI uses): `winres` drives `rc.exe` and the
//!   resulting resource object is kept by the MSVC linker.
//!
//! Either way the executable gets a real icon in Explorer and in the system
//! tray.

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=assets/multcontrol.ico");
    println!("cargo:rerun-if-changed=build.rs");

    // The icon is only used on Windows; other targets skip this entirely.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    // The build script runs with the package directory as its cwd. The assets
    // live at the workspace root, two levels up from the package.
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.to_path_buf())
        .unwrap_or_default();
    let icon = root.join("assets").join("multcontrol.ico");

    if !icon.exists() {
        // Generate it if the assets are missing (fresh checkout, clean build).
        let script = root.join("scripts").join("make-icon.py");
        let status = Command::new("python3")
            .arg(&script)
            .arg(root.join("assets"))
            .status();
        if !status.map(|s| s.success()).unwrap_or(false) {
            println!("cargo:warning=could not generate the icon; continuing without it");
            return;
        }
    }
    if !icon.exists() {
        return;
    }

    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if target_env == "gnu" {
        embed_with_windres(&icon);
    } else {
        embed_with_winres(&icon);
    }
}

/// GNU toolchain: compile the resource and link the object directly.
fn embed_with_windres(icon: &Path) {
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));

    let windres = match find_windres() {
        Some(path) => path,
        None => {
            println!("cargo:warning=windres not found; the exe will have no icon");
            return;
        }
    };

    let rc = out_dir.join("multcontrol.rc");
    let rc_text = format!(
        "1 ICON \"{}\"\n1 VERSIONINFO \
         FILEVERSION 0,1,0,0 \
         PRODUCTVERSION 0,1,0,0 \
         FILEOS 0x4 \
         FILETYPE 0x1 \
         BEGIN\n\
         BLOCK \"StringFileInfo\"\n\
         BEGIN\n\
         BLOCK \"040904b0\"\n\
         BEGIN\n\
         VALUE \"ProductName\", \"MultControl\"\n\
         VALUE \"FileDescription\", \"MultControl - share one keyboard and mouse\"\n\
         VALUE \"CompanyName\", \"multcontrol\"\n\
         END\n\
         END\n\
         END\n",
        icon.display()
    );
    if let Err(e) = std::fs::write(&rc, rc_text) {
        println!("cargo:warning=could not write the resource script: {e}");
        return;
    }

    let obj = out_dir.join("multcontrol-resource.o");
    let status = Command::new(&windres)
        .arg(&rc)
        .arg("-o")
        .arg(&obj)
        .status();
    match status {
        Ok(status) if status.success() => {
            // Pass the object file to the linker directly. `rustc-link-obj`
            // is not applied to binary targets, so the object is handed over
            // as a positional linker argument instead, where the linker
            // always keeps its sections.
            println!("cargo:rustc-link-arg-bins={}", obj.display());
        }
        Ok(_) => println!("cargo:warning=windres failed; the exe will have no icon"),
        Err(e) => println!("cargo:warning=could not run windres: {e}"),
    }
}

/// MSVC toolchain: let winres drive `rc.exe`.
fn embed_with_winres(icon: &Path) {
    let mut res = winres::WindowsResource::new();
    res.set_icon(icon.to_string_lossy().as_ref());
    res.set("ProductName", "MultControl");
    res.set(
        "FileDescription",
        "MultControl - share one keyboard and mouse",
    );
    res.set("CompanyName", "multcontrol");
    res.set("LegalCopyright", "multcontrol");
    if let Err(e) = res.compile() {
        println!("cargo:warning=could not embed the Windows icon: {e}");
    }
}

/// Find a resource compiler for the GNU toolchain.
fn find_windres() -> Option<PathBuf> {
    for candidate in [
        "windres",
        "x86_64-w64-mingw32-windres",
        "i686-w64-mingw32-windres",
    ] {
        if let Some(path) = which(candidate) {
            return Some(path);
        }
    }
    None
}

/// Look up an executable on PATH.
fn which(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(program);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}
