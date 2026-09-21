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
    println!("cargo:rustc-env=DSH_APP_UI_VERSION={}", ui_version());
    tauri_build::build()
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
