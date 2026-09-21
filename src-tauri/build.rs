//! Build script: keep the embedded shell page fresh and identify it.
//!
//! `tauri-build` only emits `rerun-if-changed` for `tauri.conf.json` and
//! `capabilities`, never for `frontendDist`, which lives outside the crate. So
//! editing `ui/` alone would not re-embed it and cargo would reuse a binary
//! carrying the previous page — the watch below is what prevents that.
//!
//! The digest is not a cache key; it is the identity shown in the settings sheet,
//! so a running window can be matched to a source revision. Cache busting is
//! handled by the page's own `no-store` directive, because the asset protocol
//! strips query strings and sets no headers of its own.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn main() {
    println!("cargo:rerun-if-changed=../ui");
    println!("cargo:rustc-env=DSH_APP_UI_VERSION={}", ui_version());
    tauri_build::build()
}

/// Every file under `ui/`, in a stable order.
///
/// Walked rather than listed: the page is several modules now and grows, and a
/// hand-kept list of names silently stops identifying the build the moment
/// somebody adds a file and forgets to add it here.
fn ui_files() -> Vec<PathBuf> {
    fn walk(dir: &Path, into: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, into);
            } else {
                into.push(path);
            }
        }
    }

    let mut files = Vec::new();
    walk(Path::new("../ui"), &mut files);
    files.sort();
    files
}

/// A content hash of the shell page, or a timestamp when hashing is unavailable.
///
/// Every file goes through **one** `openssl` invocation, name and bytes, so the
/// digest covers the whole page. Hashing the files separately and keeping the
/// first twelve characters of the concatenation — which is what this did while
/// the page was one stylesheet and four scripts — reports the first file's prefix
/// and ignores every other file, so a change to the last file would keep the old
/// identity.
///
/// `openssl dgst` avoids a build dependency for one digest, and a failure here
/// must not break the build: a timestamp still identifies the build, it is just
/// not content-addressed.
fn ui_version() -> String {
    let files = ui_files();
    if files.is_empty() {
        return fallback_version();
    }

    let mut child = match Command::new("openssl")
        .args(["dgst", "-sha256", "-r"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => return fallback_version(),
    };

    {
        let Some(stdin) = child.stdin.as_mut() else {
            return fallback_version();
        };
        for path in &files {
            let Ok(bytes) = std::fs::read(path) else {
                return fallback_version();
            };
            // A NUL separator after the name and after the bytes: a rename, and a
            // swap between two files, both change the digest.
            let name = path.to_string_lossy();
            let separator = [0u8; 1];
            for chunk in [
                name.as_bytes(),
                separator.as_slice(),
                bytes.as_slice(),
                separator.as_slice(),
            ] {
                if stdin.write_all(chunk).is_err() {
                    return fallback_version();
                }
            }
        }
    }
    drop(child.stdin.take());

    match child.wait_with_output() {
        Ok(output) if output.status.success() => {
            let text = String::from_utf8_lossy(&output.stdout);
            let digest: String = text
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .chars()
                .take(12)
                .collect();
            if digest.is_empty() {
                fallback_version()
            } else {
                digest
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
