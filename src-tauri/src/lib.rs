//! dsh-app: a launcher for DeepSeek Harness profiles.
//!
//! The shell owns three things and nothing else: spawning the `dsh` executable the
//! user named, reading its readiness line, and hosting the served GUI in a
//! window. It never imports harness internals and never interprets the client
//! protocol — see `docs/design-tauri-wrapper.md` D1/D6 and
//! `docs/spec-external-dsh.md`.
//!
//! It ships no runtime: no Node, no pnpm, no closure, no update channel. That is
//! what makes it a launcher rather than a package manager, and it is why there is
//! no `bundled`/`slim` variant to choose between.
//!
//! Several profiles may run at once. That is safe because each owns a distinct
//! profile and nothing else is shared.

mod config;
mod harness_window;
mod profile;
mod settings;
mod supervisor;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use tauri::{Emitter, Manager, RunEvent};

use config::AppConfig;
use profile::DiscoveredProfile;
use supervisor::{Phase, SupervisedChild};

/// The shell's own page, compiled into the binary from `ui/`.
const SHELL_LABEL: &str = "shell";

/// Identity of the embedded shell page, stamped by `build.rs`. Surfaced in the
/// UI so a running window can be matched to a source revision.
const UI_VERSION: &str = env!("DSH_APP_UI_VERSION");

/// Application state.
struct ShellState {
    config: AppConfig,
    /// Profiles in name order; none of them is started automatically.
    profiles: Vec<DiscoveredProfile>,
    /// Live children, keyed by profile name.
    children: BTreeMap<String, SupervisedChild>,
    /// Problems found while loading state, surfaced in the UI.
    problems: Vec<String>,
    /// Whether the page has reported its first paint. The watchdog in `run`
    /// consults this before showing a window the page never showed itself.
    shell_shown: bool,
}

impl ShellState {
    /// The profile with this name.
    fn profile(&self, id: &str) -> Option<&DiscoveredProfile> {
        self.profiles.iter().find(|entry| entry.name == id)
    }

    /// One profile row for the UI.
    fn profile_row(&self, entry: &DiscoveredProfile) -> serde_json::Value {
        let id = &entry.name;
        let (phase, url, stderr_tail) = match self.children.get(id) {
            Some(child) => {
                let snapshot = child.snapshot();
                let url = match &snapshot.phase {
                    Phase::Ready { url } => Some(url.clone()),
                    _ => None,
                };
                (snapshot.phase.label().to_owned(), url, snapshot.stderr_tail)
            }
            None => ("idle".to_owned(), None, Vec::new()),
        };
        serde_json::json!({
            "id": id,
            "label": id,
            "kind": profile::kind_for_bundles(&entry.bundles),
            "startNote": start_note(entry),
            "profile": id,
            "bundles": entry.bundles,
            "source": entry.dir.display().to_string(),
            "phase": phase,
            "url": url,
            "stderrTail": stderr_tail,
        })
    }

    /// Everything the UI renders.
    ///
    /// Re-resolved per call on purpose — the executable and the home can change
    /// the moment the guide or the sheet saves, and this payload is the only
    /// place the page reads them from — but resolved **once** per call: the
    /// settings file used to be read twice, on every status event.
    fn snapshot(&self) -> serde_json::Value {
        let saved = settings::load(&self.config.config_dir);
        let (dsh, dsh_problem) = dsh_json(&config::resolve_dsh(&saved.settings));
        serde_json::json!({
            "uiVersion": UI_VERSION,
            // What the page lays its traffic lights and shortcuts out for. The
            // page never guesses at the platform.
            "platform": std::env::consts::OS,
            "home": self.config.home.display().to_string(),
            // Which level the driven home came from, so the sheet can say that a
            // value is only the default rather than something the user chose.
            "homeSource": config::home_source(&saved.settings).label(),
            // `configured: false` is guidance, not a failure: the page shows the
            // guide instead of an error.
            "configured": dsh.is_some(),
            "dsh": dsh.as_ref().and_then(|value| value["path"].as_str()),
            "dshSource": dsh.as_ref().and_then(|value| value["source"].as_str()),
            "dshProblem": dsh_problem,
            // The guide's own state, and the only thing that decides whether the
            // page shows the guide or the launcher.
            "onboarded": saved.settings.onboarded,
            "pickedProfile": saved.settings.profile,
            "settingsFile": saved.path.display().to_string(),
            "settings": {
                "dsh": saved.settings.dsh,
                "home": saved.settings.home,
                "problem": saved.problem,
            },
            "profiles": self.profiles.iter().map(|entry| self.profile_row(entry)).collect::<Vec<_>>(),
            "problems": self.problems,
        })
    }
}

/// Why one profile cannot be started, or `None` when it can.
///
/// A discovered profile states no `kind`: its bundles are the only statement of
/// what it can be launched as, so the reason names what that profile lacks
/// instead of blaming a kind the user never chose.
fn start_note(entry: &DiscoveredProfile) -> Option<String> {
    match profile::kind_for_bundles(&entry.bundles) {
        "web" => None,
        "task" => Some(format!(
            "kind \"task\" (the {} bundle) is not implemented yet",
            profile::HEADLESS_BUNDLE
        )),
        "server" => Some(format!(
            "kind \"server\" (the {} or {} bundle) is not implemented yet",
            profile::ACP_BUNDLE,
            profile::SDK_BUNDLE
        )),
        _ => Some(format!(
            "this profile has no {} bundle, so there is no GUI to serve",
            profile::WEB_BUNDLE
        )),
    }
}

/// Resolve the `dsh` the shell will launch.
///
/// Loads settings from disk rather than trusting a cached copy: the user may have
/// just saved the form, and the very next Start has to see the new answer.
fn resolve_global_dsh(config_dir: &Path) -> Result<config::DshResolution, String> {
    let report = settings::load(config_dir);
    config::resolve_dsh(&report.settings)
}

/// The executable one Start will use.
///
/// Reads the shell's **own** config directory, never the harness home it drives.
/// The two are different directories, and reading the wrong one does not fail
/// loudly: `settings::load` treats a missing file as "unconfigured", so the
/// start resolves to nothing and the button does nothing at all. That is exactly
/// what happened, so the field choice is pinned by a test.
fn launch_dsh(config: &AppConfig) -> Result<config::DshResolution, String> {
    resolve_global_dsh(&config.config_dir)
}

/// What the UI shows about the executable this shell will launch.
///
/// `value: None` means **unconfigured**, which is a state and not a failure; a
/// message is only present when something the user named is unusable.
/// Takes an already-resolved answer instead of resolving again: one payload is
/// one read of the settings file, and "which dsh" used to be read twice on every
/// status event.
fn dsh_json(resolution: &Result<config::DshResolution, String>) -> (Option<serde_json::Value>, Option<String>) {
    match resolution {
        Ok(resolution) => {
            let value = resolution.dsh().map(|dsh| {
                serde_json::json!({
                    "path": dsh.path.display().to_string(),
                    "source": dsh.source.label(),
                })
            });
            (value, None)
        }
        Err(error) => (None, Some(error.clone())),
    }
}

/// One path the user typed, as validated.
#[derive(Debug, serde::Serialize)]
struct PathCheck {
    /// The path they named.
    path: String,
    /// Whether it is missing, a directory, not executable, or not a program.
    ok: bool,
    /// The first line `--version` printed, when the path really runs.
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<String>,
    /// Why it was rejected, when it was.
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    /// Non-fatal observations, shown but never blocking.
    warnings: Vec<String>,
}

/// The verdict on what the form asked about.
#[derive(serde::Serialize)]
struct ValidationReport {
    ok: bool,
    dsh: Option<PathCheck>,
    /// The harness home that would be used, once it has been checked.
    home: Option<PathCheck>,
    /// Whether an executable is available for the shell to launch.
    configured: bool,
}

/// Check both fields of the form.
///
/// For the executable: existence, the executable bit, and `--version`. For the
/// home: a path that expands, is absolute, and can be created. Whether the
/// executable can actually boot a profile is answered by booting one — the shell
/// deliberately knows nothing about the Node it wraps, the flags it needs, or the
/// capabilities it has (spec §4, D8).
fn validate_paths(config_dir: &Path, dsh: Option<&str>, home: Option<&str>) -> ValidationReport {
    let saved = settings::load(config_dir);

    // The executable has no default and no fallback: the field starts empty and
    // a blank value is a verdict rather than a reason to reuse the saved one.
    // The asymmetry with the home below is deliberate — the home has a default,
    // the executable does not.
    let dsh_check = Some(match dsh.map(str::trim) {
        Some("") | None => missing_dsh(),
        Some(value) => check_dsh(value),
    });
    let home_check = match home.map(str::trim) {
        Some("") | None => saved.settings.home.as_deref().map(check_home),
        Some(value) => Some(check_home(value)),
    };

    // With an invalid path saved, resolution errors; with none saved, it is
    // simply unconfigured. Both are "not launchable", reported the same way.
    let resolution = resolve_global_dsh(config_dir);
    ValidationReport {
        ok: dsh_check.as_ref().is_none_or(|check| check.ok)
            && home_check.as_ref().is_none_or(|check| check.ok),
        dsh: dsh_check,
        home: home_check,
        configured: resolution
            .as_ref()
            .map(|resolution| resolution.dsh().is_some())
            .unwrap_or(false),
    }
}

/// Save the form, refusing to store anything that does not work.
///
/// Nothing else has to be activated afterwards: the next Start resolves both
/// values again from disk, so there is no seed to install and no update to retry.
#[tauri::command]
fn save_settings(
    window: tauri::Window,
    state: tauri::State<'_, Mutex<ShellState>>,
    dsh: String,
    home: String,
) -> Result<ValidationReport, String> {
    ensure_shell(&window)?;
    let config_dir = lock_state(&state)?.config.config_dir.clone();
    let report = validate_paths(&config_dir, Some(&dsh), Some(&home));
    if !report.ok {
        return Ok(report);
    }
    settings::save(&config_dir, &dsh, &home)?;
    // The write just made these the configured values, so report the state the
    // shell is now in rather than the one it was in before the save — and adopt
    // the home it named, because the guide's next step lists *that* home's
    // profiles.
    {
        let mut state = lock_state(&state)?;
        adopt_settings(&mut state);
    }
    Ok(ValidationReport {
        configured: true,
        ..report
    })
}

/// Refuse a command that may only be issued from the shell's own page.
///
/// The ACL does not gate app commands, and the harness window runs a page this
/// app did not write, so anything that writes the shell's configuration has to
/// ask who is calling. @see docs/spec-shell-ux.md §9.11
fn ensure_shell(window: &tauri::Window) -> Result<(), String> {
    if window.label() == SHELL_LABEL {
        return Ok(());
    }
    Err(format!("{} may not configure the shell", window.label()))
}

/// Re-resolve everything a settings file implies: the driven home, and the
/// profiles under it.
///
/// Called after every save, because until this runs the shell keeps driving the
/// previous home — which used to be invisible and is now the guide's own second
/// step.
fn adopt_settings(state: &mut ShellState) {
    state.config = config::load(&state.config.config_dir);
    let mut problems = state.config.settings_problems.clone();
    if let Err(error) = std::fs::create_dir_all(&state.config.home) {
        problems.push(format!("cannot create {}: {error}", state.config.home.display()));
    }
    let (profiles, discovery) = profile::discover(&state.config.home);
    state.profiles = profiles;
    state.problems = problems.into_iter().chain(discovery).collect();
}

/// The verdict for a blank executable field.
fn missing_dsh() -> PathCheck {
    PathCheck {
        path: String::new(),
        ok: false,
        version: None,
        error: Some("a dsh executable is required".to_owned()),
        warnings: Vec::new(),
    }
}

/// Check one harness home, as a {@link PathCheck} for the form.
fn check_home(value: &str) -> PathCheck {
    let mut check = PathCheck {
        path: value.trim().to_owned(),
        ok: false,
        version: None,
        error: None,
        warnings: Vec::new(),
    };
    match config::check_home(value) {
        Ok(path) => {
            check.ok = true;
            check.path = path.display().to_string();
        }
        Err(error) => check.error = Some(error),
    }
    check
}

/// Check one path: exists, executable, and actually runs.
fn check_dsh(value: &str) -> PathCheck {
    let path = PathBuf::from(value.trim());
    let mut check = PathCheck {
        path: path.display().to_string(),
        ok: false,
        version: None,
        error: None,
        warnings: Vec::new(),
    };
    if let Err(error) = check_executable(&path) {
        check.error = Some(error);
        return check;
    }

    match run_version(&path) {
        Ok(text) => {
            check.ok = true;
            check.version = Some(text);
        }
        // A path that exists and is executable but cannot run at all: reject,
        // because everything downstream would fail confusingly.
        Err(error) => check.error = Some(error),
    }
    check
}

/// Whether the path is a file the current user may execute.
fn check_executable(path: &Path) -> Result<(), String> {
    let metadata = std::fs::metadata(path).map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => format!("{} does not exist", path.display()),
        _ => format!("cannot read {}: {error}", path.display()),
    })?;
    if metadata.is_dir() {
        return Err(format!("{} is a directory", path.display()));
    }
    if !config::usable(path) {
        return Err(format!("{} is not executable", path.display()));
    }
    Ok(())
}

/// Run `<path> --version`, returning the first line it prints.
///
/// Used as the "does this run at all" check. For dsh that line is its version,
/// which is also the only version fact the shell will ever have — recorded so a
/// failure report can say which installation was in use.
fn run_version(path: &Path) -> Result<String, String> {
    let output = std::process::Command::new(path)
        .arg("--version")
        .output()
        .map_err(|error| format!("cannot run {}: {error}", path.display()))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        let detail = stderr
            .lines()
            .chain(stdout.lines())
            .find(|line| !line.trim().is_empty())
            .unwrap_or("it printed nothing");
        return Err(format!("{} --version failed: {detail}", path.display()));
    }
    let version = stdout
        .lines()
        .chain(stderr.lines())
        .map(str::trim)
        .find(|line| !line.is_empty())
        .ok_or_else(|| format!("{} --version printed nothing", path.display()))?;
    Ok(version.to_owned())
}

/// Record a problem and let the UI refresh.
fn record_problem(app: &tauri::AppHandle, message: String) {
    eprintln!("[dsh-app] {message}");
    if let Ok(mut state) = app.state::<Mutex<ShellState>>().lock() {
        state.problems.push(message.clone());
    }
    let _ = app.emit(
        "dsh://status",
        serde_json::json!({ "phase": "failed", "error": message }),
    );
}

/// Lock the shell state, converting a poisoned mutex into a readable error.
fn lock_state<'a>(state: &'a tauri::State<'_, Mutex<ShellState>>) -> Result<MutexGuard<'a, ShellState>, String> {
    state
        .lock()
        .map_err(|error| format!("shell state is unavailable: {error}"))
}

/// Record what the shell page reported about itself.
#[tauri::command]
fn ui_probe(marker: String) {
    eprintln!("[dsh-app] page-probe {marker}");
}

/// The current UI snapshot.
///
/// `DSH_APP_DEBUG_STATE=1` also prints the payload to stderr. The shell owns no
/// window we can inspect from a terminal, so this is how the page's input is made
/// observable when a field renders unexpectedly.
#[tauri::command]
fn shell_state(state: tauri::State<'_, Mutex<ShellState>>) -> serde_json::Value {
    let payload = match state.lock() {
        Ok(state) => state.snapshot(),
        Err(error) => serde_json::json!({ "problems": [error.to_string()], "profiles": [] }),
    };
    if std::env::var("DSH_APP_DEBUG_STATE").is_ok_and(|value| value == "1") {
        eprintln!("[dsh-app] shell_state {}", serde_json::to_string(&payload).unwrap_or_default());
    }
    payload
}

/// Open one profile's served GUI (or focus it when already open).
#[tauri::command]
fn open_harness(
    app: tauri::AppHandle,
    profile_id: String,
    state: tauri::State<'_, Mutex<ShellState>>,
) -> Result<(), String> {
    let url = {
        let state = lock_state(&state)?;
        state
            .children
            .get(&profile_id)
            .and_then(|child| match child.snapshot().phase {
                Phase::Ready { url } => Some(url),
                _ => None,
            })
            .ok_or_else(|| format!("profile {profile_id} is not ready"))?
    };
    harness_window::open(&app, &profile_id, &url)
}

/// Stop one profile's runtime and close its window.
#[tauri::command]
fn stop_profile(
    app: tauri::AppHandle,
    profile_id: String,
    state: tauri::State<'_, Mutex<ShellState>>,
) -> Result<(), String> {
    {
        let mut state = lock_state(&state)?;
        if let Some(mut child) = state.children.remove(&profile_id) {
            child.shutdown();
        }
    }
    harness_window::close(&app, &profile_id);
    Ok(())
}

/// Reload the profile list from disk, reporting anything unreadable.
#[tauri::command]
fn reload_profiles(state: tauri::State<'_, Mutex<ShellState>>) -> Result<(), String> {
    let mut state = lock_state(&state)?;
    adopt_settings(&mut state);
    Ok(())
}

/// Move, minimize, zoom, drag or close one of this app's windows.
///
/// The shell's chrome is a page, so every button on it lands here — including
/// the three lights and the drag surface of the harness window, which is why
/// this command is open to every window while the configuration commands are
/// not. The action is validated rather than forwarded: the harness page can call
/// it too, and an unknown string is a bug rather than a no-op.
/// Show the shell window once its page has painted.
///
/// The window is created hidden so the first frame is the styled page and not a
/// white flash. The watchdog in `run` shows it anyway if this never arrives: an
/// invisible app with a broken page is the worst failure mode this shell has.
#[tauri::command]
fn shell_ready(app: tauri::AppHandle, state: tauri::State<'_, Mutex<ShellState>>) -> Result<(), String> {
    if let Ok(mut state) = state.lock() {
        state.shell_shown = true;
    }
    let Some(window) = app.get_webview_window(SHELL_LABEL) else {
        return Err("the shell window does not exist".to_owned());
    };
    window.show().map_err(|error| error.to_string())?;
    let _ = window.set_focus();
    Ok(())
}

/// Record which profile the guide ended on, and that it ended.
#[tauri::command]
fn finish_onboarding(
    window: tauri::Window,
    state: tauri::State<'_, Mutex<ShellState>>,
    profile: Option<String>,
) -> Result<(), String> {
    ensure_shell(&window)?;
    let (config_dir, known) = {
        let state = lock_state(&state)?;
        let known = profile.as_deref().is_some_and(|name| state.profile(name).is_some());
        (state.config.config_dir.clone(), known)
    };
    // A name the shell does not know is recorded as none rather than refused:
    // the profile may have been deleted between the guide listing it and the
    // button being pressed, and that is not worth failing the guide over.
    settings::save_onboarding(&config_dir, if known { profile.as_deref() } else { None })?;
    Ok(())
}

/// Clear the problems the launcher's banner is showing.
#[tauri::command]
fn dismiss_problems(window: tauri::Window, state: tauri::State<'_, Mutex<ShellState>>) -> Result<(), String> {
    ensure_shell(&window)?;
    lock_state(&state)?.problems.clear();
    Ok(())
}

/// Record a startup failure and notify the UI.
fn fail_profile(app: &tauri::AppHandle, message: &str) {
    record_problem(app, message.to_owned());
}

/// Spawn the launcher for `profile_id` and, once ready, open its window.
///
/// Runs on a worker thread: booting dsh takes seconds and must not block the
/// event loop that draws the window. The command name is the one the page
/// invokes — no `_command` suffix, because a mismatch between the two halves of
/// the IPC surface cannot be type-checked and would show up as a dead button.
fn spawn_profile(app: tauri::AppHandle, profile_id: String) {
    std::thread::spawn(move || {
        let handle = app.state::<Mutex<ShellState>>();
        let config = {
            let Ok(state) = handle.lock() else {
                eprintln!("[dsh-app] shell state is unavailable");
                return;
            };
            if state.children.contains_key(&profile_id) {
                return;
            }
            let Some(entry) = state.profile(&profile_id) else {
                eprintln!("[dsh-app] no profile named {profile_id}");
                return;
            };
            // Only the web carrier is implemented, and the reason is worth
            // saying rather than failing somewhere inside the child (D3.3).
            if let Some(note) = start_note(entry) {
                drop(state);
                fail_profile(&app, &format!("cannot start {profile_id}: {note}"));
                return;
            }
            state.config.clone()
        };

        let resolved = match launch_dsh(&config) {
            Ok(config::DshResolution::Configured(dsh)) => dsh,
            // Not an error: the shell is waiting for the user to name an
            // executable. Say where to do it instead of showing a failure.
            Ok(config::DshResolution::Unconfigured) => {
                let _ = app.emit(
                    "dsh://status",
                    serde_json::json!({
                        "profileId": profile_id,
                        "phase": "unconfigured",
                        "field": "dsh",
                        "error": config::unconfigured_message(),
                    }),
                );
                return;
            }
            Err(error) => {
                fail_profile(&app, &error);
                return;
            }
        };

        let _ = app.emit("dsh://status", serde_json::json!({ "profileId": profile_id }));
        eprintln!(
            "[dsh-app] starting profile {profile_id} with {} ({})",
            resolved.path.display(),
            resolved.source.label()
        );

        // The harness boots from any directory and never writes to its
        // installation, so the home is used only to give the child a stable cwd
        // that is the shell's own state directory as well.
        let child = SupervisedChild::start(
            &profile_id,
            supervisor::Launch {
                dsh: resolved.path.clone(),
                cwd: config.home.clone(),
                home: config.home.clone(),
            },
        );

        match child {
            Ok(child) => {
                let snapshot = child.snapshot();
                let url = match &snapshot.phase {
                    Phase::Ready { url } => Some(url.clone()),
                    _ => None,
                };
                let tail = snapshot.stderr_tail.clone();
                if let Ok(mut state) = handle.lock() {
                    state.children.insert(profile_id.clone(), child);
                }
                match url {
                    Some(url) => {
                        let _ = app.emit(
                            "dsh://status",
                            serde_json::json!({ "profileId": profile_id, "phase": "ready", "url": url }),
                        );
                        if let Err(error) = harness_window::open(&app, &profile_id, &url) {
                            eprintln!("[dsh-app] {error}");
                        }
                    }
                    None => {
                        // Boot failed: hand the UI the child's own diagnostics so
                        // the failure explains itself instead of only hitting a log.
                        let _ = app.emit(
                            "dsh://status",
                            serde_json::json!({ "profileId": profile_id, "phase": "failed", "stderrTail": tail }),
                        );
                    }
                }
            }
            Err(error) => {
                eprintln!("[dsh-app] {error}");
                fail_profile(&app, &error);
            }
        }
    });
}

/// Start one profile from the UI.
#[tauri::command]
fn start_profile(app: tauri::AppHandle, profile_id: String) -> Result<(), String> {
    spawn_profile(app, profile_id);
    Ok(())
}

/// Report what the shell page actually sees, on stdout.
///
/// `DSH_APP_DEBUG_STATE=1` makes the shell evaluate a probe in its own window
/// after load and print the answer. The shell has no terminal-visible surface, so
/// without a channel like this a page that fails to run is indistinguishable from
/// one that never loaded — which is exactly how a broken bridge once hid behind a
/// screen of placeholder dashes.
fn install_page_probe(app: &tauri::AppHandle) {
    if !std::env::var("DSH_APP_DEBUG_STATE").is_ok_and(|value| value == "1") {
        return;
    }
    let Some(window) = app.get_webview_window(SHELL_LABEL) else {
        eprintln!("[dsh-app] page-probe: the shell window does not exist");
        return;
    };

    // A module that throws while evaluating reports nothing on its own, so the
    // reason has to be captured in the page before the probe reads it.
    let capture = r#"
      (() => {
        const record = (what) => (event) => {
          const detail = event?.error?.stack || event?.reason?.stack
            || event?.message || String(event?.reason ?? event);
          window.__DSH_PAGE_ERROR__ = what + ': ' + detail;
        };
        window.addEventListener('error', record('error'), true);
        window.addEventListener('unhandledrejection', record('rejection'), true);
      })()
    "#;
    let _ = window.eval(capture);
    // `eval` is host-injected, so unlike an inline attribute it is not subject to
    // the page's CSP.
    let probe = r#"
      (() => {
        const seen = {
          internals: typeof window.__TAURI_INTERNALS__,
          invoke: typeof window.__TAURI_INTERNALS__?.invoke,
          global: typeof window.__TAURI__,
          // Which surface the state machine produced. Without it, "the guide did
          // not appear" and "the guide appeared and was skipped" look the same.
          view: document.documentElement.dataset.view ?? null,
          scripts: [...document.scripts].map((s) => s.src || '(inline)'),
          error: window.__DSH_PAGE_ERROR__ ?? null,
        };
        window.__TAURI_INTERNALS__.invoke('ui_probe', { marker: JSON.stringify(seen) })
          .catch((error) => { document.title = 'probe failed: ' + error; });
      })()
    "#;
    let injected = window.eval(probe);
    eprintln!("[dsh-app] page-probe injected: {}", injected.is_ok());

    // `DSH_APP_SELFTEST=<profile>` drives one Start from the page and reports
    // what the page saw. A launcher is a GUI with no terminal, so the only way
    // to exercise "the button really starts something" from a script is to have
    // the page press it — and without this, a broken IPC surface looks exactly
    // like a button that was never clicked.
    if let Ok(wanted) = std::env::var("DSH_APP_SELFTEST") {
        if !wanted.trim().is_empty() {
            let quoted = serde_json::to_string(wanted.trim()).unwrap_or_else(|_| "\"\"".to_owned());
            let script = format!(
                r#"
      (async () => {{
        const call = window.__TAURI_INTERNALS__.invoke;
        const seen = {{ selftest: 'starting' }};
        try {{
          await call('start_profile', {{ profileId: {quoted} }});
          seen.selftest = 'invoked';
        }} catch (error) {{
          seen.selftest = 'rejected: ' + String(error);
        }}
        for (let i = 0; i < 25; i += 1) {{
          await new Promise((resolve) => setTimeout(resolve, 1000));
          try {{
            const now = await call('shell_state');
            const row = (now.profiles || []).find((entry) => entry.id === {quoted});
            if (row && (row.phase === 'ready' || row.phase === 'failed' || row.phase === 'exited')) {{
              seen.phase = row.phase;
              seen.url = row.url;
              seen.tail = (row.stderrTail || []).slice(-6);
              break;
            }}
          }} catch (error) {{ seen.poll = String(error); }}
        }}
        seen.phase = seen.phase || 'still-starting';
        await call('ui_probe', {{ marker: JSON.stringify(seen) }}).catch(() => {{}});
      }})()
    "#
            );
            let _ = window.eval(script);
            eprintln!("[dsh-app] page-selftest injected for {wanted}");
        }
    }
}

/// Build and run the shell.
pub fn run() {
    let mut problems = Vec::new();

    // The shell's own configuration lives in its own directory — not in the home
    // it drives — so it is known before anything is read from it.
    let config_dir = settings::config_dir();
    if let Err(error) = std::fs::create_dir_all(&config_dir) {
        problems.push(format!("cannot create {}: {error}", config_dir.display()));
    }

    // Recorded because a packaged app has no terminal: "which dsh is it using" is
    // the first question whenever a profile does not come up.
    match resolve_global_dsh(&config_dir) {
        Ok(config::DshResolution::Configured(dsh)) => eprintln!(
            "[dsh-app] {} will launch {} ({})",
            settings::settings_path(&config_dir).display(),
            dsh.path.display(),
            dsh.source.label()
        ),
        Ok(config::DshResolution::Unconfigured) => eprintln!(
            "[dsh-app] no dsh is configured; the shell is waiting for the executable path field"
        ),
        Err(error) => eprintln!("[dsh-app] {error}"),
    }

    let config = config::load(&config_dir);
    problems.extend(config.settings_problems.iter().cloned());

    // The home the shell drives: the child needs it as its working directory and
    // as its `DSH_HOME`, so it has to exist even when nothing has been launched.
    eprintln!("[dsh-app] harness home {}", config.home.display());
    if let Err(error) = std::fs::create_dir_all(&config.home) {
        problems.push(format!("cannot create {}: {error}", config.home.display()));
    }

    let (profiles, profile_problems) = profile::discover(&config.home);
    eprintln!("[dsh-app] {} profile(s) listed", profiles.len());
    problems.extend(profile_problems);

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            // `show` as well as focus: a second launch during the hidden start-up
            // window would otherwise be a no-op.
            if let Some(window) = app.get_webview_window(SHELL_LABEL) {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .manage(Mutex::new(ShellState {
            config,
            profiles,
            children: BTreeMap::new(),
            problems,
            shell_shown: false,
        }))
        .invoke_handler(tauri::generate_handler![
            shell_state,
            ui_probe,
            open_harness,
            stop_profile,
            start_profile,
            reload_profiles,
            save_settings,
            shell_ready,
            finish_onboarding,
            dismiss_problems
        ])
        .setup(move |app| {
            // Nothing is started here on purpose: the launcher opens on its list,
            // and a runtime only exists once the user asks for one. The shell
            // therefore never picks a profile on the user's behalf, and a first
            // run with nothing configured stays a form instead of a failure.
            let probe_handle = app.handle().clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(6));
                install_page_probe(&probe_handle);
            });

            // The window is created hidden, and the page shows it once it has
            // painted. If that never happens the app would be invisible with
            // nothing anywhere saying why, so the window is shown regardless —
            // late, and with a line on stderr.
            let watchdog = app.handle().clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(3));
                let shown = watchdog
                    .state::<Mutex<ShellState>>()
                    .lock()
                    .map(|state| state.shell_shown)
                    .unwrap_or(false);
                if shown {
                    return;
                }
                eprintln!("[dsh-app] the page never reported ready; showing the window anyway");
                if let Some(window) = watchdog.get_webview_window(SHELL_LABEL) {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            });
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("failed to build the dsh-app shell");

    app.run(|app_handle, event| {
        if let RunEvent::ExitRequested { .. } | RunEvent::Exit = event {
            // Stop every supervised child concurrently before the process leaves:
            // none may outlive its window. If the shell is SIGKILLed instead, the
            // child's stdin EOF is the remaining guard.
            // The children are taken out under the lock and joined after it is
            // released, so a slow SIGTERM grace period never blocks another event
            // handler that needs the same state.
            let handle = app_handle.state::<Mutex<ShellState>>();
            let children: Vec<SupervisedChild> = match handle.lock() {
                Ok(mut state) => std::mem::take(&mut state.children).into_values().collect(),
                Err(_) => Vec::new(),
            };
            drop(handle);
            let workers: Vec<std::thread::JoinHandle<()>> = children
                .into_iter()
                .map(|mut child| std::thread::spawn(move || child.shutdown()))
                .collect();
            for worker in workers {
                let _ = worker.join();
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// A throwaway harness home, removed when the test ends.
    struct TempHome(PathBuf);

    impl TempHome {
        fn new(label: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("dsh-app-lib-{label}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(dir.join(profile::PROFILES_DIR)).expect("create the test home");
            Self(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        /// Write one profile manifest, in the shape upstream `initProfile` writes.
        fn profile(&self, name: &str, bundles: &[&str]) {
            let dir = self.0.join(profile::PROFILES_DIR).join(name);
            fs::create_dir_all(&dir).expect("create the profile directory");
            let manifest = serde_json::json!({ "dsh": { "profile": { "bundles": bundles } } });
            fs::write(dir.join("package.json"), manifest.to_string()).expect("write the manifest");
        }

        /// An executable that answers `--version` the way dsh does.
        fn fake_dsh(&self, name: &str, version: &str) -> PathBuf {
            let path = self.0.join(name);
            fs::write(&path, format!("#!/bin/sh\necho {version}\n")).expect("write the fake dsh");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod");
            }
            path
        }
    }

    impl Drop for TempHome {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn entry(name: &str, bundles: &[&str]) -> DiscoveredProfile {
        DiscoveredProfile {
            name: name.to_owned(),
            dir: PathBuf::from("/tmp").join(name),
            bundles: bundles.iter().map(|value| (*value).to_owned()).collect(),
        }
    }

    /// A start resolves the executable from the shell's config directory, not
    /// from the home it drives. When those were confused the start silently
    /// became "unconfigured" — no error, no process, just a dead button.
    #[test]
    fn a_start_resolves_the_executable_from_the_config_directory() {
        let config_dir = TempHome::new("start-config-dir");
        let driven_home = TempHome::new("start-driven-home");
        let dsh = config_dir.fake_dsh("dsh", "0.1.6-alpha.2");
        // The config names the executable and drives a home that has no config of
        // its own — the shape that used to resolve to nothing.
        settings::save(
            config_dir.path(),
            &dsh.display().to_string(),
            &driven_home.path().display().to_string(),
        )
        .expect("the save succeeds");

        let config = config::load(config_dir.path());
        assert_eq!(config.config_dir, config_dir.path());
        // Which directory the driven home resolves to depends on the session
        // (DSH_HOME wins), so the invariant asserted here is the one that
        // matters: the two roles are different directories and must not be
        // confused for one another.
        assert_ne!(config.home, config.config_dir);
        assert!(
            !config.home.join("config.toml").exists(),
            "the driven home has no config of its own, which is what made the mistake silent"
        );
        match launch_dsh(&config) {
            Ok(config::DshResolution::Configured(resolved)) => assert_eq!(resolved.path, dsh),
            other => panic!("a start must find the configured executable, got {other:?}"),
        }
    }

    /// The IPC surface has two halves that cannot be checked against each other:
    /// the `invoke('…')` calls in the page, and the commands registered below.
    /// A mismatch is invisible — the button simply does nothing — so it is
    /// asserted here instead. Neither half is restated: the command list is read
    /// out of this file, and the call sites out of every `.js` file under `ui/`,
    /// so the scan cannot go stale as the page gains modules.
    #[test]
    fn every_command_the_page_invokes_is_registered() {
        let source = include_str!("lib.rs");
        let marker = concat!("#[tauri::", "command]");
        let declared: Vec<&str> = source
            .match_indices(marker)
            .filter_map(|(at, _)| source[at..].split_once("fn ").map(|(_, rest)| rest))
            .map(|rest| rest.split(|c: char| !(c.is_alphanumeric() || c == '_')).next().unwrap_or(""))
            .collect();

        let mut invoked: Vec<String> = Vec::new();
        for entry in std::fs::read_dir("../ui").expect("the ui directory is readable") {
            let path = entry.expect("a directory entry").path();
            if path.extension().is_none_or(|extension| extension != "js") {
                continue;
            }
            let page = std::fs::read_to_string(&path).expect("a page file is readable");
            invoked.extend(page_invocations(&page));
        }

        assert!(!declared.is_empty(), "no commands were found; the scan broke, do not delete it");
        assert!(!invoked.is_empty(), "the page invokes nothing; the scan broke, do not delete it");
        for name in &invoked {
            assert!(
                declared.contains(&name.as_str()),
                "a page file invokes `{name}`, which is not a registered command; registered: {declared:?}"
            );
        }
    }

    /// The command names one page file invokes, minus the plugin protocol.
    fn page_invocations(page: &str) -> Vec<String> {
        page.match_indices("invoke('")
            .filter_map(|(at, _)| page[at..].split_once("invoke('").map(|(_, rest)| rest))
            .map(|rest| rest.split('\'').next().unwrap_or("").to_owned())
            .filter(|name| !name.starts_with("plugin:"))
            .collect()
    }

    /// The only carrier this shell drives is the web one; everything else says so
    /// instead of starting a process that can never print a URL.
    #[test]
    fn only_a_web_profile_can_be_started() {
        assert_eq!(start_note(&entry("web", &[profile::WEB_BUNDLE])), None);
        assert!(start_note(&entry("t", &[profile::HEADLESS_BUNDLE])).is_some_and(|note| note.contains("task")));
        assert!(start_note(&entry("s", &[profile::ACP_BUNDLE])).is_some_and(|note| note.contains("server")));
        assert!(start_note(&entry("bare", &["@deepseek-ai/dsh-base"]))
            .is_some_and(|note| note.contains(profile::WEB_BUNDLE)));
    }

    /// A shell nobody has configured is a state, not a failure: the payload says
    /// `configured: false` and carries no problem.
    #[test]
    fn an_unconfigured_home_reports_no_problem() {
        let home = TempHome::new("unconfigured");
        let (dsh, problem) = dsh_json(&config::resolve_dsh(&settings::load(home.path()).settings));
        assert!(dsh.is_none());
        assert!(problem.is_none(), "{problem:?}");
    }

    #[test]
    fn a_configured_executable_is_reported_with_its_source() {
        let home = TempHome::new("configured");
        let path = home.fake_dsh("dsh", "0.1.6-alpha.2");
        settings::save(home.path(), &path.display().to_string(), "").expect("the save succeeds");

        let (dsh, problem) = dsh_json(&config::resolve_dsh(&settings::load(home.path()).settings));
        assert!(problem.is_none(), "{problem:?}");
        let dsh = dsh.expect("a dsh is configured");
        assert_eq!(dsh["path"], path.display().to_string());
        assert_eq!(dsh["source"], "settings");
    }

    /// A saved path that stopped working is an error the form shows, not a
    /// silent fall-through to "unconfigured".
    #[test]
    fn a_saved_path_that_stopped_working_is_an_error() {
        let home = TempHome::new("stale");
        let missing = home.path().join("gone");
        settings::save(home.path(), &missing.display().to_string(), "").expect("the save succeeds");

        let (dsh, problem) = dsh_json(&config::resolve_dsh(&settings::load(home.path()).settings));
        assert!(dsh.is_none());
        assert!(
            problem.as_ref().is_some_and(|message| message.contains("not an executable file")),
            "{problem:?}"
        );
    }

    /// The whole form contract: a runnable executable validates, and validation
    /// records the version it printed without judging it.
    #[test]
    fn a_runnable_executable_validates_and_reports_its_version() {
        let home = TempHome::new("validate-ok");
        let path = home.fake_dsh("dsh", "0.1.6-alpha.2");

        let report = validate_paths(home.path(), Some(&path.display().to_string()), None);
        assert!(report.ok);
        // Nothing is written yet, so the shell is still unconfigured: the verdict
        // on the field and the state of the machine are two different facts.
        assert!(!report.configured);
        let check = report.dsh.expect("a check was made");
        assert!(check.ok);
        assert_eq!(check.version.as_deref(), Some("0.1.6-alpha.2"));
        assert!(check.warnings.is_empty());
    }

    #[test]
    fn a_missing_or_unrunnable_executable_is_rejected() {
        let home = TempHome::new("validate-bad");

        let missing = validate_paths(home.path(), Some("/definitely/not/here/dsh"), None);
        assert!(!missing.ok);
        assert!(!missing.configured);
        assert!(missing.dsh.expect("a check was made").error.is_some_and(|e| e.contains("does not exist")));

        // Executable but not runnable: the shebang names an interpreter that does
        // not exist, which is what a broken install looks like.
        let broken = home.path().join("broken");
        fs::write(&broken, "#!/nonexistent/interpreter\n").expect("write");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&broken, fs::Permissions::from_mode(0o755)).expect("chmod");
        }
        let report = validate_paths(home.path(), Some(&broken.display().to_string()), None);
        assert!(!report.ok);
        assert!(report.dsh.expect("a check was made").error.is_some());
    }

    /// The home field is checked like the executable: a relative path is a
    /// verdict the form shows, and an absolute one is created for the child.
    #[test]
    fn the_home_field_is_checked_and_created() {
        let home = TempHome::new("home-field");
        let dsh = home.fake_dsh("dsh", "0.1.6-alpha.2");
        let target = home.path().join("harness");
        let report = validate_paths(
            home.path(),
            Some(&dsh.display().to_string()),
            Some(&target.display().to_string()),
        );
        assert!(report.ok, "{:?}", report.home.as_ref().and_then(|check| check.error.clone()));
        assert!(target.is_dir(), "an accepted home is created so the child can use it");

        let relative = validate_paths(home.path(), Some(&dsh.display().to_string()), Some("relative/home"));
        assert!(!relative.ok);
        assert!(relative.home.expect("a check was made").error.is_some());
    }

    /// The executable has no default and no fallback — that is the ask, in code.
    #[test]
    fn a_blank_executable_is_required_even_with_one_saved() {
        let home = TempHome::new("blank-field");
        let path = home.fake_dsh("dsh", "0.1.6-alpha.2");
        settings::save(home.path(), &path.display().to_string(), "").expect("the save succeeds");

        let report = validate_paths(home.path(), Some("   "), None);
        assert!(!report.ok);
        let check = report.dsh.expect("a check was made");
        assert!(!check.ok);
        assert!(check.error.as_deref().is_some_and(|error| error.contains("required")), "{check:?}");
        // A saved value is still a working runtime — the verdict is about the
        // field the user emptied, not about the machine.
        assert!(report.configured);
    }

    /// Every profile the harness has created is listed, and none of them is
    /// started on its own.
    #[test]
    fn discovered_profiles_become_rows_without_starting_anything() {
        let home = TempHome::new("rows");
        home.profile("web", &["@deepseek-ai/dsh-base", profile::WEB_BUNDLE]);
        home.profile("demo", &["@deepseek-ai/dsh-base", profile::WEB_BUNDLE]);
        home.profile("headless", &[profile::HEADLESS_BUNDLE]);

        let (profiles, problems) = profile::discover(home.path());
        assert!(problems.is_empty(), "{problems:?}");
        let state = ShellState {
            config: config::load(home.path()),
            profiles,
            children: BTreeMap::new(),
            problems: Vec::new(),
            shell_shown: false,
        };
        let snapshot = state.snapshot();
        let rows = snapshot["profiles"].as_array().expect("rows");
        // Name order: demo, headless, web — and none of them is running.
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0]["id"], "demo");
        assert_eq!(rows[0]["kind"], "web");
        assert_eq!(rows[0]["phase"], "idle");
        assert_eq!(rows[0]["startNote"], serde_json::Value::Null);
        // The headless profile is listed but cannot be started, and says why.
        assert_eq!(rows[1]["id"], "headless");
        assert_eq!(rows[1]["kind"], "task");
        assert!(rows[1]["startNote"].is_string());
        assert_eq!(rows[2]["id"], "web");
        assert_eq!(rows[2]["startNote"], serde_json::Value::Null);
    }

    /// Step 2 of the guide lists the home step 1 just saved, so a save has to
    /// adopt it in the same breath: a home resolved once at startup would show
    /// the previous directory's profiles at exactly the moment they matter.
    #[test]
    fn adopting_settings_re_reads_the_home_and_its_profiles() {
        let config_dir = TempHome::new("adopt-config");
        let first = TempHome::new("adopt-first");
        let second = TempHome::new("adopt-second");
        second.profile("web", &[profile::WEB_BUNDLE]);
        let dsh = config_dir.fake_dsh("dsh", "0.1.6-alpha.2");

        settings::save(
            config_dir.path(),
            &dsh.display().to_string(),
            &first.path().display().to_string(),
        )
        .expect("the first save succeeds");
        let mut state = ShellState {
            config: config::load(config_dir.path()),
            profiles: Vec::new(),
            children: BTreeMap::new(),
            problems: Vec::new(),
            shell_shown: false,
        };

        settings::save(
            config_dir.path(),
            &dsh.display().to_string(),
            &second.path().display().to_string(),
        )
        .expect("the second save succeeds");
        adopt_settings(&mut state);

        // What adoption must produce is what a fresh process would see.
        let fresh = config::load(config_dir.path());
        assert_eq!(state.config.home, fresh.home);
        assert_eq!(state.profiles.len(), profile::discover(&fresh.home).0.len());
        // `DSH_HOME` outranks the file by design, so the literal path is asserted
        // only in a process with no session override.
        if std::env::var("DSH_HOME").is_err() {
            assert_eq!(state.config.home, second.path());
            assert_eq!(
                state.profiles.iter().map(|entry| entry.name.as_str()).collect::<Vec<_>>(),
                vec!["web"]
            );
        }
    }

    /// X1: the harness window's page gets host chrome and nothing else.
    ///
    /// The window is borderless, so the shell writes exactly two things into a page
    /// it does not own: the marker attribute the runtime documents for a macOS
    /// host, and a listener that drags the window from the band that marker
    /// reserves. It may not draw, restyle or read the runtime's UI, and it may only
    /// call plugin commands — this app's own commands are unreachable from a remote
    /// origin (docs/spec-shell-ux.md §2.5).
    #[test]
    fn the_harness_window_writes_only_host_chrome() {
        let source = include_str!("harness_window.rs");
        assert_eq!(
            source.matches("initialization_script").count(),
            1,
            "the harness window injects one script, not several"
        );
        assert!(source.contains("include_str!(\"../../ui/harness-host.js\")"));
        assert!(!source.contains(".eval("), "no eval: the page is written to once");

        let host = include_str!("../../ui/harness-host.js");
        assert!(
            host.contains("dataset.platform = 'darwin'"),
            "the script must set the marker the runtime documents"
        );
        for forbidden in [
            "document.body",
            "getComputedStyle",
            "styleSheets",
            "insertRule",
            "createElement('style')",
            "querySelector(",
            ".append(",
            "innerHTML",
        ] {
            assert!(
                !host.contains(forbidden),
                "the host script must not use `{forbidden}`; see docs/spec-shell-ux.md §2.5"
            );
        }
        // Every window command goes through the plugin, never through this app:
        // the runtime's page is a remote origin and the app's commands are closed
        // to it by construction.
        assert!(host.contains("invoke(`plugin:window|${command}`"));
        for (at, _) in host.match_indices("invoke('") {
            let name = host[at..]
                .split_once("invoke('")
                .and_then(|(_, rest)| rest.split('\'').next())
                .unwrap_or("");
            assert!(
                name.starts_with("plugin:"),
                "the host script invokes `{name}`, which is not a plugin command"
            );
        }
    }
}
