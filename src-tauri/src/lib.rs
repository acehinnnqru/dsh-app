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
mod profile;
mod settings;
mod supervisor;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use tauri::{Emitter, Manager, RunEvent, WebviewUrl, WebviewWindowBuilder};

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
    fn snapshot(&self) -> serde_json::Value {
        // Re-resolved per call on purpose: the executable can change the moment
        // the user saves the form, and the payload is the only place the page
        // reads it from.
        let (dsh, dsh_problem) = dsh_report(&self.config.config_dir);
        let saved = settings::load(&self.config.config_dir);
        serde_json::json!({
            "uiVersion": UI_VERSION,
            "home": self.config.home.display().to_string(),
            // `configured: false` is guidance, not a failure: the page shows the
            // form instead of an error.
            "configured": dsh.is_some(),
            "dsh": dsh.as_ref().and_then(|value| value["path"].as_str()),
            "dshSource": dsh.as_ref().and_then(|value| value["source"].as_str()),
            "dshProblem": dsh_problem,
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

/// What the UI shows about the executable this shell will launch.
///
/// `value: None` means **unconfigured**, which is a state and not a failure; a
/// message is only present when something the user named is unusable.
fn dsh_report(config_dir: &Path) -> (Option<serde_json::Value>, Option<String>) {
    match resolve_global_dsh(config_dir) {
        Ok(resolution) => {
            let value = resolution.dsh().map(|dsh| {
                serde_json::json!({
                    "path": dsh.path.display().to_string(),
                    "source": dsh.source.label(),
                })
            });
            (value, None)
        }
        Err(error) => (None, Some(error)),
    }
}

/// One path the user typed, as validated.
#[derive(serde::Serialize)]
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

    // A blank field means "still unconfigured", not "invalid": a saved value is
    // allowed to stand in, so the form can report the state it would leave behind
    // rather than complaining about a field the user cleared.
    let dsh_check = match dsh.map(str::trim) {
        Some("") | None => saved.settings.dsh.as_deref().map(check_dsh),
        Some(value) => Some(check_dsh(value)),
    };
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
    state: tauri::State<'_, Mutex<ShellState>>,
    dsh: String,
    home: String,
) -> Result<ValidationReport, String> {
    let config_dir = lock_state(&state)?.config.config_dir.clone();
    let report = validate_paths(&config_dir, Some(&dsh), Some(&home));
    if !report.ok {
        return Ok(report);
    }
    settings::save(&config_dir, &dsh, &home)?;
    // The write just made these the configured values, so report the state the
    // shell is now in rather than the one it was in before the save.
    Ok(ValidationReport {
        configured: true,
        ..report
    })
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

/// The window label that hosts one profile's GUI.
fn harness_label(profile_id: &str) -> String {
    format!("harness:{profile_id}")
}

/// Focus an existing harness window for this profile, or create it at `url`.
fn open_harness_window(app: &tauri::AppHandle, profile_id: &str, url: &str) -> Result<(), String> {
    let label = harness_label(profile_id);
    if let Some(existing) = app.get_webview_window(&label) {
        let _ = existing.set_focus();
        return Ok(());
    }

    // The guard is attached before the URL loads, so the only navigation the
    // runtime's page can perform stays on its own authority. Comparing scheme,
    // host and explicit port — never `Origin`, which maps an absent port to the
    // scheme default — keeps a profile pointed at a different loopback service
    // reachable through this window.
    let parsed = tauri::Url::parse(url).map_err(|error| format!("invalid harness url {url}: {error}"))?;
    let allowed = (
        parsed.scheme().to_owned(),
        parsed.host_str().unwrap_or_default().to_owned(),
        parsed.port_or_known_default(),
    );

    WebviewWindowBuilder::new(app, &label, WebviewUrl::External(parsed))
        .title(format!("DeepSeek Harness — {profile_id}"))
        .inner_size(1280.0, 860.0)
        .on_navigation(move |target| {
            target.scheme() == allowed.0
                && target.host_str() == Some(allowed.1.as_str())
                && target.port_or_known_default() == allowed.2
        })
        .build()
        .map_err(|error| format!("cannot open the harness window: {error}"))?;
    Ok(())
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
    open_harness_window(&app, &profile_id, &url)
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
    if let Some(window) = app.get_webview_window(&harness_label(&profile_id)) {
        let _ = window.close();
    }
    Ok(())
}

/// Reload the profile list from disk, reporting anything unreadable.
#[tauri::command]
fn reload_profiles(state: tauri::State<'_, Mutex<ShellState>>) -> Result<(), String> {
    let mut state = lock_state(&state)?;
    let home = state.config.home.clone();
    let (profiles, problems) = profile::discover(&home);
    state.profiles = profiles;
    state.problems = problems;
    Ok(())
}

/// Record a startup failure and notify the UI.
fn fail_profile(app: &tauri::AppHandle, message: &str) {
    record_problem(app, message.to_owned());
}

/// Spawn the launcher for `profile_id` and, once ready, open its window.
///
/// Runs on a worker thread: booting dsh takes seconds and must not block the
/// event loop that draws the window.
fn start_profile(app: tauri::AppHandle, profile_id: String) {
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

        let resolved = match resolve_global_dsh(&config.home) {
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
                        if let Err(error) = open_harness_window(&app, &profile_id, &url) {
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
fn start_profile_command(app: tauri::AppHandle, profile_id: String) -> Result<(), String> {
    start_profile(app, profile_id);
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
          scripts: [...document.scripts].map((s) => s.src || '(inline)'),
          error: window.__DSH_PAGE_ERROR__ ?? null,
        };
        window.__TAURI_INTERNALS__.invoke('ui_probe', { marker: JSON.stringify(seen) })
          .catch((error) => { document.title = 'probe failed: ' + error; });
      })()
    "#;
    let injected = window.eval(probe);
    eprintln!("[dsh-app] page-probe injected: {}", injected.is_ok());
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
            if let Some(window) = app.get_webview_window(SHELL_LABEL) {
                let _ = window.set_focus();
            }
        }))
        .manage(Mutex::new(ShellState {
            config,
            profiles,
            children: BTreeMap::new(),
            problems,
        }))
        .invoke_handler(tauri::generate_handler![
            shell_state,
            ui_probe,
            open_harness,
            stop_profile,
            start_profile_command,
            reload_profiles,
            save_settings
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
        let (dsh, problem) = dsh_report(home.path());
        assert!(dsh.is_none());
        assert!(problem.is_none(), "{problem:?}");
    }

    #[test]
    fn a_configured_executable_is_reported_with_its_source() {
        let home = TempHome::new("configured");
        let path = home.fake_dsh("dsh", "0.1.6-alpha.2");
        settings::save(home.path(), &path.display().to_string(), "").expect("the save succeeds");

        let (dsh, problem) = dsh_report(home.path());
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

        let (dsh, problem) = dsh_report(home.path());
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
        let target = home.path().join("harness");
        let report = validate_paths(home.path(), None, Some(&target.display().to_string()));
        assert!(report.ok, "{:?}", report.home.as_ref().and_then(|check| check.error.clone()));
        assert!(target.is_dir(), "an accepted home is created so the child can use it");

        let relative = validate_paths(home.path(), None, Some("relative/home"));
        assert!(!relative.ok);
        assert!(relative.home.expect("a check was made").error.is_some());
    }

    #[test]
    fn a_blank_field_falls_back_to_the_saved_value() {
        let home = TempHome::new("blank-field");
        let path = home.fake_dsh("dsh", "0.1.6-alpha.2");
        settings::save(home.path(), &path.display().to_string(), "").expect("the save succeeds");

        let report = validate_paths(home.path(), Some("   "), None);
        assert!(report.ok);
        assert!(report.configured);
        assert_eq!(report.dsh.expect("a check was made").path, path.display().to_string());
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
}
