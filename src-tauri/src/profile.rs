//! Materialize a preset's profile on first boot.
//!
//! dsh-app writes the profile itself rather than invoking `dsh plugin`, which
//! forwards to PATH `pnpm` and would make first boot depend on a package manager
//! being resolvable. The shapes mirror the harness `initProfile` output
//! (`@deepseek-ai/dsh-app-boot`).
//!
//! @see docs/design-tauri-wrapper.md D3, §11.5

use std::path::{Path, PathBuf};

use crate::preset::Preset;
use crate::profile_template;

/// The profile directory for one preset under a harness home.
pub fn profile_dir(home: &Path, preset: &Preset) -> PathBuf {
    home.join("profiles").join(preset.profile_name())
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
