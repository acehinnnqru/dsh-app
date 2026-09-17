//! The release channel: where a newer runtime comes from, and how it is applied.
//!
//! The runtime lives in the user's own directory and is replaced there, so
//! "updating dsh" is an ordinary file operation the shell performs — no relaunch,
//! no re-notarization (design doc D10). What that buys in convenience it costs in
//! verification: nothing else proves the new runtime boots, so `check` and
//! `update` are deliberately separate, and `update` gates activation on a smoke
//! test (D8.2 U1).
//!
//! A channel publishes one small JSON document:
//!
//! ```json
//! {
//!   "channel": "release",
//!   "releases": [
//!     {
//!       "version": "0.1.6",
//!       "platform": "darwin-arm64",
//!       "tarball": "https://…/dsh-runtime-0.1.6-darwin-arm64.tar.gz",
//!       "sha256": "…",
//!       "shellContract": 1
//!     }
//!   ]
//! }
//! ```
//!
//! Per-platform entries rather than one archive: the closure resolves native
//! packages (`node-pty`, `koffi`, the `require-builtin` addon) for the machine
//! that builds it, so shipping a single archive would ship one platform's
//! binaries to every platform. Resolving them on the user's machine instead
//! would mean shipping pnpm and a store, which is exactly what the expanded
//! closure avoids.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::runtime_store::RuntimeStore;

/// The shell contract this build provides. A release may require no more than
/// this; anything higher needs a newer shell (design doc D6).
pub const SHELL_CONTRACT: u32 = 1;

/// One published runtime.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Release {
    /// dsh version this runtime carries.
    pub version: String,
    /// `<os>-<arch>`, matching the runtime manifest's `platform`.
    pub platform: String,
    /// Archive holding the closure.
    pub tarball: String,
    /// Expected sha256 of the archive, lowercase hex.
    pub sha256: String,
    /// Shell contract this runtime requires, when it declares one.
    #[serde(default, rename = "shellContract")]
    pub shell_contract: Option<u32>,
}

/// A channel's published releases.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ChannelDocument {
    /// Channel name, for diagnostics only.
    #[serde(default)]
    pub channel: Option<String>,
    /// Every published runtime.
    #[serde(default)]
    pub releases: Vec<Release>,
}

/// What an update check found.
#[derive(Debug, Clone, Serialize)]
pub struct UpdateCheck {
    /// The active version, when a runtime is installed.
    pub current: Option<String>,
    /// The newest release this platform can use, when one exists.
    pub available: Option<String>,
    /// The tarball of that release.
    pub tarball: Option<String>,
    /// Whether `available` differs from `current`.
    pub update_available: bool,
    /// Why nothing is available, when that is the case.
    pub detail: Option<String>,
}

/// The platform tag releases are published under.
///
/// Node's spelling (`darwin-arm64`), not Rust's (`macos-aarch64`): the same tag
/// appears in `runtime.json`, in the official Node archive names, and in the
/// packaging scripts, and a second convention would silently stop matching.
pub fn platform_tag() -> String {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        other => other,
    };
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        other => other,
    };
    format!("{os}-{arch}")
}

/// Fetch and parse one channel document.
pub fn fetch_document(url: &str) -> Result<ChannelDocument, String> {
    let document = if let Some(path) = url.strip_prefix("file://") {
        // A local document is what a test or a staged rollout uses; it is also
        // how this path can be exercised without publishing anything.
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("cannot read {path}: {error}"))?;
        serde_json::from_str(&text).map_err(|error| format!("cannot parse {path}: {error}"))?
    } else {
        let response = http_get_text(url)?;
        serde_json::from_str(&response).map_err(|error| format!("cannot parse {url}: {error}"))?
    };
    Ok(document)
}

/// The newest release for this platform that this shell can run.
///
/// Version comparison is a simple dotted-numeric ordering: channel documents list
/// published artifacts, and the harness version scheme is numeric with optional
/// pre-release suffixes. A suffix makes a version *older* than the same numbers
/// without one, which is what `-rc.N` means everywhere else in this project.
pub fn newest_for_platform<'a>(document: &'a ChannelDocument, platform: &str) -> Option<&'a Release> {
    let mut candidates: Vec<&Release> = document
        .releases
        .iter()
        .filter(|release| release.platform == platform)
        .filter(|release| release.shell_contract.unwrap_or(SHELL_CONTRACT) <= SHELL_CONTRACT)
        .collect();
    candidates.sort_by(|left, right| compare_versions(&right.version, &left.version));
    candidates.into_iter().next()
}

/// Order two versions: numeric components first, then a release outranks its
/// pre-releases.
fn compare_versions(left: &str, right: &str) -> std::cmp::Ordering {
    let split = |value: &str| -> (Vec<u64>, Option<String>) {
        let (numbers, suffix) = match value.split_once('-') {
            Some((numbers, suffix)) => (numbers, Some(suffix.to_owned())),
            None => (value, None),
        };
        let parsed = numbers
            .split('.')
            .map(|part| part.parse::<u64>().unwrap_or(0))
            .collect();
        (parsed, suffix)
    };
    let (left_numbers, left_suffix) = split(left);
    let (right_numbers, right_suffix) = split(right);
    match left_numbers.cmp(&right_numbers) {
        std::cmp::Ordering::Equal => match (left_suffix, right_suffix) {
            (None, None) => std::cmp::Ordering::Equal,
            // A plain release is newer than any pre-release of the same numbers.
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (Some(_), None) => std::cmp::Ordering::Less,
            (Some(left), Some(right)) => left.cmp(&right),
        },
        other => other,
    }
}

/// Check the channel against what is installed.
pub fn check(store: &RuntimeStore, url: &str) -> Result<UpdateCheck, String> {
    let platform = platform_tag();
    let document = fetch_document(url)?;
    let current = store.status().active_dsh_version;
    match newest_for_platform(&document, &platform) {
        Some(release) => Ok(UpdateCheck {
            current: current.clone(),
            available: Some(release.version.clone()),
            tarball: Some(release.tarball.clone()),
            update_available: current.as_deref() != Some(release.version.as_str()),
            detail: None,
        }),
        None => Ok(UpdateCheck {
            current,
            available: None,
            tarball: None,
            update_available: false,
            detail: Some(format!("no release is published for {platform}")),
        }),
    }
}

/// What one update produced.
#[derive(Debug, Clone, Serialize)]
pub struct UpdateOutcome {
    /// The version that is now active.
    pub activated: String,
    /// The URL the smoke test reached, token redacted.
    pub smoke_url: Option<String>,
    /// The version this replaced, available for rollback.
    pub replaced: Option<String>,
}

/// Download, verify, smoke-test, and activate the newest release.
///
/// The order is the point: nothing touches the active runtime until a smoke test
/// has booted the candidate (design doc D8.2 U1). A failure here leaves the
/// installation exactly as it was and reports why.
pub fn update(
    store: &RuntimeStore,
    url: &str,
    node: &Path,
    smoke_home: &Path,
    node_args: Vec<String>,
) -> Result<UpdateOutcome, String> {
    let platform = platform_tag();
    let document = fetch_document(url)?;
    let release = newest_for_platform(&document, &platform)
        .ok_or_else(|| format!("no release is published for {platform}"))?
        .clone();

    let id = format!("{}-{}", release.version, platform);
    let installed_versions = store.installed_versions();
    if installed_versions.iter().any(|name| *name == id) {
        // Already downloaded; activation is still worth running because the
        // active version may have been rolled back since.
        eprintln!("[dsh-app] runtime {id} is already downloaded");
    } else {
        let archive = store.root().join(format!(".download-{id}.tar.gz"));
        download(&release.tarball, &archive)?;
        let digest = sha256_file(&archive)?;
        if digest != release.sha256.to_lowercase() {
            let _ = std::fs::remove_file(&archive);
            return Err(format!(
                "the downloaded runtime does not match its published digest\n  expected {}\n  actual   {digest}",
                release.sha256
            ));
        }
        let staging = store.root().join(format!(".unpack-{id}"));
        let _ = std::fs::remove_dir_all(&staging);
        std::fs::create_dir_all(&staging)
            .map_err(|error| format!("cannot create {}: {error}", staging.display()))?;
        extract(&archive, &staging)?;
        let _ = std::fs::remove_file(&archive);
        // `install` copies from here and validates the manifest, so a malformed
        // archive is rejected before it can be activated.
        let source = find_runtime_root(&staging)?;
        // The published archive is platform-specific, so a mismatch means the
        // channel served the wrong artifact — cheaper to catch here than as an
        // ABI failure on first boot.
        let manifest = crate::config::RuntimeManifest::load(&source)?;
        if manifest.platform != platform {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(format!(
                "the published runtime is for {} but this machine is {platform}",
                manifest.platform
            ));
        }
        store.install(&id, &source)?;
        let _ = std::fs::remove_dir_all(&staging);
    }

    let dir = store.root().join(&id);
    let report = crate::smoke::smoke(&dir, node, smoke_home, node_args);
    if !report.ok {
        return Err(format!(
            "the downloaded runtime failed its smoke test and was not activated: {}",
            report.detail.unwrap_or_else(|| "unknown reason".to_owned())
        ));
    }

    let replaced = store.status().active;
    store.activate(&id)?;
    Ok(UpdateOutcome {
        activated: id,
        smoke_url: report.url,
        replaced,
    })
}

/// Read a URL into a file.
fn download(url: &str, target: &Path) -> Result<(), String> {
    if let Some(path) = url.strip_prefix("file://") {
        std::fs::copy(path, target)
            .map_err(|error| format!("cannot copy {path}: {error}"))?;
        return Ok(());
    }
    let bytes = http_get_bytes(url)?;
    std::fs::write(target, bytes).map_err(|error| format!("cannot write {}: {error}", target.display()))
}

/// Minimal HTTPS GET returning text.
///
/// Shells out to `curl` rather than taking an HTTP client dependency: the shell
/// needs one GET, the runtime already depends on the system's TLS trust store,
/// and a static binary is not a goal here.
fn http_get_text(url: &str) -> Result<String, String> {
    let bytes = http_get_bytes(url)?;
    String::from_utf8(bytes).map_err(|error| format!("{url} is not UTF-8: {error}"))
}

/// Minimal GET returning bytes.
fn http_get_bytes(url: &str) -> Result<Vec<u8>, String> {
    let output = std::process::Command::new("curl")
        .args(["--fail", "--silent", "--show-error", "--location", url])
        .output()
        .map_err(|error| format!("cannot run curl: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "GET {url} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(output.stdout)
}

/// sha256 of a file.
fn sha256_file(path: &Path) -> Result<String, String> {
    // `shasum` ships with macOS and every Linux base image this targets.
    let output = std::process::Command::new("shasum")
        .args(["-a", "256"])
        .arg(path)
        .output()
        .map_err(|error| format!("cannot run shasum: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "cannot hash {}: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    text.split_whitespace()
        .next()
        .map(|digest| digest.to_lowercase())
        .ok_or_else(|| format!("shasum printed nothing for {}", path.display()))
}

/// Extract a `.tar.gz` into `target`.
fn extract(archive: &Path, target: &Path) -> Result<(), String> {
    let output = std::process::Command::new("tar")
        .arg("xzf")
        .arg(archive)
        .arg("-C")
        .arg(target)
        .output()
        .map_err(|error| format!("cannot run tar: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "cannot extract {}: {}",
            archive.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
}

/// The runtime inside an extracted archive: either the archive root or a single
/// directory holding it.
fn find_runtime_root(staging: &Path) -> Result<PathBuf, String> {
    if staging.join("runtime.json").is_file() {
        return Ok(staging.to_path_buf());
    }
    let entries = std::fs::read_dir(staging)
        .map_err(|error| format!("cannot read {}: {error}", staging.display()))?;
    for entry in entries.filter_map(Result::ok) {
        let candidate = entry.path();
        if candidate.join("runtime.json").is_file() {
            return Ok(candidate);
        }
    }
    Err(format!(
        "the archive has no runtime.json at its root or one level below ({})",
        staging.display()
    ))
}
