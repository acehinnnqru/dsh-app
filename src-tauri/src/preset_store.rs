//! The preset store: user-selectable launch configurations.
//!
//! A preset is a thin wrapper. Every field it carries maps onto an existing
//! dsh concept — `profile` → `--profile`, `patches` → `--patch`, `env` → the
//! child environment — so dsh-app never invents a second configuration
//! language. Presets live in `$HOME/dsh-app/presets/<id>.toml`.
//!
//! The store also lists the profiles that already exist under
//! `$HOME/profiles/`: a profile the `dsh` CLI created is a complete launch
//! configuration on its own, so it is listed as it is instead of being restated
//! as a preset (design doc §21).
//!
//! @see docs/design-tauri-wrapper.md D3, §21

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::preset::{self, Preset};
use crate::profile::{self, DiscoveredProfile};

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
    /// Node executable to run this preset with; see `Preset::node`.
    #[serde(default)]
    node: Option<String>,
}

fn default_kind() -> String {
    "web".to_owned()
}

/// Where one launchable entry came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Origin {
    /// A `.toml` file under `dsh-app/presets/` that the user declared.
    Preset,
    /// A profile directory under `profiles/` that already existed.
    Profile,
}

/// One launchable entry plus the directory its relative paths resolve against.
#[derive(Debug, Clone)]
pub struct ResolvedPreset {
    pub preset: Preset,
    /// Absolute patch files, in application order.
    pub patch_files: Vec<PathBuf>,
    /// The preset file, or the profile directory, this came from.
    pub source: PathBuf,
    /// Which of the two it is.
    pub origin: Origin,
}

/// Every launchable entry: the declared presets, then the profiles they do not own.
///
/// A profile a preset already points at is not listed again: the preset is the
/// more specific statement (it may add patch layers or an environment), and two
/// rows starting the same profile would be two ways to do one thing. Rows are
/// keyed by id — the child map and the harness window label both use it — so a
/// profile whose name a preset already uses as its id is skipped as well.
pub fn load_all_with_profiles(home: &Path, seed: Option<&Path>) -> (Vec<ResolvedPreset>, Vec<String>) {
    let (mut entries, mut problems) = load_all(home, seed);
    let (profiles, profile_problems) = profile::discover(home);
    problems.extend(profile_problems);

    // Owned, not borrowed: the entries vector grows below.
    let claimed: BTreeSet<String> = entries
        .iter()
        .flat_map(|entry| [entry.preset.id.clone(), entry.preset.profile_name().to_owned()])
        .collect();
    for profile in profiles {
        if claimed.contains(profile.name.as_str()) {
            continue;
        }
        entries.push(from_profile(profile));
    }
    (entries, problems)
}

/// Wrap one discovered profile as a launchable entry.
///
/// Nothing is materialized and nothing is written: the profile exists, and its
/// own manifest is the authority for its bundles and reload mode.
fn from_profile(profile: DiscoveredProfile) -> ResolvedPreset {
    let kind = preset::kind_for_bundles(&profile.bundles).to_owned();
    ResolvedPreset {
        preset: Preset {
            id: profile.name.clone(),
            label: Some(profile.name.clone()),
            kind,
            profile: Some(profile.name),
            // Display only: this list is never written back to the profile.
            bundles: Some(profile.bundles),
            patch_reload: profile.patch_reload,
            env: BTreeMap::new(),
            node: None,
        },
        patch_files: Vec::new(),
        source: profile.dir,
        origin: Origin::Profile,
    }
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
            node: stored.node,
        },
        patch_files,
        source: path.to_path_buf(),
        origin: Origin::Preset,
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
         # The Node runtime to run this preset with. Leave it out to use the one the\n\
         # app carries (`bundled` variant) or `DSH_APP_NODE`.\n\
         #\n\
         # It is an explicit path on purpose: an app started from Finder does not\n\
         # inherit your shell's PATH — it sees only /usr/bin:/bin:/usr/sbin:/sbin —\n\
         # so auto-detection would choose from an environment you did not set up.\n\
         # Point it at a Node from nodejs.org: distribution-repackaged builds can\n\
         # load the `require-builtin` addon and then refuse every id, which breaks\n\
         # `patch_reload = \"live\"`.\n\
         # node = \"/opt/homebrew/bin/node\"\n\
         \n\
         [env]\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// A throwaway harness home, removed when the test ends.
    struct TempHome(PathBuf);

    impl TempHome {
        fn new(label: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("dsh-app-{label}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).expect("create the test home");
            Self(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        /// Write one preset file.
        fn preset(&self, id: &str, profile: &str) {
            let dir = self.0.join(PRESETS_DIR);
            fs::create_dir_all(&dir).expect("create the preset directory");
            let text = format!("id = \"{id}\"\nlabel = \"{id}\"\nprofile = \"{profile}\"\n");
            fs::write(dir.join(format!("{id}.toml")), text).expect("write the preset");
        }

        /// Write one profile manifest, in the shape upstream `initProfile` writes.
        fn profile(&self, name: &str, bundles: &[&str], patch_reload: Option<&str>) {
            let dir = self.0.join(crate::profile::PROFILES_DIR).join(name);
            fs::create_dir_all(&dir).expect("create the profile directory");
            let mut profile = serde_json::json!({ "bundles": bundles });
            if let Some(value) = patch_reload {
                profile["patchReload"] = serde_json::Value::String(value.to_owned());
            }
            let manifest = serde_json::json!({ "dsh": { "profile": profile } });
            fs::write(dir.join("package.json"), manifest.to_string()).expect("write the manifest");
        }
    }

    impl Drop for TempHome {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// The store writes a default preset on first run, and that preset claims
    /// the `dsh-app` profile: one profile must not become two rows.
    #[test]
    fn a_declared_preset_owns_its_profile() {
        let home = TempHome::new("store-dedup");
        home.profile("dsh-app", &["@deepseek-ai/dsh-base", preset::WEB_BUNDLE], None);
        home.profile("demo", &["@deepseek-ai/dsh-base", preset::WEB_BUNDLE], Some("live"));

        let (entries, problems) = load_all_with_profiles(home.path(), None);
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(
            entries.iter().map(|entry| entry.preset.id.as_str()).collect::<Vec<_>>(),
            ["dsh-app", "demo"]
        );
        assert_eq!(entries[0].origin, Origin::Preset);
        assert_eq!(entries[1].origin, Origin::Profile);
    }

    #[test]
    fn a_discovered_profile_keeps_its_own_declaration() {
        let home = TempHome::new("store-declaration");
        home.profile("demo", &["@deepseek-ai/dsh-base", preset::WEB_BUNDLE], Some("live"));

        let (entries, problems) = load_all_with_profiles(home.path(), None);
        assert!(problems.is_empty(), "{problems:?}");
        let demo = entries
            .iter()
            .find(|entry| entry.preset.id == "demo")
            .expect("demo is listed");
        assert_eq!(demo.preset.kind, "web");
        assert_eq!(demo.preset.label.as_deref(), Some("demo"));
        assert_eq!(demo.preset.profile_name(), "demo");
        assert_eq!(demo.source, home.path().join(crate::profile::PROFILES_DIR).join("demo"));
        // A discovered profile adds no patch layer: its own `cordis.patch.yml`
        // is the user's layer, and the harness is what applies it.
        assert!(demo.patch_files.is_empty());
        // `live` is the profile's own statement, so internals capability is required.
        assert!(demo.preset.needs_internals());
        assert!(demo.preset.env.is_empty());
        assert!(demo.preset.node.is_none());
    }

    /// Rows are keyed by id: a preset that merely *borrows* the name must not
    /// leave two rows fighting over the same child and window label.
    #[test]
    fn a_profile_whose_name_is_already_a_preset_id_is_skipped() {
        let home = TempHome::new("store-id-clash");
        home.preset("demo", "somewhere-else");
        home.profile("demo", &[preset::WEB_BUNDLE], None);
        home.profile("other", &[preset::WEB_BUNDLE], None);

        let (entries, problems) = load_all_with_profiles(home.path(), None);
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(
            entries.iter().map(|entry| entry.preset.id.as_str()).collect::<Vec<_>>(),
            ["demo", "dsh-app", "other"]
        );
    }

    #[test]
    fn presets_come_first_and_profiles_follow_in_name_order() {
        let home = TempHome::new("store-order");
        home.preset("zulu", "zulu");
        home.preset("alpha", "alpha");
        home.profile("gamma", &[preset::WEB_BUNDLE], None);
        home.profile("delta", &[preset::WEB_BUNDLE], None);

        let (entries, problems) = load_all_with_profiles(home.path(), None);
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(
            entries.iter().map(|entry| entry.preset.id.as_str()).collect::<Vec<_>>(),
            ["alpha", "dsh-app", "zulu", "delta", "gamma"]
        );
    }
}
