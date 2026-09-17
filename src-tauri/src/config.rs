//! Launch configuration.
//!
//! The packaged app will read presets from `$DSH_HOME/dsh-app/presets/`; during
//! development the same shape is read from a repository-local file so the shell
//! can be exercised before the runtime store (M5) exists.
//!
//! @see docs/design-tauri-wrapper.md D3 (preset), D7 (runtime layout)

use std::path::{Path, PathBuf};

use crate::runtime_store::RuntimeStore;

use serde::{Deserialize, Serialize};

/// A resolved runtime layout on disk.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RuntimeConfig {
    /// Directory holding `runtime.json`, `package.json`, `node_modules/`.
    pub dir: PathBuf,
    /// The Node executable to run the closure with.
    pub node: PathBuf,
}

/// The launch plan the shell needs: where state lives and which runtime to use.
#[derive(Debug, Clone, Serialize)]
pub struct AppConfig {
    /// Where dsh-app keeps its own state (`DSH_HOME` for the child).
    pub home: PathBuf,
    /// The selected runtime.
    pub runtime: RuntimeConfig,
}

/// `runtime.json` as written by `scripts/prepare-runtime.mjs`.
#[derive(Debug, Clone, Deserialize)]
pub struct RuntimeManifest {
    pub schema: u32,
    pub channel: String,
    #[serde(rename = "dshVersion")]
    pub dsh_version: String,
    pub platform: String,
    pub entry: RuntimeEntry,
    pub node: RuntimeNode,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RuntimeEntry {
    pub kind: String,
    pub path: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RuntimeNode {
    pub requirement: String,
}

impl RuntimeManifest {
    /// Read and validate `<dir>/runtime.json`.
    pub fn load(dir: &Path) -> Result<Self, String> {
        let path = dir.join("runtime.json");
        let text = std::fs::read_to_string(&path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        let manifest: Self = serde_json::from_str(&text)
            .map_err(|error| format!("cannot parse {}: {error}", path.display()))?;
        if manifest.schema != 1 {
            return Err(format!(
                "{}: unsupported runtime.json schema {}",
                path.display(),
                manifest.schema
            ));
        }
        Ok(manifest)
    }

    /// Absolute path of the launcher entry inside the closure.
    pub fn entry_path(&self, dir: &Path) -> PathBuf {
        dir.join(&self.entry.path)
    }
}

/// The repository root, derived from the built binary's location so the shell
/// works both from `target/debug` and from a bundle.
pub fn repo_root() -> PathBuf {
    // src-tauri/target/<profile>/dsh-app -> repo root is four levels up.
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("."));
    let mut candidate = exe.as_path();
    for _ in 0..4 {
        if let Some(parent) = candidate.parent() {
            candidate = parent;
        }
    }
    candidate.to_path_buf()
}

/// The shell's home: where its presets and the runtime's `DSH_HOME` live.
///
/// Public because startup needs it before any configuration can be loaded: the
/// runtime store lives here, and the store is what the configuration is built
/// from.
///
/// `DSH_APP_HOME` overrides it, which is what makes an isolated run possible
/// (`DSH_APP_HOME=$(mktemp -d) dsh-app`) and lets a developer point the shell at
/// their real `~/.dsh` to share configuration with the CLI. Otherwise the
/// repository-local default keeps sessions out of the user's own harness home.
pub fn resolve_home(root: &Path) -> PathBuf {
    match std::env::var("DSH_APP_HOME") {
        Ok(value) if !value.trim().is_empty() => PathBuf::from(value),
        _ => root.join(".dsh-build").join("home"),
    }
}

/// Resolve the shell's configuration.
///
/// The runtime is served from the store under the shell's home (design doc D10):
/// the app bundle carries only a seed, and updates replace the store copy. An
/// already-populated store therefore wins, and `.dsh-build/runtimes` is used
/// only until the first seed has been installed — which keeps a fresh checkout
/// working with nothing but `pnpm runtime:prepare`.
///
/// Node resolution follows D2's precedence: `DSH_APP_NODE`, then the bundled
/// official archive, then `PATH`.
pub fn load(home: &Path, store: &RuntimeStore) -> Result<AppConfig, String> {
    let root = repo_root();
    let dir = match store.active_dir() {
        Some(dir) => dir,
        None => newest_runtime_dir(&root.join(".dsh-build").join("runtimes"))?,
    };
    let manifest = RuntimeManifest::load(&dir)?;
    let node = resolve_node(&root, &manifest)?;

    Ok(AppConfig {
        home: home.to_path_buf(),
        runtime: RuntimeConfig { dir, node },
    })
}

/// The first-run seed runtime the bundle carries, if any.
///
/// Resolved at runtime rather than staged into `OUT_DIR`, because the closure is
/// hundreds of megabytes: copying it on every build would dominate build time,
/// and a build-script copy would silently go stale whenever
/// `pnpm runtime:prepare` rebuilt it. Candidate layouts, in order:
///
/// * a packaged app — `<bundle>/Contents/Resources/runtimes/<id>/`
/// * a development build — `<repo>/.dsh-build/runtimes/<id>/`
pub fn seed_runtime_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let beside_exe = exe.parent()?.join("seed-runtime");
    let resources = exe.parent()?.join("../Resources/runtimes");
    let develop = repo_root().join(".dsh-build").join("runtimes");
    for candidate in [beside_exe, resources, develop] {
        if let Some(seed) = newest_runtime_dir(&candidate).ok() {
            return Some(seed);
        }
    }
    None
}

/// Resolve the Node executable for one runtime directory.
///
/// Public so the activation smoke test can launch a candidate that is not yet
/// the active runtime.
pub fn resolve_node_for(root: &Path, dir: &Path) -> Result<PathBuf, String> {
    let manifest = RuntimeManifest::load(dir)?;
    resolve_node(root, &manifest)
}

/// The Node version the shell bundles, read from the root manifest's pin.
fn bundled_node_version(root: &Path) -> Option<String> {
    let manifest = std::fs::read_to_string(root.join("package.json")).ok()?;
    let manifest: serde_json::Value = serde_json::from_str(&manifest).ok()?;
    Some(manifest.get("dshRuntime")?.get("nodeVersion")?.as_str()?.to_owned())
}

/// The newest prepared runtime by directory name.
fn newest_runtime_dir(runtimes: &Path) -> Result<PathBuf, String> {
    let mut candidates: Vec<PathBuf> = std::fs::read_dir(runtimes)
        .map_err(|error| {
            format!(
                "no prepared runtime under {}: {error}\nrun `pnpm runtime:prepare` first",
                runtimes.display()
            )
        })?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.join("runtime.json").is_file())
        .collect();
    candidates.sort();
    candidates
        .pop()
        .ok_or_else(|| format!("no prepared runtime under {}\nrun `pnpm runtime:prepare` first", runtimes.display()))
}

/// Resolve the Node executable per D2's precedence.
fn resolve_node(root: &Path, manifest: &RuntimeManifest) -> Result<PathBuf, String> {
    if let Ok(explicit) = std::env::var("DSH_APP_NODE") {
        if !explicit.is_empty() {
            let path = PathBuf::from(explicit);
            if path.is_file() {
                return Ok(path);
            }
            return Err(format!("DSH_APP_NODE is not a file: {}", path.display()));
        }
    }

    // The bundled official archive: an explicit version first (the pin in
    // package.json), then whatever else has been staged. Official builds are the
    // only ones `node-addon-require-builtin` supports, so this level outranks PATH.
    let staged = root.join(".dsh-build").join("node");
    let mut candidates = Vec::new();
    if let Some(version) = bundled_node_version(root) {
        candidates.push(staged.join(&version).join("bin").join("node"));
    }
    if let Ok(entries) = std::fs::read_dir(&staged) {
        for entry in entries.flatten() {
            let candidate = entry.path().join("bin").join("node");
            if !candidates.contains(&candidate) {
                candidates.push(candidate);
            }
        }
    }
    for candidate in candidates {
        if candidate.is_file() {
            return Ok(candidate);
        }
    }

    // Fall back to PATH so a plain `cargo run` works in the devshell.
    if let Ok(path) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join("node");
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }

    Err(format!(
        "no Node executable found for runtime {} ({}); set DSH_APP_NODE or run `pnpm runtime:prepare`",
        manifest.dsh_version, manifest.node.requirement
    ))
}
