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
//! It ships nothing of its own: no default preset is written and no sample is
//! seeded into a fresh home. A launcher lists what is there and starts what the
//! user picks, while creating a profile stays with the harness and the user
//! (design doc §22).
//!
//! @see docs/design-tauri-wrapper.md D3, §21, §22

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::preset::{self, Preset};
use crate::profile::{self, DiscoveredProfile};

/// Directory holding preset files, under the shell's own home.
pub const PRESETS_DIR: &str = "dsh-app/presets";

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
pub fn load_all_with_profiles(home: &Path) -> (Vec<ResolvedPreset>, Vec<String>) {
    let (mut entries, mut problems) = load_all(home);
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

/// Load every preset from `<home>/dsh-app/presets/`.
///
/// An empty directory is a normal state rather than a first run waiting to be
/// filled in: the shell writes no preset of its own (design doc §22). A
/// malformed preset file is reported rather than skipped, because a silently
/// ignored preset would look like the shell lost the user's configuration.
pub fn load_all(home: &Path) -> (Vec<ResolvedPreset>, Vec<String>) {
    let dir = home.join(PRESETS_DIR);
    let mut problems = Vec::new();
    if let Err(error) = std::fs::create_dir_all(&dir) {
        problems.push(format!("cannot create {}: {error}", dir.display()));
        return (Vec::new(), problems);
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

    /// A preset the user wrote owns its profile: one profile must not become
    /// two rows.
    #[test]
    fn a_declared_preset_owns_its_profile() {
        let home = TempHome::new("store-dedup");
        home.preset("dsh-app", "dsh-app");
        home.profile("dsh-app", &["@deepseek-ai/dsh-base", preset::WEB_BUNDLE], None);
        home.profile("demo", &["@deepseek-ai/dsh-base", preset::WEB_BUNDLE], Some("live"));

        let (entries, problems) = load_all_with_profiles(home.path());
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

        let (entries, problems) = load_all_with_profiles(home.path());
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

        let (entries, problems) = load_all_with_profiles(home.path());
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(
            entries.iter().map(|entry| entry.preset.id.as_str()).collect::<Vec<_>>(),
            ["demo", "other"]
        );
    }

    /// The shell ships no preset of its own: a fresh home stays empty until the
    /// user puts something there (design doc §22).
    #[test]
    fn a_fresh_home_gets_no_preset_from_us() {
        let home = TempHome::new("store-no-default");
        home.profile("web", &["@deepseek-ai/dsh-base", preset::WEB_BUNDLE], None);

        let (entries, problems) = load_all_with_profiles(home.path());
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(
            entries.iter().map(|entry| entry.preset.id.as_str()).collect::<Vec<_>>(),
            ["web"]
        );
        let written: Vec<String> = std::fs::read_dir(home.path().join(PRESETS_DIR))
            .expect("the preset directory exists")
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(written.is_empty(), "the shell wrote {written:?} into the user's home");
    }

    #[test]
    fn presets_come_first_and_profiles_follow_in_name_order() {
        let home = TempHome::new("store-order");
        home.preset("zulu", "zulu");
        home.preset("alpha", "alpha");
        home.profile("gamma", &[preset::WEB_BUNDLE], None);
        home.profile("delta", &[preset::WEB_BUNDLE], None);

        let (entries, problems) = load_all_with_profiles(home.path());
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(
            entries.iter().map(|entry| entry.preset.id.as_str()).collect::<Vec<_>>(),
            ["alpha", "zulu", "delta", "gamma"]
        );
    }
}
