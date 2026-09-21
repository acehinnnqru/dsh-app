//! Materialize a preset's profile on first boot, and read the profiles that
//! already exist.
//!
//! dsh-app writes the profile itself rather than invoking `dsh plugin`, which
//! forwards to PATH `pnpm` and would make first boot depend on a package manager
//! being resolvable. The shapes mirror the harness `initProfile` output
//! (`@deepseek-ai/dsh-app-boot`).
//!
//! Discovery is the mirror image: a profile the `dsh` CLI created is already a
//! complete launch configuration, so the launcher lists it as it is rather than
//! asking the user to restate it as a preset.
//!
//! @see docs/design-tauri-wrapper.md D3, §11.5, §21

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::preset::Preset;
use crate::profile_template;

/// Directory under a harness home holding every profile.
pub const PROFILES_DIR: &str = "profiles";

/// The profile directory for one preset under a harness home.
pub fn profile_dir(home: &Path, preset: &Preset) -> PathBuf {
    home.join(PROFILES_DIR).join(preset.profile_name())
}

/// A profile that exists on disk, as its own manifest declares it.
///
/// The shell reads the profile's statement about itself instead of inventing
/// one: `bundles` decides which carrier it can be launched as, and
/// `patchReload` decides whether it needs internals access.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredProfile {
    /// Directory name, which is the `--profile` argument.
    pub name: String,
    /// The profile directory.
    pub dir: PathBuf,
    /// `dsh.profile.bundles`, in order.
    pub bundles: Vec<String>,
    /// `dsh.profile.patchReload`, when the profile declares one.
    pub patch_reload: Option<String>,
}

/// `package.json`'s `dsh.profile` section, as far as the shell reads it.
#[derive(Debug, Deserialize)]
struct ProfileManifest {
    dsh: Option<ProfileSection>,
}

#[derive(Debug, Deserialize)]
struct ProfileSection {
    profile: Option<ProfileDeclaration>,
}

#[derive(Debug, Deserialize)]
struct ProfileDeclaration {
    #[serde(default)]
    bundles: Option<Vec<String>>,
    #[serde(default, rename = "patchReload")]
    patch_reload: Option<String>,
}

/// Read every materialized profile under `<home>/profiles`.
///
/// A directory without `package.json` is skipped, because that is exactly what
/// the harness treats as "not a profile yet" (`initProfile` writes the manifest
/// first). One that *has* a manifest the shell cannot read is reported instead:
/// silently dropping it would look like the profile vanished.
///
/// A directory named `node_modules` is never a profile — that name is the
/// installation's shared fallback, and upstream rejects it as a profile name too.
pub fn discover(home: &Path) -> (Vec<DiscoveredProfile>, Vec<String>) {
    let dir = home.join(PROFILES_DIR);
    let mut problems = Vec::new();
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        // No profiles directory yet is not a problem: nothing has been created.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return (Vec::new(), problems),
        Err(error) => {
            problems.push(format!("cannot read {}: {error}", dir.display()));
            return (Vec::new(), problems);
        }
    };

    let mut profiles = Vec::new();
    for entry in entries.filter_map(Result::ok) {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == "node_modules" || name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let manifest = path.join("package.json");
        if !manifest.is_file() {
            continue;
        }
        match read_declaration(&manifest) {
            Ok(Some(declaration)) => profiles.push(DiscoveredProfile {
                name,
                dir: path,
                bundles: declaration.bundles.unwrap_or_default(),
                patch_reload: declaration.patch_reload,
            }),
            Ok(None) => problems.push(format!(
                "{} declares no dsh.profile section, so it is not a profile the launcher can start",
                manifest.display()
            )),
            Err(error) => problems.push(error),
        }
    }
    profiles.sort_by(|left, right| left.name.cmp(&right.name));
    (profiles, problems)
}

/// Read one profile manifest's declaration, or `Ok(None)` when it has none.
fn read_declaration(manifest: &Path) -> Result<Option<ProfileDeclaration>, String> {
    let text = std::fs::read_to_string(manifest)
        .map_err(|error| format!("cannot read {}: {error}", manifest.display()))?;
    let parsed: ProfileManifest = serde_json::from_str(&text)
        .map_err(|error| format!("cannot parse {}: {error}", manifest.display()))?;
    Ok(parsed.dsh.and_then(|section| section.profile))
}

/// Whether the profile already exists (any of its owned files present).
pub fn is_materialized(dir: &Path) -> bool {
    dir.join("package.json").is_file()
}

/// Result of one materialization attempt.
pub struct MaterializeReport {
    pub dir: PathBuf,
    pub created: bool,
}

/// Create the profile manifest, empty user patch layer, and pnpm settings.
///
/// Existing files are never touched, matching upstream's `initProfile`, so this
/// is a no-op on an initialized profile and user edits survive.
pub fn materialize(home: &Path, preset: &Preset) -> Result<MaterializeReport, String> {
    let dir = profile_dir(home, preset);
    let existed = is_materialized(&dir);
    std::fs::create_dir_all(&dir)
        .map_err(|error| format!("cannot create profile {}: {error}", dir.display()))?;

    let manifest_path = dir.join("package.json");
    if !manifest_path.exists() {
        let manifest = serde_json::json!({
            "name": format!("dsh-profile-{}", preset.profile_name()),
            "private": true,
            "dependencies": {},
            "dsh": {
                "profile": {
                    "bundles": preset.bundle_list(),
                    "patchReload": preset.patch_reload_value(),
                }
            }
        });
        let text = format!("{}\n", serde_json::to_string_pretty(&manifest).map_err(|error| error.to_string())?);
        write_new(&manifest_path, &text)?;
    }

    let patch_path = dir.join("cordis.patch.yml");
    if !patch_path.exists() {
        write_new(&patch_path, profile_template::PROFILE_PATCH_TEMPLATE)?;
    }

    let workspace_path = dir.join("pnpm-workspace.yaml");
    if !workspace_path.exists() {
        write_new(&workspace_path, profile_template::PROFILE_PNPM_WORKSPACE)?;
    }

    Ok(MaterializeReport { dir, created: !existed })
}

/// Write a file only when it does not exist yet.
fn write_new(path: &Path, contents: &str) -> Result<(), String> {
    std::fs::write(path, contents).map_err(|error| format!("cannot write {}: {error}", path.display()))
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
            fs::create_dir_all(dir.join(PROFILES_DIR)).expect("create the test home");
            Self(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        /// Write one profile manifest, in the shape upstream `initProfile` writes.
        fn profile(&self, name: &str, manifest: &str) {
            let dir = self.0.join(PROFILES_DIR).join(name);
            fs::create_dir_all(&dir).expect("create the profile directory");
            fs::write(dir.join("package.json"), manifest).expect("write the manifest");
        }
    }

    impl Drop for TempHome {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    const WEB_MANIFEST: &str = r#"{
      "name": "dsh-profile-demo",
      "dsh": { "profile": { "bundles": ["@deepseek-ai/dsh-base", "@deepseek-ai/dsh-web-app"], "patchReload": "live" } }
    }"#;

    #[test]
    fn a_profile_is_read_from_its_own_manifest() {
        let home = TempHome::new("discover-read");
        home.profile("demo", WEB_MANIFEST);

        let (profiles, problems) = discover(home.path());
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].name, "demo");
        assert_eq!(profiles[0].dir, home.path().join(PROFILES_DIR).join("demo"));
        assert_eq!(
            profiles[0].bundles,
            ["@deepseek-ai/dsh-base", "@deepseek-ai/dsh-web-app"]
        );
        assert_eq!(profiles[0].patch_reload.as_deref(), Some("live"));
    }

    /// A manifest without `dsh.profile` is a real directory the user can see in
    /// the finder, so dropping it silently would look like it vanished.
    #[test]
    fn a_directory_that_is_not_a_profile_is_reported() {
        let home = TempHome::new("discover-not-a-profile");
        home.profile("plain", r#"{ "name": "dsh-profile-plain" }"#);

        let (profiles, problems) = discover(home.path());
        assert!(profiles.is_empty());
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].contains("dsh.profile"), "{problems:?}");
    }

    #[test]
    fn a_broken_manifest_is_reported_rather_than_dropped() {
        let home = TempHome::new("discover-broken");
        home.profile("broken", "{ not json");

        let (profiles, problems) = discover(home.path());
        assert!(profiles.is_empty());
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].contains("cannot parse"), "{problems:?}");
    }

    /// `profiles/node_modules` is the installation's shared fallback, not a
    /// profile; a directory the harness has not initialized has no manifest.
    #[test]
    fn the_fallback_and_uninitialized_directories_are_skipped() {
        let home = TempHome::new("discover-skip");
        let profiles_dir = home.path().join(PROFILES_DIR);
        fs::create_dir_all(profiles_dir.join("uninitialized")).expect("create an uninitialized profile");
        fs::create_dir_all(profiles_dir.join("node_modules")).expect("create the fallback");
        fs::write(profiles_dir.join("node_modules/package.json"), "{ not json").expect("write junk");
        home.profile("web", WEB_MANIFEST);

        let (profiles, problems) = discover(home.path());
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(profiles.iter().map(|entry| entry.name.as_str()).collect::<Vec<_>>(), ["web"]);
    }

    #[test]
    fn discovery_is_sorted_and_a_home_without_profiles_is_not_a_problem() {
        let home = TempHome::new("discover-order");
        home.profile("zulu", WEB_MANIFEST);
        home.profile("alpha", WEB_MANIFEST);

        let (profiles, problems) = discover(home.path());
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(
            profiles.iter().map(|entry| entry.name.as_str()).collect::<Vec<_>>(),
            ["alpha", "zulu"]
        );

        fs::remove_dir_all(home.path().join(PROFILES_DIR)).expect("remove the profiles directory");
        let (profiles, problems) = discover(home.path());
        assert!(profiles.is_empty());
        assert!(problems.is_empty(), "{problems:?}");
    }
}
