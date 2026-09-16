//! The preset store: user-selectable launch configurations.
//!
//! A preset is a thin wrapper. Every field it carries maps onto an existing
//! dsh concept — `profile` → `--profile`, `patches` → `--patch`, `env` → the
//! child environment — so dsh-app never invents a second configuration
//! language. Presets live in `$HOME/dsh-app/presets/<id>.toml`.
//!
//! @see docs/design-tauri-wrapper.md D3

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::Preset;

/// Directory holding preset files, under the shell's own home.
pub const PRESETS_DIR: &str = "dsh-app/presets";

/// The preset file written on first run so a fresh install is self-explanatory.
const DEFAULT_PRESET_ID: &str = "dsh-app";

/// One preset as stored on disk.
#[derive(Debug, Clone, Deserialize, Serialize)]
struct StoredPreset {
    id: String,
    #[serde(default)]
    label: Option<String>,
    #[serde(default = "default_kind")]
    kind: String,
    #[serde(default)]
    profile: Option<String>,
    #[serde(default)]
    bundles: Option<Vec<String>>,
    #[serde(default)]
    patch_reload: Option<String>,
    /// Extra patch layers, resolved relative to the preset file.
    #[serde(default)]
    patches: Option<Vec<String>>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    /// Positional job argument, used by `kind = "task"` (not implemented yet).
    #[serde(default)]
    job: Option<String>,
}

fn default_kind() -> String {
    "web".to_owned()
}

/// A preset plus the directory its relative paths resolve against.
#[derive(Debug, Clone)]
pub struct ResolvedPreset {
    pub preset: Preset,
    /// Absolute patch files, in application order.
    pub patch_files: Vec<PathBuf>,
    /// The preset file this came from.
    pub source: PathBuf,
}

/// Load every preset from `<home>/dsh-app/presets/`, writing the default first.
///
/// A malformed preset file is reported rather than skipped: a silently ignored
/// preset would look like the shell lost the user's configuration.
///
/// `seed` is a directory to copy in on first run (the packaged app seeds its
/// own sample presets this way). Existing files are never overwritten, so a
/// seeded preset stops being ours the moment the user edits it.
pub fn load_all(home: &Path, seed: Option<&Path>) -> (Vec<ResolvedPreset>, Vec<String>) {
    let dir = home.join(PRESETS_DIR);
    let mut problems = Vec::new();
    if let Err(error) = std::fs::create_dir_all(&dir) {
        problems.push(format!("cannot create {}: {error}", dir.display()));
        return (Vec::new(), problems);
    }
    if let Some(seed) = seed {
        if let Err(error) = seed_missing(seed, &dir) {
            problems.push(error);
        }
    }

    let default_path = dir.join(format!("{DEFAULT_PRESET_ID}.toml"));
    if !default_path.exists() {
        if let Err(error) = std::fs::write(&default_path, default_preset_text()) {
            problems.push(format!("cannot write {}: {error}", default_path.display()));
        }
    }

    let mut entries: Vec<PathBuf> = match std::fs::read_dir(&dir) {
        Ok(entries) => entries
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "toml"))
            .collect(),
        Err(error) => {
            problems.push(format!("cannot read {}: {error}", dir.display()));
            return (Vec::new(), problems);
        }
    };
    entries.sort();

    let mut presets = Vec::new();
    for path in entries {
        match load_one(&path) {
            Ok(preset) => presets.push(preset),
            Err(error) => problems.push(error),
        }
    }
    (presets, problems)
}

/// Load and resolve one preset file.
pub fn load_one(path: &Path) -> Result<ResolvedPreset, String> {
    let text = std::fs::read_to_string(path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let stored: StoredPreset = toml::from_str(&text).map_err(|error| format!("cannot parse {}: {error}", path.display()))?;
    if stored.id.trim().is_empty() {
        return Err(format!("{}: preset id must not be empty", path.display()));
    }
    let base = path.parent().unwrap_or(Path::new("."));
    let patch_files = stored
        .patches
        .unwrap_or_default()
        .into_iter()
        .map(|entry| {
            let candidate = PathBuf::from(&entry);
            if candidate.is_absolute() {
                candidate
            } else {
                base.join(candidate)
            }
        })
        .collect();

    Ok(ResolvedPreset {
        preset: Preset {
            id: stored.id,
            label: stored.label,
            kind: stored.kind,
            profile: stored.profile,
            bundles: stored.bundles,
            patch_reload: stored.patch_reload,
            env: stored.env,
        },
        patch_files,
        source: path.to_path_buf(),
    })
}

/// Copy seed entries that do not exist yet, recursively.
///
/// Presets may reference patch files beside themselves, so the whole tree is
/// mirrored and not just the `.toml` files.
fn seed_missing(from: &Path, to: &Path) -> Result<(), String> {
    let entries = match std::fs::read_dir(from) {
        Ok(entries) => entries,
        // No seed directory is not a problem: a packaged app may ship none.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("cannot read the preset seed {}: {error}", from.display())),
    };
    for entry in entries.filter_map(Result::ok) {
        let source = entry.path();
        let target = to.join(entry.file_name());
        if target.exists() {
            continue;
        }
        if source.is_dir() {
            std::fs::create_dir_all(&target)
                .map_err(|error| format!("cannot create {}: {error}", target.display()))?;
            seed_missing(&source, &target)?;
            continue;
        }
        std::fs::copy(&source, &target)
            .map_err(|error| format!("cannot seed {}: {error}", target.display()))?;
    }
    Ok(())
}

/// The default preset's contents, written on first run.
fn default_preset_text() -> String {
    format!(
        "# One dsh-app preset. Every field maps onto an existing dsh concept:\n\
         #   profile -> `--profile`   patches -> `--patch`   env -> the child environment\n\
         #\n\
         # kind = \"web\"      the served GUI opens in its own window (implemented)\n\
         # kind = \"task\"     one-shot; `job` is the positional argument (not implemented)\n\
         # kind = \"server\"   long-lived stdio, no window (not implemented)\n\
         #\n\
         # patch_reload = \"startup\" needs nothing extra; \"live\" adds the HMR row and\n\
         # therefore requires internals access (`--expose-internals` or the native addon).\n\
         \n\
         id = \"{DEFAULT_PRESET_ID}\"\n\
         label = \"Default\"\n\
         kind = \"web\"\n\
         profile = \"{DEFAULT_PRESET_ID}\"\n\
         patch_reload = \"startup\"\n\
         \n\
         # Bundles are written into the profile only on its first materialization;\n\
         # after that the profile directory belongs to you.\n\
         bundles = [\"@deepseek-ai/dsh-base\", \"@deepseek-ai/dsh-web-app\"]\n\
         \n\
         # Extra patch layers, applied in order on top of every bundle and profile\n\
         # layer (this is dsh's `--patch`, so a layer wins over the layers below it).\n\
         # Each file is a top-level YAML array of id-targeted patch entries, e.g.\n\
         #   - id: webserver\n\
         #     config: {{ host: 127.0.0.1, port: 45671 }}\n\
         # NOTE: `config` REPLACES the target row's whole config object; it is not a\n\
         # deep merge, so a patch must repeat every required field of that row.\n\
         # patches = [\"patch.d/10-example.yml\"]\n\
         \n\
         [env]\n"
    )
}
