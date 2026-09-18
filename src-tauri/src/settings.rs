//! The shell's own settings: where the user's Node and pnpm live.
//!
//! `slim` builds carry neither, so the user names them explicitly. This file is
//! app-level rather than per-preset on purpose: "where is my Node installed" is a
//! property of the machine, while a preset describes a way of working. A preset
//! may still override the Node (design doc D2, and `Preset::node`).
//!
//! The file sits beside `dsh-app/runtimes` and `dsh-app/presets`, so it survives
//! runtime updates — those only swap directories under `runtimes/`.
//!
//! @see docs/spec-slim-runtime-config.md §2.2

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Directory under the shell's home that holds everything dsh-app owns.
pub const APP_DIR: &str = "dsh-app";

/// The only settings schema this build understands.
const SCHEMA: u32 = 1;

/// What the user told us, as stored on disk.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Settings {
    /// Written by us; absent in a hand-written file, which is accepted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<u32>,
    /// Absolute path to a Node executable. Absent means "unconfigured".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
    /// Absolute path to a pnpm executable; optional, because pnpm only affects
    /// plugin management and must not block the GUI (spec §4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pnpm: Option<String>,
}

impl Settings {
    /// The configured pnpm, when there is one.
    pub fn pnpm_path(&self) -> Option<PathBuf> {
        self.pnpm
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    }
}

/// The outcome of reading `settings.json`.
///
/// A corrupt or unreadable file is **not** fatal: it is reported and treated as
/// "unconfigured", because refusing to start would turn "not filled in yet" into
/// an error — exactly what the spec's core requirement forbids.
#[derive(Debug, Clone)]
pub struct SettingsReport {
    pub settings: Settings,
    /// The path that was read, for the UI and for error messages.
    pub path: PathBuf,
    /// Why the file could not be used, when it could not.
    pub problem: Option<String>,
}

/// Where the shell's settings file lives, for a given shell home.
pub fn settings_path(home: &Path) -> PathBuf {
    home.join(APP_DIR).join("settings.json")
}

/// Read the settings file, tolerating absence and corruption.
pub fn load(home: &Path) -> SettingsReport {
    let path = settings_path(home);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        // Never written yet: the normal first-run state, not a problem.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return SettingsReport {
                settings: Settings::default(),
                path,
                problem: None,
            }
        }
        Err(error) => {
            return SettingsReport {
                settings: Settings::default(),
                path: path.clone(),
                problem: Some(format!("cannot read {}: {error}", path.display())),
            }
        }
    };

    match serde_json::from_str::<Settings>(&text) {
        Ok(settings) => {
            // A future schema may mean fields we would silently ignore, so say so
            // rather than overwriting a file this build does not understand.
            let problem = match settings.schema {
                Some(found) if found != SCHEMA => Some(format!(
                    "{}: unsupported schema {found} (this build understands {SCHEMA})",
                    path.display()
                )),
                _ => None,
            };
            let settings = if problem.is_some() {
                Settings::default()
            } else {
                settings
            };
            SettingsReport { settings, path, problem }
        }
        Err(error) => SettingsReport {
            settings: Settings::default(),
            path: path.clone(),
            problem: Some(format!("cannot parse {}: {error}", path.display())),
        },
    }
}

/// Write the settings file atomically.
///
/// Same discipline as the runtime store: write a temporary file, then `rename`
/// over the target, so an interrupted write can never leave a half-parsed file
/// behind. `node` is required; passing `None` or an empty string for `pnpm`
/// clears it.
pub fn save(home: &Path, node: &str, pnpm: Option<&str>) -> Result<PathBuf, String> {
    let path = settings_path(home);
    let dir = path
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", path.display()))?;
    std::fs::create_dir_all(dir).map_err(|error| format!("cannot create {}: {error}", dir.display()))?;

    let settings = Settings {
        schema: Some(SCHEMA),
        node: Some(node.trim().to_owned()),
        pnpm: pnpm
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned),
    };
    let text = serde_json::to_string_pretty(&settings).map_err(|error| error.to_string())?;

    let temporary = dir.join(".settings.json.next");
    std::fs::write(&temporary, format!("{text}\n"))
        .map_err(|error| format!("cannot write {}: {error}", temporary.display()))?;
    std::fs::rename(&temporary, &path)
        .map_err(|error| format!("cannot replace {}: {error}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch home, removed by `Drop`.
    struct TempHome(PathBuf);

    impl TempHome {
        fn new() -> Self {
            let unique = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("the clock is after the epoch")
                .as_nanos();
            let dir = std::env::temp_dir().join(format!("dsh-app-settings-{}-{unique}", std::process::id()));
            std::fs::create_dir_all(&dir).expect("the scratch home is creatable");
            TempHome(dir)
        }
    }

    impl Drop for TempHome {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn an_absent_file_is_unconfigured_rather_than_a_problem() {
        let home = TempHome::new();
        let report = load(&home.0);
        assert!(report.problem.is_none());
        assert!(report.settings.node.is_none());
    }

    #[test]
    fn saving_and_loading_round_trips() {
        let home = TempHome::new();
        save(&home.0, "/opt/homebrew/bin/node", Some("/opt/homebrew/bin/pnpm")).expect("the save succeeds");

        let report = load(&home.0);
        assert!(report.problem.is_none(), "{:?}", report.problem);
        assert_eq!(report.settings.node.as_deref(), Some("/opt/homebrew/bin/node"));
        assert_eq!(report.settings.pnpm_path(), Some(PathBuf::from("/opt/homebrew/bin/pnpm")));
    }

    #[test]
    fn an_empty_pnpm_is_stored_as_absent() {
        let home = TempHome::new();
        save(&home.0, "/usr/bin/node", Some("   ")).expect("the save succeeds");
        assert!(load(&home.0).settings.pnpm.is_none());
    }

    #[test]
    fn a_hand_written_file_without_a_schema_is_accepted() {
        let home = TempHome::new();
        let path = settings_path(&home.0);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("the directory is creatable");
        std::fs::write(&path, r#"{"node":"/opt/homebrew/bin/node"}"#).expect("the write succeeds");

        let report = load(&home.0);
        assert!(report.problem.is_none(), "{:?}", report.problem);
        assert_eq!(report.settings.node.as_deref(), Some("/opt/homebrew/bin/node"));
    }

    #[test]
    fn corruption_is_reported_and_treated_as_unconfigured() {
        let home = TempHome::new();
        let path = settings_path(&home.0);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("the directory is creatable");
        std::fs::write(&path, "{ not json").expect("the write succeeds");

        let report = load(&home.0);
        assert!(report.problem.is_some());
        assert!(report.settings.node.is_none());
    }

    #[test]
    fn a_future_schema_is_refused_instead_of_silently_dropped() {
        let home = TempHome::new();
        let path = settings_path(&home.0);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("the directory is creatable");
        std::fs::write(&path, r#"{"schema":9,"node":"/usr/bin/node"}"#).expect("the write succeeds");

        let report = load(&home.0);
        assert!(report.problem.as_deref().is_some_and(|text| text.contains("schema 9")));
        // The value must not be used *or* overwritten: the user's file is newer
        // than this build's understanding of it.
        assert!(report.settings.node.is_none());
    }
}
