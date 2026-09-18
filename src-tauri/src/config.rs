//! Launch configuration.
//!
//! The packaged app will read presets from `$DSH_HOME/dsh-app/presets/`; during
//! development the same shape is read from a repository-local file so the shell
//! can be exercised before the runtime store (M5) exists.
//!
//! @see docs/design-tauri-wrapper.md D3 (preset), D7 (runtime layout)

use std::path::{Path, PathBuf};

use crate::runtime_store::RuntimeStore;
use crate::settings::Settings;

use serde::{Deserialize, Serialize};

/// A resolved runtime layout on disk.
///
/// Deliberately no longer carries a Node: resolution needs the settings file
/// (`slim` carries no Node at all), so it is done explicitly by the callers that
/// know which context they are in — global startup, or one preset.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RuntimeConfig {
    /// Directory holding `runtime.json`, `package.json`, `node_modules/`.
    pub dir: PathBuf,
}

/// The launch plan the shell needs: where state lives and which runtime to use.
#[derive(Debug, Clone, Serialize)]
pub struct AppConfig {
    /// Where dsh-app keeps its own state (`DSH_HOME` for the child).
    pub home: PathBuf,
    /// The selected runtime.
    pub runtime: RuntimeConfig,
    /// Problems found while loading settings, surfaced in the UI. A corrupt
    /// settings file never stops the shell: it means "unconfigured".
    pub settings_problems: Vec<String>,
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
/// Node resolution is **not** done here: only a preset knows its own `node`
/// override, and a `slim` build may legitimately have none at all
/// (spec §2.1).
pub fn load(home: &Path, store: &RuntimeStore) -> Result<AppConfig, String> {
    let root = repo_root();
    let dir = match store.active_dir() {
        Some(dir) => dir,
        None => newest_runtime_dir(&root.join(".dsh-build").join("runtimes"))?,
    };
    RuntimeManifest::load(&dir)?;
    let settings = crate::settings::load(home);
    let settings_problems = settings.problem.iter().cloned().collect();

    Ok(AppConfig {
        home: home.to_path_buf(),
        runtime: RuntimeConfig { dir },
        settings_problems,
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

/// Where a resolved Node came from.
///
/// Carried so the UI and the logs can say which runtime is in use: an explicit
/// choice, the copy the app carries, and a discovered installation behave
/// differently when something goes wrong, and "which node is it using" is the
/// first question in every such case.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NodeSource {
    /// The preset named it.
    Preset,
    /// `DSH_APP_NODE` named it.
    Environment,
    /// The settings file named it — what the user typed into the shell's form.
    Settings,
    /// The copy the app bundle carries.
    Bundled,
    /// Found at a standard installation location.
    Installed,
}

impl NodeSource {
    /// Stable label for the payload and diagnostics.
    pub fn label(self) -> &'static str {
        match self {
            NodeSource::Preset => "preset",
            NodeSource::Environment => "DSH_APP_NODE",
            NodeSource::Settings => "settings",
            NodeSource::Bundled => "bundled",
            NodeSource::Installed => "installed",
        }
    }
}

/// A Node executable together with where it was found.
#[derive(Debug, Clone)]
pub struct ResolvedNode {
    pub path: PathBuf,
    pub source: NodeSource,
}

/// The outcome of Node resolution.
///
/// `Unconfigured` is a **state, not an error** (spec §1): a `slim` build with no
/// Node named anywhere is waiting for the user to fill in the form, and must not
/// show an error before they have had the chance.
#[derive(Debug, Clone)]
pub enum NodeResolution {
    Configured(ResolvedNode),
    Unconfigured,
}

impl NodeResolution {
    /// The resolved Node, when there is one.
    pub fn node(&self) -> Option<&ResolvedNode> {
        match self {
            NodeResolution::Configured(node) => Some(node),
            NodeResolution::Unconfigured => None,
        }
    }
}

/// Where auto-discovery stands for this build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeOptions {
    /// Release behaviour: never guess (spec §2).
    Explicit,
    /// Development only: also try standard locations and `PATH`, so `cargo run`
    /// works in a devshell without ceremony.
    ///
    /// Consequence to remember when testing: a dev build almost always finds a
    /// Node, so the "unconfigured" flow can only be exercised by a release build.
    Discover,
}

impl NodeOptions {
    /// What this build should do.
    pub fn for_current_build() -> Self {
        if cfg!(debug_assertions) {
            NodeOptions::Discover
        } else {
            NodeOptions::Explicit
        }
    }
}

/// Everything resolution needs besides the explicit per-preset value.
pub struct NodeContext<'a> {
    pub settings: &'a Settings,
    pub options: NodeOptions,
    /// A development checkout's staged Node installs, searched at level 4
    /// alongside the bundle's copy. Empty for a packaged app.
    pub staged: &'a [PathBuf],
}

/// Resolve the Node executable for one runtime directory.
///
/// Precedence, highest first (spec §2):
///
/// 1. the preset's `node` — the most specific scope;
/// 2. `DSH_APP_NODE` — one session;
/// 3. `settings.json` — this machine;
/// 4. the Node the app bundle carries, when it carries one;
/// 5. *development builds only*: a standard installation location, then `PATH`.
///
/// "More specific scope wins" is what makes a per-preset override usable at all:
/// if the settings file outranked the preset, the override could never take
/// effect once the form had been filled in.
///
/// A value that was named but is unusable (missing, not executable) is an error
/// rather than a reason to fall through: silently launching a different Node than
/// the one that was asked for is worse than saying so.
pub fn resolve_node(
    explicit: Option<&str>,
    context: &NodeContext<'_>,
) -> Result<NodeResolution, String> {
    // 1. The preset's own choice.
    if let Some(chosen) = non_empty(explicit) {
        let path = PathBuf::from(chosen);
        return match usable(&path) {
            true => Ok(NodeResolution::Configured(ResolvedNode {
                path,
                source: NodeSource::Preset,
            })),
            false => Err(format!(
                "the preset's node is not an executable file: {}\n\
                 fix the preset's `node` setting, or remove it to use the app's own runtime",
                path.display()
            )),
        };
    }

    // 2. The session.
    if let Ok(from_env) = std::env::var("DSH_APP_NODE") {
        if let Some(chosen) = non_empty(Some(&from_env)) {
            let path = PathBuf::from(chosen);
            return match usable(&path) {
                true => Ok(NodeResolution::Configured(ResolvedNode {
                    path,
                    source: NodeSource::Environment,
                })),
                false => Err(format!(
                    "DSH_APP_NODE is not an executable file: {}",
                    path.display()
                )),
            };
        }
    }

    // 3. What the user typed into the shell's form.
    if let Some(chosen) = non_empty(context.settings.node.as_deref()) {
        let path = PathBuf::from(chosen);
        return match usable(&path) {
            true => Ok(NodeResolution::Configured(ResolvedNode {
                path,
                source: NodeSource::Settings,
            })),
            false => Err(format!(
                "settings.json names a node that is not an executable file: {}\n\
                 re-enter it in the shell's Node path field",
                path.display()
            )),
        };
    }

    // 4. The copy the app carries — this is what makes `bundled` zero-config —
    // then a development checkout's staged install.
    for candidate in bundled_node_candidates().iter().chain(context.staged) {
        if usable(candidate) {
            return Ok(NodeResolution::Configured(ResolvedNode {
                path: candidate.clone(),
                source: NodeSource::Bundled,
            }));
        }
    }

    // 5. Development conveniences, never present in a release build: a macOS app
    // started from Finder does not inherit the login shell's environment, so
    // "look on PATH" would choose from an environment the user never set up.
    if context.options == NodeOptions::Discover {
        for candidate in installed_node_candidates() {
            if usable(&candidate) {
                return Ok(NodeResolution::Configured(ResolvedNode {
                    path: candidate,
                    source: NodeSource::Installed,
                }));
            }
        }
    }

    Ok(NodeResolution::Unconfigured)
}

/// The message shown when nothing is configured. Phrased as guidance, because
/// that is what it is: the form is the answer, not an error dialog.
pub fn unconfigured_message() -> String {
    "no Node runtime is configured.\n\
     \n\
     A `slim` build carries none: name the Node this app should use in the shell's\n\
     Node path field, or set DSH_APP_NODE=/path/to/node for one session.\n\
     \n\
     Use a Node from nodejs.org: a distribution-repackaged build can load the\n\
     require-builtin addon and then refuse every id, which breaks live reload."
        .to_owned()
}

/// The Node the running executable's bundle carries, if any.
fn bundled_node_candidates() -> Vec<PathBuf> {
    let Ok(exe) = std::env::current_exe() else {
        return Vec::new();
    };
    let Some(dir) = exe.parent() else {
        return Vec::new();
    };
    // It sits in Resources rather than MacOS because `Contents/MacOS` is reserved
    // for the shell executable (design doc A.3).
    vec![dir.join("../Resources/node/bin/node")]
}

/// Node installations staged for a development checkout.
pub fn staged_candidates(root: &Path) -> Vec<PathBuf> {
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
    candidates
}

/// Standard locations and `PATH`, for development builds only.
fn installed_node_candidates() -> Vec<PathBuf> {
    let mut candidates: Vec<PathBuf> = ["/opt/homebrew/bin/node", "/usr/local/bin/node", "/usr/bin/node"]
        .iter()
        .map(PathBuf::from)
        .collect();
    if let Ok(path) = std::env::var("PATH") {
        candidates.extend(std::env::split_paths(&path).map(|dir| dir.join("node")));
    }
    candidates
}

/// A non-empty, trimmed value.
fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

/// Whether this path can actually be launched.
///
/// `is_file()` alone is not enough: a plain file without the execute bit, or a
/// non-Node binary, would pass it and fail much later with a confusing message
/// (spec §3.1).
pub fn usable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
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

#[cfg(test)]
mod tests {
    use super::*;

    /// An executable scratch file, so `usable` has something real to look at.
    fn executable(dir: &Path, name: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let path = dir.join(name);
        std::fs::write(&path, "#!/bin/sh\nexit 0\n").expect("the file is writable");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod works");
        path
    }

    fn scratch(name: &str) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the clock is after the epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("dsh-app-config-{name}-{}-{unique}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("the scratch directory is creatable");
        dir
    }

    #[test]
    fn nothing_configured_is_a_state_and_not_an_error() {
        let settings = Settings::default();
        let empty = scratch("unconfigured");
        let context = NodeContext {
            settings: &settings,
            // Release behaviour: no discovery, so the outcome cannot depend on the
            // machine running the test.
            options: NodeOptions::Explicit,
            staged: &[],
        };
        let resolution = resolve_node(None, &context).expect("unconfigured is not a failure");
        assert!(resolution.node().is_none());
        let _ = std::fs::remove_dir_all(&empty);
    }

    #[test]
    fn a_staged_checkout_node_satisfies_level_four() {
        let dir = scratch("staged");
        let node = executable(&dir, "node");
        let settings = Settings::default();
        let staged = vec![node.clone()];
        let context = NodeContext {
            settings: &settings,
            options: NodeOptions::Explicit,
            staged: &staged,
        };
        let resolved = resolve_node(None, &context).expect("a staged node resolves").node().cloned();
        assert_eq!(resolved.map(|node| node.path), Some(node));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_configured_node_outranks_the_bundle() {
        let dir = scratch("settings");
        let node = executable(&dir, "node");
        let settings = Settings {
            schema: None,
            node: Some(node.display().to_string()),
            pnpm: None,
        };
        let context = NodeContext {
            settings: &settings,
            options: NodeOptions::Explicit,
            staged: &[],
        };
        let resolved = resolve_node(None, &context).expect("a configured node resolves");
        let resolved = resolved.node().expect("it is configured");
        assert_eq!(resolved.path, node);
        assert_eq!(resolved.source, NodeSource::Settings);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_named_but_unusable_path_is_an_error_not_a_fallthrough() {
        let dir = scratch("unusable");
        // Exists, but nothing may execute it: silently using a different Node
        // would be worse than saying so.
        let path = dir.join("node");
        std::fs::write(&path, "not executable").expect("the file is writable");
        let settings = Settings {
            schema: None,
            node: Some(path.display().to_string()),
            pnpm: None,
        };
        let context = NodeContext {
            settings: &settings,
            options: NodeOptions::Explicit,
            staged: &[],
        };
        let error = resolve_node(None, &context).expect_err("a non-executable path is refused");
        assert!(error.contains("not an executable file"), "{error}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_per_preset_choice_outranks_everything_else() {
        let dir = scratch("preset");
        let node = executable(&dir, "node");
        let settings = Settings {
            schema: None,
            node: Some("/nonexistent/from-settings".to_owned()),
            pnpm: None,
        };
        let context = NodeContext {
            settings: &settings,
            options: NodeOptions::Explicit,
            staged: &[],
        };
        let resolved = resolve_node(Some(&node.display().to_string()), &context)
            .expect("the preset's node resolves")
            .node()
            .cloned()
            .expect("it is configured");
        assert_eq!(resolved.source, NodeSource::Preset);
        assert_eq!(resolved.path, node);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
