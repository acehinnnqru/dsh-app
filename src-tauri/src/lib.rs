//! dsh-app: a thin Tauri shell around supervised DeepSeek Harness runtimes.
//!
//! The shell owns three things and nothing else: materializing a preset's
//! profile, spawning the launcher and reading its readiness line, and hosting
//! the served GUI in a window. It never imports harness internals and never
//! interprets the client protocol — see `docs/design-tauri-wrapper.md` D1/D6.
//!
//! Several presets may run at once. That is safe because each preset owns a
//! distinct profile and nothing else is shared; the invariants that keep it safe
//! are documented in the design doc D3.4 and enforced here.

mod config;
mod node_probe;
mod preset;
mod preset_store;
mod profile;
mod profile_template;
mod runtime_store;
mod smoke;
mod supervisor;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

use tauri::{Emitter, Manager, RunEvent, WebviewUrl, WebviewWindowBuilder};

use config::AppConfig;
use node_probe::NodeProbeCache;
use preset_store::ResolvedPreset;
use runtime_store::RuntimeStore;
use supervisor::{Phase, SupervisedChild};

/// The shell's own page, compiled into the binary from `ui/`.
const SHELL_LABEL: &str = "shell";

/// Identity of the embedded shell page, stamped by `build.rs`. Surfaced in the
/// UI so a running window can be matched to a source revision.
const UI_VERSION: &str = env!("DSH_APP_UI_VERSION");

/// Application state.
struct ShellState {
    config: AppConfig,
    /// Presets in load order; the first is the default.
    presets: Vec<ResolvedPreset>,
    /// Live children, keyed by preset id.
    children: BTreeMap<String, SupervisedChild>,
    /// Problems found while loading state, surfaced in the UI.
    problems: Vec<String>,
    /// The runtime store this installation serves from.
    store: RuntimeStore,
    /// Capability verdicts for candidate Node executables.
    node_probe: NodeProbeCache,
    /// Whether the launcher had to pass `--expose-internals` because the
    /// selected Node cannot use the native addon.
    expose_internals: bool,
}

impl ShellState {
    /// The preset with this id.
    fn preset(&self, id: &str) -> Option<&ResolvedPreset> {
        self.presets.iter().find(|entry| entry.preset.id == id)
    }

    /// Identity of the runtime every preset is launched with.
    ///
    /// A single resolved runtime for the whole shell is what keeps the harness's
    /// shared `profiles/node_modules` fallback consistent (design doc D3.4 I1):
    /// two different installs would rewrite that fallback under each other.
    fn runtime_id(&self) -> String {
        self.config
            .runtime
            .dir
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.config.runtime.dir.display().to_string())
    }

    /// One preset row for the UI.
    fn preset_row(&self, entry: &ResolvedPreset) -> serde_json::Value {
        let id = &entry.preset.id;
        let (phase, url, stderr_tail, patch_files) = match self.children.get(id) {
            Some(child) => {
                let snapshot = child.snapshot();
                let url = match &snapshot.phase {
                    Phase::Ready { url } => Some(url.clone()),
                    _ => None,
                };
                (
                    snapshot.phase.label().to_owned(),
                    url,
                    snapshot.stderr_tail,
                    snapshot
                        .patch_files
                        .iter()
                        .map(|path| path.display().to_string())
                        .collect(),
                )
            }
            None => ("idle".to_owned(), None, Vec::new(), Vec::new()),
        };
        serde_json::json!({
            "id": id,
            "label": entry.preset.label.clone().unwrap_or_else(|| id.clone()),
            "kind": entry.preset.kind,
            "profile": entry.preset.profile_name(),
            "bundles": entry.preset.bundle_list(),
            "needsInternals": entry.preset.needs_internals(),
            "patchReload": entry.preset.patch_reload_value(),
            "patchFiles": patch_files,
            "source": entry.source.display().to_string(),
            "runtimeId": self.runtime_id(),
            "needsInternals": entry.preset.needs_internals(),
            "internalsViaFlag": matches!(phase.as_str(), "ready" | "starting")
                && entry.preset.needs_internals()
                && self.expose_internals,
            "phase": phase,
            "url": url,
            "stderrTail": stderr_tail,
        })
    }

    /// Everything the UI renders.
    fn snapshot(&self) -> serde_json::Value {
        serde_json::json!({
            "uiVersion": UI_VERSION,
            "runtime": self.store.status(),
            "home": self.config.home.display().to_string(),
            "runtimeDir": self.config.runtime.dir.display().to_string(),
            "node": self.config.runtime.node.display().to_string(),
            "presets": self.presets.iter().map(|entry| self.preset_row(entry)).collect::<Vec<_>>(),
            "problems": self.problems,
        })
    }
}

/// Roll back to the previous runtime.
///
/// Refuses while any preset is running: the running children hold the old
/// runtime open, and D3.4's ordering rule is that a version change happens with
/// everything stopped.
#[tauri::command]
fn rollback_runtime(state: tauri::State<'_, Mutex<ShellState>>) -> Result<String, String> {
    let state = lock_state(&state)?;
    if !state.children.is_empty() {
        return Err("stop every running preset before changing the runtime version".to_owned());
    }
    let id = state.store.rollback()?;
    Ok(id)
}

/// Sample presets staged by `build.rs`, seeded into the home on first run.
fn preset_seed_dir() -> Option<PathBuf> {
    let staged = PathBuf::from(env!("OUT_DIR")).join("seed-presets");
    staged.is_dir().then_some(staged)
}

/// Lock the shell state, converting a poisoned mutex into a readable error.
fn lock_state<'a>(state: &'a tauri::State<'_, Mutex<ShellState>>) -> Result<MutexGuard<'a, ShellState>, String> {
    state
        .lock()
        .map_err(|error| format!("shell state is unavailable: {error}"))
}

/// The window label that hosts one preset's GUI.
fn harness_label(preset_id: &str) -> String {
    format!("harness:{preset_id}")
}

/// Focus an existing harness window for this preset, or create it at `url`.
fn open_harness_window(app: &tauri::AppHandle, preset_id: &str, url: &str) -> Result<(), String> {
    let label = harness_label(preset_id);
    if let Some(existing) = app.get_webview_window(&label) {
        let _ = existing.set_focus();
        return Ok(());
    }

    // The guard is attached before the URL loads, so the only navigation the
    // runtime's page can perform stays on its own authority. Comparing scheme,
    // host and explicit port — never `Origin`, which maps an absent port to the
    // scheme default — keeps a preset pointed at a different loopback service
    // reachable through this window.
    let parsed = tauri::Url::parse(url).map_err(|error| format!("invalid harness url {url}: {error}"))?;
    let allowed = (
        parsed.scheme().to_owned(),
        parsed.host_str().unwrap_or_default().to_owned(),
        parsed.port_or_known_default(),
    );

    WebviewWindowBuilder::new(app, &label, WebviewUrl::External(parsed))
        .title(format!("DeepSeek Harness — {preset_id}"))
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
/// window we can inspect from a terminal, so this is how the page's input is
/// made observable when a field renders unexpectedly.
#[tauri::command]
fn shell_state(state: tauri::State<'_, Mutex<ShellState>>) -> serde_json::Value {
    let payload = match state.lock() {
        Ok(state) => state.snapshot(),
        Err(error) => serde_json::json!({ "problems": [error.to_string()], "presets": [] }),
    };
    if std::env::var("DSH_APP_DEBUG_STATE").is_ok_and(|value| value == "1") {
        eprintln!("[dsh-app] shell_state {}", serde_json::to_string(&payload).unwrap_or_default());
    }
    payload
}

/// Open one preset's served GUI (or focus it when already open).
#[tauri::command]
fn open_harness(
    app: tauri::AppHandle,
    preset_id: String,
    state: tauri::State<'_, Mutex<ShellState>>,
) -> Result<(), String> {
    let url = {
        let state = lock_state(&state)?;
        state
            .children
            .get(&preset_id)
            .and_then(|child| match child.snapshot().phase {
                Phase::Ready { url } => Some(url),
                _ => None,
            })
            .ok_or_else(|| format!("preset {preset_id} is not ready"))?
    };
    open_harness_window(&app, &preset_id, &url)
}

/// Stop one preset's runtime and close its window.
#[tauri::command]
fn stop_preset(
    app: tauri::AppHandle,
    preset_id: String,
    state: tauri::State<'_, Mutex<ShellState>>,
) -> Result<(), String> {
    {
        let mut state = lock_state(&state)?;
        if let Some(mut child) = state.children.remove(&preset_id) {
            child.shutdown();
        }
    }
    if let Some(window) = app.get_webview_window(&harness_label(&preset_id)) {
        let _ = window.close();
    }
    Ok(())
}

/// Reload presets from disk, reporting any file that could not be parsed.
#[tauri::command]
fn reload_presets(state: tauri::State<'_, Mutex<ShellState>>) -> Result<(), String> {
    let mut state = lock_state(&state)?;
    let home = state.config.home.clone();
    let (presets, problems) = preset_store::load_all(&home, preset_seed_dir().as_deref());
    state.presets = presets;
    state.problems = problems;
    Ok(())
}

/// Record a startup failure and notify the UI.
fn fail_preset(app: &tauri::AppHandle, message: &str) {
    if let Ok(mut state) = app.state::<Mutex<ShellState>>().lock() {
        state.problems.push(message.to_owned());
    }
    let _ = app.emit(
        "dsh://status",
        serde_json::json!({ "phase": "failed", "error": message }),
    );
}

/// Spawn the launcher for `preset_id` and, once ready, open its window.
///
/// Runs on a worker thread: booting dsh takes seconds and must not block the
/// event loop that draws the window.
fn start_preset(app: tauri::AppHandle, preset_id: String) {
    std::thread::spawn(move || {
        let handle = app.state::<Mutex<ShellState>>();
        let (config, preset, patch_files, node_args, capability) = {
            let Ok(state) = handle.lock() else {
                eprintln!("[dsh-app] shell state is unavailable");
                return;
            };
            if state.children.contains_key(&preset_id) {
                return;
            }
            let Some(resolved) = state.preset(&preset_id) else {
                eprintln!("[dsh-app] no preset with id {preset_id}");
                return;
            };

            // Ask the selected Node what it can do before deciding how to launch
            // it: a `live` profile mounts the HMR row, which needs internals
            // access, and only an official Node build provides it through the
            // native addon.
            let capability = state.node_probe.probe(&state.config.runtime.node, &state.config.runtime.dir);
            let mut node_args: Vec<String> = Vec::new();
            if resolved.preset.needs_internals() {
                if capability.internals_available() {
                    // The addon works; nothing extra is needed.
                } else if capability.version == "unknown" {
                    eprintln!(
                        "[dsh-app] cannot probe {}: {}",
                        state.config.runtime.node.display(),
                        capability.detail.as_deref().unwrap_or("unknown reason")
                    );
                } else {
                    // Equivalent capability, wider privilege surface, so it is
                    // used only for the profile that actually requires it.
                    node_args.push("--expose-internals".to_owned());
                }
            }
            (
                state.config.clone(),
                resolved.preset.clone(),
                resolved.patch_files.clone(),
                node_args,
                capability,
            )
        };
        if let Ok(mut state) = handle.lock() {
            state.expose_internals = node_args.iter().any(|arg| arg == "--expose-internals");
        }
        if preset.needs_internals() && !capability.internals_available() && capability.version == "unknown" {
            let message = format!(
                "preset {preset_id} needs internals access but {} could not be probed: {}",
                config.runtime.node.display(),
                capability.detail.unwrap_or_else(|| "unknown reason".to_owned())
            );
            fail_preset(&app, &message);
            return;
        }

        let _ = app.emit("dsh://status", serde_json::json!({ "presetId": preset_id }));
        if let Err(error) = profile::materialize(&config.home, &preset) {
            eprintln!("[dsh-app] {error}");
            fail_preset(&app, &error);
            return;
        }

        let manifest = match config::RuntimeManifest::load(&config.runtime.dir) {
            Ok(manifest) => manifest,
            Err(error) => {
                eprintln!("[dsh-app] {error}");
                fail_preset(&app, &error);
                return;
            }
        };

        let child = SupervisedChild::start(
            preset,
            config.runtime.node.clone(),
            manifest.entry_path(&config.runtime.dir),
            config.runtime.dir.clone(),
            config.home.clone(),
            patch_files,
            node_args,
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
                    state.children.insert(preset_id.clone(), child);
                }
                match url {
                    Some(url) => {
                        let _ = app.emit(
                            "dsh://status",
                            serde_json::json!({ "presetId": preset_id, "phase": "ready", "url": url }),
                        );
                        if let Err(error) = open_harness_window(&app, &preset_id, &url) {
                            eprintln!("[dsh-app] {error}");
                        }
                    }
                    None => {
                        // Boot failed: hand the UI the child's own diagnostics so
                        // the failure explains itself instead of only hitting a log.
                        let _ = app.emit(
                            "dsh://status",
                            serde_json::json!({ "presetId": preset_id, "phase": "failed", "stderrTail": tail }),
                        );
                    }
                }
            }
            Err(error) => {
                eprintln!("[dsh-app] {error}");
                fail_preset(&app, &error);
            }
        }
    });
}

/// Start one preset from the UI.
#[tauri::command]
fn start_preset_command(app: tauri::AppHandle, preset_id: String) -> Result<(), String> {
    start_preset(app, preset_id);
    Ok(())
}

/// Report what the shell page actually sees, on stdout.
///
/// `DSH_APP_DEBUG_STATE=1` makes the shell evaluate a probe in its own window
/// after load and print the answer. The shell has no terminal-visible surface,
/// so without a channel like this a page that fails to run is indistinguishable
/// from one that never loaded — which is exactly how a broken bridge once hid
/// behind a screen of placeholder dashes.
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
    // `eval` is host-injected, so unlike an inline attribute it is not subject
    // to the page's CSP.
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

/// Act on flags that must work before (or without) a window.
///
/// `--rollback` is a recovery path, not a convenience: if a bad runtime keeps
/// the shell from coming up usefully, this is how the user gets back to the
/// previous one from a terminal. It returns the process exit code.
fn handle_startup_flags(store: &RuntimeStore) -> Option<i32> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if !args.iter().any(|arg| arg == "--rollback") {
        return None;
    }
    match store.rollback() {
        Ok(id) => {
            println!("dsh-app: rolled back to {id}");
            Some(0)
        }
        Err(error) => {
            eprintln!("dsh-app: rollback failed: {error}");
            Some(1)
        }
    }
}

/// Build and run the shell.
pub fn run() {
    let mut problems = Vec::new();
    let root = config::repo_root();
    let home = config::resolve_home(&root);
    let store = RuntimeStore::new(&home);

    // First run installs the seed the bundle carries, so a packaged app boots
    // offline and a checkout needs only `pnpm runtime:prepare` (design doc U3).
    if let Some(seed) = config::seed_runtime_dir() {
        if let Some(id) = seed.file_name().map(|name| name.to_string_lossy().into_owned()) {
            match store.install(&id, &seed) {
                Ok(installed) => {
                    if installed {
                        eprintln!("[dsh-app] installed the seed runtime {id}");
                    }
                    // Activate only when nothing is active: an update the user
                    // already applied must not be reverted by a stale seed.
                    if store.active_dir().is_none() {
                        // Mandatory gate (design doc D8.2 U1): nothing else proves
                        // this runtime can be launched by this shell, so a seed
                        // that cannot boot must not become the active runtime.
                        let dir = store.root().join(&id);
                        let node = config::resolve_node_for(&root, &dir);
                        match node {
                            Ok(node) => {
                                let report = smoke::smoke(&dir, &node, &smoke::smoke_home(&home), Vec::new());
                                if report.ok {
                                    eprintln!(
                                        "[dsh-app] seed runtime {id} passed its smoke test ({})",
                                        report.url.as_deref().unwrap_or("no url")
                                    );
                                    if let Err(error) = store.activate(&id) {
                                        problems.push(error);
                                    }
                                } else {
                                    problems.push(format!(
                                        "the seed runtime {id} failed its smoke test and was not activated: {}",
                                        report.detail.unwrap_or_else(|| "unknown reason".to_owned())
                                    ));
                                }
                            }
                            Err(error) => problems.push(error),
                        }
                    }
                    if let Err(error) = store.record_seed(&id) {
                        problems.push(error);
                    }
                }
                Err(error) => problems.push(error),
            }
        }
    }

    // Before anything opens a window: a recovery flag must not depend on the
    // shell being able to start.
    if let Some(code) = handle_startup_flags(&store) {
        std::process::exit(code);
    }

    // Which runtime and which Node this process resolved. Recorded because a
    // packaged app's Home is not the repository, so "which Node is it using" is
    // the first question whenever a bundled variant misbehaves.
    if let Ok(config) = config::load(&home, &store) {
        eprintln!(
            "[dsh-app] runtime {} via node {}",
            config.runtime.dir.display(),
            config.runtime.node.display()
        );
    }

    let config = match config::load(&home, &store) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("[dsh-app] {error}");
            problems.push(error);
            AppConfig {
                home: home.clone(),
                runtime: config::RuntimeConfig {
                    dir: PathBuf::from("."),
                    node: PathBuf::from("node"),
                },
            }
        }
    };

    let (presets, preset_problems) = preset_store::load_all(&config.home, preset_seed_dir().as_deref());
    problems.extend(preset_problems);
    let autostart = presets.first().map(|entry| entry.preset.id.clone());
    let can_autostart = !presets.is_empty();

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Some(window) = app.get_webview_window(SHELL_LABEL) {
                let _ = window.set_focus();
            }
        }))
        .manage(Mutex::new(ShellState {
            config,
            presets,
            children: BTreeMap::new(),
            problems,
            store,
            node_probe: NodeProbeCache::default(),
            expose_internals: false,
        }))
        .invoke_handler(tauri::generate_handler![
            shell_state,
            ui_probe,
            open_harness,
            stop_preset,
            start_preset_command,
            reload_presets,
            rollback_runtime
        ])
        .setup(move |app| {
            let probe_handle = app.handle().clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(6));
                install_page_probe(&probe_handle);
            });
            if can_autostart {
                if let Some(preset_id) = autostart.clone() {
                    start_preset(app.handle().clone(), preset_id);
                }
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("failed to build the dsh-app shell");

    app.run(|app_handle, event| {
        if let RunEvent::ExitRequested { .. } | RunEvent::Exit = event {
            // Stop every supervised child concurrently before the process
            // leaves: none may outlive its window. If the shell is SIGKILLed
            // instead, the child's stdin EOF is the remaining guard.
            // The children are taken out under the lock and joined after it is
            // released, so a slow SIGTERM grace period never blocks another
            // event handler that needs the same state.
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
