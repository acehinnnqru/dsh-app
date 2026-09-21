//! The shell's own configuration file.
//!
//! Deliberately the shell's own directory rather than the harness home: what this
//! file configures — which dsh to launch, and which home to drive — has to be
//! readable before either of those answers exists, and a value inside a file
//! cannot decide where that file is. Keeping it here also means
//! `DSH_HOME=$(mktemp -d)` isolates the harness home without making the app
//! forget which dsh you use.
//!
//! Location: `$XDG_CONFIG_HOME/dsh-app/config.toml`, otherwise `$HOME/.config/dsh-app/config.toml`.
//!
//! @see docs/spec-external-dsh.md §3

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Directory under the user's config root that holds everything dsh-app owns.
pub const APP_DIR: &str = "dsh-app";

/// The file itself.
const FILE_NAME: &str = "config.toml";

/// The only config schema this build understands.
const SCHEMA: u32 = 1;

/// Header written above the serialized settings. TOML, unlike JSON, is meant to
/// be read and edited by hand.
const HEADER: &str = concat!(
    "# dsh-app configuration.\n",
    "#\n",
    "# dsh    the executable this launcher supervises.\n",
    "# home   the harness home it drives; default ~/.dsh.\n",
    "#\n",
    "# Both values are read on every start.\n",
    "#\n",
    "# onboarded and profile belong to the first-run guide. They are written by\n",
    "# the app, and editing them by hand is harmless rather than supported.\n",
);

/// What the user told us, as stored on disk.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Settings {
    /// Written by us; absent in a hand-written file, which is accepted. Kept so a
    /// file written by a newer build is refused instead of half-understood.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<u32>,
    /// Absolute path to a dsh executable. Absent means "unconfigured".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dsh: Option<String>,
    /// The harness home to drive, as the user typed it. Absent or blank means the
    /// default: `$HOME/.dsh`, or `DSH_HOME` for one session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home: Option<String>,
    /// Whether the first-run guide has been finished on this machine. Absent
    /// means "not yet": a file written by an older build simply gets the guide
    /// once, which is the harmless direction to fail in.
    #[serde(default)]
    pub onboarded: bool,
    /// The profile the guide ended on. Purely cosmetic — the launcher sorts it
    /// first and never starts it on its own — so an unknown name is dropped
    /// rather than refused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
}

/// The outcome of reading the configuration file.
///
/// A corrupt or unreadable file is **not** fatal: it is reported and treated as
/// "unconfigured", because refusing to start would turn "not filled in yet" into
/// an error — exactly what the design forbids.
#[derive(Debug, Clone)]
pub struct SettingsReport {
    pub settings: Settings,
    /// The path that was read, for the UI and for error messages.
    pub path: PathBuf,
    /// Why the file could not be used, when it could not.
    pub problem: Option<String>,
    /// Whether saving over this file is safe. False for exactly one case: a
    /// schema this build does not understand, which was written by a *newer*
    /// shell. A corrupt file is overwritable — replacing it is the fix — but a
    /// newer one must not be silently downgraded.
    pub overwritable: bool,
}

/// The directory the shell keeps its own configuration in.
pub fn config_dir() -> PathBuf {
    config_dir_with(
        std::env::var("XDG_CONFIG_HOME").ok().as_deref(),
        std::env::var("HOME").ok().as_deref(),
    )
}

/// `config_dir` with the environment passed in, so the rule can be tested
/// without mutating the environment of a shared test process.
pub fn config_dir_with(xdg_config_home: Option<&str>, home: Option<&str>) -> PathBuf {
    if let Some(value) = non_empty(xdg_config_home) {
        return PathBuf::from(value).join(APP_DIR);
    }
    match non_empty(home) {
        Some(home) => PathBuf::from(home).join(".config").join(APP_DIR),
        // No `HOME` at all: nowhere sensible to write. The shell reports this when
        // it cannot create the directory.
        None => PathBuf::from(".config").join(APP_DIR),
    }
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

/// Where the configuration file lives, for a given config directory.
pub fn settings_path(dir: &Path) -> PathBuf {
    dir.join(FILE_NAME)
}

/// Read the configuration file, tolerating absence and corruption.
pub fn load(dir: &Path) -> SettingsReport {
    let path = settings_path(dir);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        // Never written yet: the normal first-run state, not a problem.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return SettingsReport {
                settings: Settings::default(),
                path,
                problem: None,
                overwritable: true,
            }
        }
        Err(error) => {
            return SettingsReport {
                settings: Settings::default(),
                path: path.clone(),
                problem: Some(format!("cannot read {}: {error}", path.display())),
                // A file that cannot be read cannot be respected either, and a
                // write is the user's only way out of it.
                overwritable: true,
            }
        }
    };

    match toml::from_str::<Settings>(&text) {
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
            let unsupported = problem.is_some();
            let settings = if unsupported {
                Settings::default()
            } else {
                settings
            };
            SettingsReport {
                settings,
                path,
                problem,
                overwritable: !unsupported,
            }
        }
        Err(error) => SettingsReport {
            settings: Settings::default(),
            path: path.clone(),
            problem: Some(format!("cannot parse {}: {error}", path.display())),
            overwritable: true,
        },
    }
}

/// Write the configuration file atomically.
///
/// Write a temporary file, then rename over the target, so an interrupted write
/// can never leave a half-parsed file behind. A blank value is omitted rather
/// than written empty: absent means the default.
///
/// Load-modify-write rather than replace: the guide's own bookkeeping lives in
/// the same file, so editing the runtime in the settings sheet must not clear
/// which profile the guide ended on.
pub fn save(dir: &Path, dsh: &str, home: &str) -> Result<PathBuf, String> {
    let report = load(dir);
    refuse_downgrade(&report)?;
    write(
        dir,
        &Settings {
            schema: Some(SCHEMA),
            dsh: non_empty(Some(dsh)).map(str::to_owned),
            home: non_empty(Some(home)).map(str::to_owned),
            onboarded: report.settings.onboarded,
            profile: report.settings.profile.clone(),
        },
    )
}

/// Record that the guide finished, and which profile it ended on.
///
/// `dsh` and `home` are left exactly as they are: step 1 saves those, and a
/// second visit only ever adds this bookkeeping. An unknown profile name is
/// stored as none rather than refused — the launcher tolerates a profile that
/// disappeared, and a stale name is not worth blocking the guide over.
pub fn save_onboarding(dir: &Path, profile: Option<&str>) -> Result<PathBuf, String> {
    let report = load(dir);
    refuse_downgrade(&report)?;
    write(
        dir,
        &Settings {
            schema: Some(SCHEMA),
            onboarded: true,
            profile: non_empty(profile).map(str::to_owned),
            ..report.settings
        },
    )
}

/// Refuse to write over a file this build does not understand.
///
/// The one irreversible mistake this module could make is downgrading a file
/// written by a newer shell, so that case — and only that case — blocks a write.
fn refuse_downgrade(report: &SettingsReport) -> Result<(), String> {
    if report.overwritable {
        return Ok(());
    }
    Err(format!(
        "{} was written by a newer dsh-app; refusing to replace it",
        report.path.display()
    ))
}

/// The atomic write itself.
fn write(dir: &Path, settings: &Settings) -> Result<PathBuf, String> {
    let path = settings_path(dir);
    std::fs::create_dir_all(dir).map_err(|error| format!("cannot create {}: {error}", dir.display()))?;

    let body = toml::to_string(settings).map_err(|error| error.to_string())?;

    let temporary = dir.join(".config.toml.next");
    std::fs::write(&temporary, format!("{HEADER}{body}"))
        .map_err(|error| format!("cannot write {}: {error}", temporary.display()))?;
    std::fs::rename(&temporary, &path)
        .map_err(|error| format!("cannot replace {}: {error}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch config directory, removed by Drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let unique = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("the clock is after the epoch")
                .as_nanos();
            let dir = std::env::temp_dir().join(format!("dsh-app-config-{}-{unique}", std::process::id()));
            std::fs::create_dir_all(&dir).expect("the scratch directory is creatable");
            TempDir(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// The shell's own directory, never the harness home.
    #[test]
    fn the_config_directory_is_the_shells_own() {
        assert_eq!(
            config_dir_with(None, Some("/Users/someone")),
            PathBuf::from("/Users/someone/.config/dsh-app")
        );
        assert_eq!(
            config_dir_with(Some("/tmp/xdg"), Some("/Users/someone")),
            PathBuf::from("/tmp/xdg/dsh-app")
        );
        // A blank override is the same as not setting it.
        assert_eq!(
            config_dir_with(Some("   "), Some("/Users/someone")),
            PathBuf::from("/Users/someone/.config/dsh-app")
        );
    }

    #[test]
    fn an_absent_file_is_unconfigured_rather_than_a_problem() {
        let dir = TempDir::new();
        let report = load(&dir.0);
        assert!(report.problem.is_none());
        assert!(report.settings.dsh.is_none());
        assert_eq!(report.path, dir.0.join("config.toml"));
    }

    #[test]
    fn saving_and_loading_round_trips() {
        let dir = TempDir::new();
        save(&dir.0, "/etc/profiles/per-user/me/bin/dsh", "~/.dsh").expect("the save succeeds");

        let report = load(&dir.0);
        assert!(report.problem.is_none(), "{:?}", report.problem);
        assert_eq!(report.settings.dsh.as_deref(), Some("/etc/profiles/per-user/me/bin/dsh"));
        // The home is stored as typed: expanding a tilde is the resolver's job, and
        // storing the expansion would freeze one machine's answer into the file.
        assert_eq!(report.settings.home.as_deref(), Some("~/.dsh"));
    }

    #[test]
    fn a_blank_value_is_omitted_rather_than_written_empty() {
        let dir = TempDir::new();
        let path = save(&dir.0, "/usr/bin/dsh", "   ").expect("the save succeeds");
        let text = std::fs::read_to_string(&path).expect("the file is readable");
        assert!(text.starts_with("# dsh-app configuration."), "{text}");
        assert!(text.contains("dsh = \"/usr/bin/dsh\""), "{text}");
        assert!(!text.contains("home ="), "{text}");
    }

    #[test]
    fn a_hand_written_file_without_a_schema_is_accepted() {
        let dir = TempDir::new();
        std::fs::write(settings_path(&dir.0), "dsh = \"/opt/homebrew/bin/dsh\"\n").expect("the write succeeds");

        let report = load(&dir.0);
        assert!(report.problem.is_none(), "{:?}", report.problem);
        assert_eq!(report.settings.dsh.as_deref(), Some("/opt/homebrew/bin/dsh"));
    }

    #[test]
    fn corruption_is_reported_and_treated_as_unconfigured() {
        let dir = TempDir::new();
        std::fs::write(settings_path(&dir.0), "not = [valid").expect("the write succeeds");

        let report = load(&dir.0);
        assert!(report.problem.is_some());
        assert!(report.settings.dsh.is_none());
    }

    #[test]
    fn a_future_schema_is_refused_instead_of_silently_dropped() {
        let dir = TempDir::new();
        std::fs::write(settings_path(&dir.0), "schema = 9\ndsh = \"/usr/bin/dsh\"\n").expect("the write succeeds");

        let report = load(&dir.0);
        assert!(report.problem.as_deref().is_some_and(|text| text.contains("schema 9")));
        // The value must not be used *or* overwritten: the user's file is newer
        // than this build's understanding of it.
        assert!(report.settings.dsh.is_none());
    }

    /// The one irreversible mistake this module could make: replacing a file a
    /// newer shell wrote.
    #[test]
    fn a_newer_file_is_never_overwritten() {
        let dir = TempDir::new();
        let path = settings_path(&dir.0);
        std::fs::write(&path, "schema = 9\ndsh = \"/usr/bin/dsh\"\n").expect("the write succeeds");

        assert!(save(&dir.0, "/usr/bin/other", "").is_err());
        assert!(save_onboarding(&dir.0, Some("web")).is_err());
        let text = std::fs::read_to_string(&path).expect("the file is readable");
        assert!(text.contains("schema = 9"), "the newer file must survive: {text}");
    }

    /// The guide's bookkeeping lives in the same file as the runtime, so an edit
    /// to the runtime must not clear it.
    #[test]
    fn saving_the_runtime_keeps_the_guides_bookkeeping() {
        let dir = TempDir::new();
        save(&dir.0, "/usr/bin/dsh", "~/.dsh").expect("the save succeeds");
        save_onboarding(&dir.0, Some("web")).expect("the guide finishes");

        save(&dir.0, "/opt/other/dsh", "").expect("the runtime is edited");

        let report = load(&dir.0);
        assert!(report.problem.is_none(), "{:?}", report.problem);
        assert_eq!(report.settings.dsh.as_deref(), Some("/opt/other/dsh"));
        assert!(report.settings.onboarded);
        assert_eq!(report.settings.profile.as_deref(), Some("web"));
    }

    /// A file written before the guide existed is an unfinished guide, not a
    /// problem: the guide runs once and then never again.
    #[test]
    fn a_file_without_the_guide_fields_is_not_onboarded() {
        let dir = TempDir::new();
        std::fs::write(settings_path(&dir.0), "dsh = \"/usr/bin/dsh\"\n").expect("the write succeeds");

        let report = load(&dir.0);
        assert!(!report.settings.onboarded);
        assert!(report.settings.profile.is_none());
    }

    #[test]
    fn finishing_the_guide_records_the_profile_and_keeps_the_runtime() {
        let dir = TempDir::new();
        save(&dir.0, "/usr/bin/dsh", "~/elsewhere").expect("the save succeeds");
        save_onboarding(&dir.0, Some("demo")).expect("the guide finishes");

        let report = load(&dir.0);
        assert!(report.settings.onboarded);
        assert_eq!(report.settings.profile.as_deref(), Some("demo"));
        assert_eq!(report.settings.dsh.as_deref(), Some("/usr/bin/dsh"));
        assert_eq!(report.settings.home.as_deref(), Some("~/elsewhere"));

        // Skipping the profile step is a valid ending, not a failure.
        save_onboarding(&dir.0, None).expect("the guide finishes without a profile");
        assert!(load(&dir.0).settings.profile.is_none());
    }
}
