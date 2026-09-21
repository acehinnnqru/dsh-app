//! The shell's home, and the `dsh` executable it launches.
//!
//! The shell ships no runtime of its own: it supervises an executable the user
//! names, so "which dsh" is the only resolution left, and it has exactly two
//! sources — one session, or this machine's settings file.
//!
//! @see docs/spec-external-dsh.md §2, §3, §5

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::settings::{self, Settings};

/// Where the shell keeps state and which dsh it will launch.
#[derive(Debug, Clone, Serialize)]
pub struct AppConfig {
    /// The harness home the shell drives: where profiles come from, the child's
    /// working directory, and the value it is told as `DSH_HOME`.
    pub home: PathBuf,
    /// Where the shell keeps its own configuration. Its own directory, not the
    /// home it drives, so that a configured home can never relocate the file
    /// that configures it.
    pub config_dir: PathBuf,
    /// Problems found while loading settings, surfaced in the UI. A corrupt
    /// settings file never stops the shell: it means "unconfigured".
    pub settings_problems: Vec<String>,
}

/// Where a resolved `dsh` came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DshSource {
    /// `DSH_APP_DSH` named it — one session.
    Environment,
    /// The settings file named it — what the user typed into the shell's form.
    Settings,
}

impl DshSource {
    /// Stable label for the payload and diagnostics.
    pub fn label(self) -> &'static str {
        match self {
            DshSource::Environment => "DSH_APP_DSH",
            DshSource::Settings => "settings",
        }
    }
}

/// A `dsh` executable together with where it was found.
#[derive(Debug, Clone)]
pub struct ResolvedDsh {
    pub path: PathBuf,
    pub source: DshSource,
}

/// The outcome of resolution.
///
/// `Unconfigured` is a **state, not an error**: a shell that has not been told
/// where dsh is waits for the user to fill in the form, and must not show an
/// error before they have had the chance.
#[derive(Debug, Clone)]
pub enum DshResolution {
    Configured(ResolvedDsh),
    Unconfigured,
}

impl DshResolution {
    /// The resolved executable, when there is one.
    pub fn dsh(&self) -> Option<&ResolvedDsh> {
        match self {
            DshResolution::Configured(dsh) => Some(dsh),
            DshResolution::Unconfigured => None,
        }
    }
}

/// Resolve the executable the shell launches.
///
/// Precedence, highest first (spec §2):
///
/// 1. `DSH_APP_DSH` — one session (a terminal, a CI job);
/// 2. the settings file — this machine.
///
/// A value that was named but is unusable (missing, a directory, not executable)
/// is an error rather than a reason to fall through: silently launching a
/// different dsh than the one that was asked for is worse than saying so.
pub fn resolve_dsh(settings: &Settings) -> Result<DshResolution, String> {
    resolve_dsh_with(std::env::var("DSH_APP_DSH").ok().as_deref(), settings)
}

/// `resolve_dsh` with the session-level value passed in, so the precedence can be
/// tested without mutating the environment of a shared test process.
pub fn resolve_dsh_with(from_env: Option<&str>, settings: &Settings) -> Result<DshResolution, String> {
    if let Some(chosen) = non_empty(from_env) {
        let path = PathBuf::from(chosen);
        return match usable(&path) {
            true => Ok(DshResolution::Configured(ResolvedDsh {
                path,
                source: DshSource::Environment,
            })),
            false => Err(format!("DSH_APP_DSH is not an executable file: {}", path.display())),
        };
    }

    if let Some(chosen) = non_empty(settings.dsh.as_deref()) {
        let path = PathBuf::from(chosen);
        return match usable(&path) {
            true => Ok(DshResolution::Configured(ResolvedDsh {
                path,
                source: DshSource::Settings,
            })),
            false => Err(format!(
                "the configured dsh is not an executable file: {}\n\
                 fix the path in the shell's form, or clear it to stay unconfigured",
                path.display()
            )),
        };
    }

    Ok(DshResolution::Unconfigured)
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

/// Whether the path is an executable file the shell could spawn.
pub fn usable(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    metadata.is_file() && is_executable(&metadata)
}

#[cfg(unix)]
fn is_executable(metadata: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_metadata: &std::fs::Metadata) -> bool {
    true
}

/// The message shown when nothing is configured. Guidance, not an error.
pub fn unconfigured_message() -> String {
    "no dsh executable is configured yet. Run `which dsh` in a terminal and paste \
     the path into this form."
        .to_owned()
}

/// The harness home the shell drives: where profiles come from, and what the
/// child is told as `DSH_HOME`.
///
/// Precedence, highest first — the same "more specific scope wins" rule the dsh
/// path follows:
///
/// 1. `DSH_HOME` — one session, and what makes `DSH_HOME=$(mktemp -d) dsh-app`
///    an isolated run;
/// 2. the form's value — this machine;
/// 3. `$HOME/.dsh` — the harness's own default. Its resolver calls
///    `os.homedir()`, which honours `HOME`, so the two agree without either
///    having to say anything.
///
/// The shell's *own* configuration deliberately does not live here: it is in the
/// shell's config directory (`settings::config_dir`), so that a value in it can
/// never decide where it is, and so `DSH_HOME=$(mktemp -d)` isolates the harness
/// without making the app forget which dsh you use.
pub fn resolve_home(settings: &Settings) -> PathBuf {
    resolve_home_with(
        std::env::var("DSH_HOME").ok().as_deref(),
        settings.home.as_deref(),
        std::env::var("HOME").ok().as_deref(),
    )
}

/// `resolve_home` with every input passed in, so the rule can be tested
/// without mutating the environment of a shared test process.
fn resolve_home_with(dsh_home: Option<&str>, from_settings: Option<&str>, user_home: Option<&str>) -> PathBuf {
    let user_home = non_empty(user_home).map(PathBuf::from);
    if let Some(value) = non_empty(dsh_home) {
        return expand_tilde(value, user_home.as_deref());
    }
    if let Some(value) = non_empty(from_settings) {
        return expand_tilde(value, user_home.as_deref());
    }
    match user_home {
        Some(home) => home.join(".dsh"),
        // No `HOME` at all: there is nowhere sensible to write. The shell reports
        // this when it cannot create the path, and the child is told the same
        // thing rather than resolving a home of its own.
        None => PathBuf::from(".dsh"),
    }
}

/// Where the driven home came from, for the settings sheet's diagnostics.
///
/// The same precedence `resolve_home` applies, reported instead of resolved: a
/// field that shows the stored value and a line that names the effective one are
/// only honest if they can disagree out loud.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum HomeSource {
    /// `DSH_HOME` named it — one session.
    Environment,
    /// The settings file named it — what the user typed into the guide.
    Settings,
    /// Neither: the harness's own default, `$HOME/.dsh`.
    Default,
}

impl HomeSource {
    /// Stable label for the payload and diagnostics.
    pub fn label(self) -> &'static str {
        match self {
            HomeSource::Environment => "DSH_HOME",
            HomeSource::Settings => "settings",
            HomeSource::Default => "default",
        }
    }
}

/// Which level `resolve_home` would take the home from.
pub fn home_source(settings: &Settings) -> HomeSource {
    home_source_with(
        std::env::var("DSH_HOME").ok().as_deref(),
        settings.home.as_deref(),
    )
}

/// `home_source` with both inputs passed in, so the rule is testable without
/// mutating the environment of a shared test process.
pub fn home_source_with(dsh_home: Option<&str>, from_settings: Option<&str>) -> HomeSource {
    if non_empty(dsh_home).is_some() {
        return HomeSource::Environment;
    }
    if non_empty(from_settings).is_some() {
        return HomeSource::Settings;
    }
    HomeSource::Default
}

/// Expand a leading `~` the way the harness does, so a home typed as `~/.dsh`
/// means the same thing to both.
fn expand_tilde(value: &str, user_home: Option<&Path>) -> PathBuf {
    let Some(home) = user_home else {
        return PathBuf::from(value);
    };
    if value == "~" {
        return home.to_path_buf();
    }
    match value.strip_prefix("~/").or_else(|| value.strip_prefix("~\\")) {
        Some(rest) => home.join(rest),
        None => PathBuf::from(value),
    }
}

/// Check a harness home the user typed, returning the absolute path to use.
///
/// Blank means the default. Anything else must be absolute after expanding `~`,
/// and the directory is created here: the shell needs it as the child's `DSH_HOME`
/// and as its working directory, so "this path cannot be created" is exactly the
/// verdict the form should show.
pub fn check_home(value: &str) -> Result<PathBuf, String> {
    let user_home = non_empty(std::env::var("HOME").ok().as_deref()).map(PathBuf::from);
    let path = match non_empty(Some(value)) {
        Some(typed) => expand_tilde(typed, user_home.as_deref()),
        None => match user_home {
            Some(home) => home.join(".dsh"),
            None => PathBuf::from(".dsh"),
        },
    };
    if !path.is_absolute() {
        return Err(format!(
            "{} is not an absolute path; the harness home cannot be relative",
            path.display()
        ));
    }
    std::fs::create_dir_all(&path).map_err(|error| format!("cannot create {}: {error}", path.display()))?;
    if !path.is_dir() {
        return Err(format!("{} is not a directory", path.display()));
    }
    Ok(path)
}

/// Load the shell's configuration from its own config directory.
pub fn load(config_dir: &Path) -> AppConfig {
    let report = settings::load(config_dir);
    AppConfig {
        home: resolve_home(&report.settings),
        config_dir: config_dir.to_path_buf(),
        settings_problems: report.problem.iter().cloned().collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// A scratch directory, removed by `Drop`.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(label: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("dsh-app-config-{label}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).expect("the scratch directory is creatable");
            Scratch(dir)
        }

        /// An executable file at `name`, plus its absolute path.
        fn executable(&self, name: &str) -> PathBuf {
            let path = self.0.join(name);
            fs::write(&path, "#!/bin/sh\nexit 0\n").expect("the file is writable");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("it is chmod-able");
            }
            path
        }

        /// A non-executable file at `name`, plus its absolute path.
        fn plain_file(&self, name: &str) -> PathBuf {
            let path = self.0.join(name);
            fs::write(&path, "not a program").expect("the file is writable");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).expect("it is chmod-able");
            }
            path
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn settings_naming(path: &Path) -> Settings {
        Settings {
            dsh: Some(path.display().to_string()),
            ..Settings::default()
        }
    }

    /// The whole point of the shell's form: not being told where dsh is must
    /// stay a state, not a failure (spec §1).
    #[test]
    fn nothing_configured_is_a_state_and_not_an_error() {
        let resolved = resolve_dsh_with(None, &Settings::default()).expect("absence is not an error");
        assert!(resolved.dsh().is_none());
        assert!(matches!(resolved, DshResolution::Unconfigured));
    }

    #[test]
    fn the_settings_file_names_the_executable() {
        let scratch = Scratch::new("from-settings");
        let path = scratch.executable("dsh");
        let resolved = resolve_dsh_with(None, &settings_naming(&path)).expect("resolution succeeds");
        let dsh = resolved.dsh().expect("a dsh is configured");
        assert_eq!(dsh.path, path);
        assert_eq!(dsh.source, DshSource::Settings);
    }

    #[test]
    fn the_session_value_outranks_the_settings_file() {
        let scratch = Scratch::new("precedence");
        let from_settings = scratch.executable("dsh-from-settings");
        let from_env = scratch.executable("dsh-from-env");
        let resolved = resolve_dsh_with(Some(&from_env.display().to_string()), &settings_naming(&from_settings))
            .expect("resolution succeeds");
        assert_eq!(resolved.dsh().expect("a dsh is configured").path, from_env);
    }

    /// A blank override is the same as not setting it: the common case is a
    /// shell launched with an empty variable in the environment.
    #[test]
    fn a_blank_session_value_falls_through_to_the_settings_file() {
        let scratch = Scratch::new("blank-env");
        let path = scratch.executable("dsh");
        let resolved = resolve_dsh_with(Some("   "), &settings_naming(&path)).expect("resolution succeeds");
        assert_eq!(resolved.dsh().expect("a dsh is configured").path, path);
    }

    /// Named but unusable is an error, not a silent fall-through: launching a
    /// different dsh than the one that was asked for is worse than saying so.
    #[test]
    fn a_named_but_unusable_path_is_an_error_not_a_fallthrough() {
        let scratch = Scratch::new("unusable");
        let missing = scratch.0.join("gone");
        let error = resolve_dsh_with(None, &settings_naming(&missing)).expect_err("a missing path is an error");
        assert!(error.contains("not an executable file"), "{error}");

        let not_executable = scratch.plain_file("plain");
        let error =
            resolve_dsh_with(None, &settings_naming(&not_executable)).expect_err("a non-executable is an error");
        assert!(error.contains("not an executable file"), "{error}");

        let error = resolve_dsh_with(Some(&missing.display().to_string()), &Settings::default())
            .expect_err("a missing DSH_APP_DSH is an error");
        assert!(error.contains("DSH_APP_DSH"), "{error}");
    }

    #[test]
    fn a_directory_is_never_usable() {
        let scratch = Scratch::new("directory");
        assert!(!usable(&scratch.0));
    }

    /// The precedence the form has to explain: session, then this machine, then
    /// the harness's own default.
    #[test]
    fn the_home_resolution_order_is_session_then_settings_then_default() {
        let user = Path::new("/Users/someone");
        assert_eq!(
            resolve_home_with(Some("/tmp/session"), Some("/tmp/form"), Some("/Users/someone")),
            PathBuf::from("/tmp/session")
        );
        assert_eq!(
            resolve_home_with(None, Some("/tmp/form"), Some("/Users/someone")),
            PathBuf::from("/tmp/form")
        );
        assert_eq!(
            resolve_home_with(None, None, Some("/Users/someone")),
            PathBuf::from("/Users/someone/.dsh")
        );
        // A blank value at either level is the same as not setting it.
        assert_eq!(
            resolve_home_with(Some("   "), Some("  "), Some("/Users/someone")),
            user.join(".dsh")
        );
    }

    /// `~` has to mean the same thing to the shell and to the harness, which
    /// expands it too.
    #[test]
    fn a_tilde_is_expanded_against_the_users_home() {
        assert_eq!(
            resolve_home_with(None, Some("~/.dsh"), Some("/Users/someone")),
            PathBuf::from("/Users/someone/.dsh")
        );
        assert_eq!(
            resolve_home_with(None, Some("~/work/harness"), Some("/Users/someone")),
            PathBuf::from("/Users/someone/work/harness")
        );
        assert_eq!(
            resolve_home_with(None, Some("~"), Some("/Users/someone")),
            PathBuf::from("/Users/someone")
        );
        assert_eq!(
            resolve_home_with(None, Some("/work/harness"), Some("/Users/someone")),
            PathBuf::from("/work/harness")
        );
    }

    /// The form's verdict: blank means the default, a relative path is refused,
    /// and anything accepted is created.
    #[test]
    fn a_home_is_checked_created_and_returned_absolute() {
        let scratch = Scratch::new("home-check");
        let target = scratch.0.join("harness-home");
        let checked = check_home(&target.display().to_string()).expect("an absolute path is accepted");
        assert_eq!(checked, target);
        assert!(target.is_dir(), "the home is created so the child can use it as its cwd");

        // A blank field is the default, which this test cannot create (it would
        // be the real `$HOME/.dsh`), so only the refusal is asserted here.
        assert!(check_home("relative/path").is_err());
    }

    /// The diagnostics line has to be able to say "this is only the default",
    /// because that is the difference between a stored answer and a guess.
    #[test]
    fn the_home_source_names_which_level_won() {
        assert_eq!(
            home_source_with(Some("/tmp/session"), Some("/tmp/form")),
            HomeSource::Environment
        );
        assert_eq!(home_source_with(None, Some("/tmp/form")), HomeSource::Settings);
        assert_eq!(home_source_with(None, None), HomeSource::Default);
        // Blank is the same as unset at either level, exactly as in resolution.
        assert_eq!(home_source_with(Some("  "), Some("  ")), HomeSource::Default);
        assert_eq!(home_source_with(Some("  "), Some("/tmp/form")), HomeSource::Settings);
    }

    #[test]
    fn loading_a_home_without_settings_reports_no_problem() {
        let scratch = Scratch::new("empty-home");
        let config = load(&scratch.0);
        assert!(config.settings_problems.is_empty(), "{:?}", config.settings_problems);
        // The settings location is what this call was given; the driven home is
        // resolved from the environment and the file, so it is not asserted here.
        assert_eq!(config.config_dir, scratch.0);
    }
}
