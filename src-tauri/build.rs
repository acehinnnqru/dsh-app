//! Build script: keep the embedded shell page fresh and identify it.
//!
//! `tauri-build` only emits `rerun-if-changed` for `tauri.conf.json` and
//! `capabilities`, never for `frontendDist`, which lives outside the crate. So
//! editing `ui/` alone would not re-embed it and cargo would reuse a binary
//! carrying the previous page — the watch below is what prevents that.
//!
//! The digest is not a cache key; it is the identity shown in the shell's
//! Runtime section, so a running window can be matched to a source revision.
//! Cache busting is handled by the page's own `no-store` directive, because the
//! asset protocol strips query strings and sets no headers of its own.

use std::path::Path;
use std::process::Command;

/// The shell page files whose contents define the stamped identity.
const UI_FILES: [&str; 5] = [
    "../ui/index.html",
    "../ui/shell.js",
    "../ui/shell-page.js",
    "../ui/tauri.js",
    "../ui/shell.css",
];

fn main() {
    println!("cargo:rerun-if-changed=../ui");
    println!("cargo:rerun-if-changed=presets");
    println!("cargo:rustc-env=DSH_APP_UI_VERSION={}", ui_version());
    copy_seed_presets();
    create_seed_runtime_dir();
    tauri_build::build()
}

/// Copy the sample presets into `OUT_DIR`, where the shell reads them from.
///
/// A compile-time path beats deriving one from the executable: `target/<profile>/`
/// and a bundle's `Contents/MacOS` sit at different depths, and guessing wrong
/// would silently seed nothing.
fn copy_seed_presets() {
    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR is set by cargo");
    let target = Path::new(&out_dir).join("seed-presets");
    let source = Path::new("presets");
    if !source.is_dir() {
        return;
    }
    let _ = std::fs::remove_dir_all(&target);
    if let Err(error) = copy_dir(source, &target) {
        // A missing seed is not fatal; the shell writes its own default preset.
        println!("cargo:warning=could not stage sample presets: {error}");
    }
}

/// Create the resources directory the runtime seed is placed in.
///
/// The closure is tens of thousands of files and hundreds of megabytes, so it is
/// deliberately NOT copied here: `config::seed_runtime_dir` resolves the seed
/// from the bundle's resources at runtime, and staging never has to run for a
/// build. An empty directory keeps the resource declaration valid.
fn create_seed_runtime_dir() {
    // Relative to this crate, not `OUT_DIR`: a bundle resource path is resolved
    // against the manifest directory.
    let dir = Path::new("gen/seed-runtime");
    let _ = std::fs::create_dir_all(dir);
    let marker = dir.join("README.md");
    if !marker.exists() {
        let _ = std::fs::write(
            &marker,
            "# Seed runtime drop point\n\n\
             A built closure (`.dsh-build/runtimes/<id>/`) is copied in here when\n\
             packaging, so the app can boot offline on first run. During development\n\
             the shell reads `.dsh-build/runtimes` directly and this stays empty.\n",
        );
    }
}

/// Recursively copy `from` into `to`.
fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)?.filter_map(Result::ok) {
        let source = entry.path();
        let target = to.join(entry.file_name());
        if source.is_dir() {
            copy_dir(&source, &target)?;
        } else {
            std::fs::copy(&source, &target)?;
        }
    }
    Ok(())
}

/// A content hash of the shell page, or a timestamp when hashing is unavailable.
///
/// `openssl dgst` avoids a build dependency for one digest, and a failure here
/// must not break the build: a timestamp still identifies the build, it is just
/// not content-addressed.
fn ui_version() -> String {
    match Command::new("openssl").args(["dgst", "-sha256", "-r"]).args(UI_FILES).output() {
        Ok(output) if output.status.success() => {
            let text = String::from_utf8_lossy(&output.stdout);
            let digest: String = text
                .lines()
                .filter_map(|line| line.split_whitespace().next())
                .collect::<Vec<_>>()
                .join("");
            let short: String = digest.chars().take(12).collect();
            if short.is_empty() {
                fallback_version()
            } else {
                short
            }
        }
        _ => fallback_version(),
    }
}

/// Seconds since the epoch, used only when the digest is unavailable.
fn fallback_version() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_owned())
}
