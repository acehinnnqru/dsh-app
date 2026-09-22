//! The window that hosts one profile's runtime GUI.
//!
//! A borderless window: no title bar, the system's traffic lights floating over
//! the page, and the window dragged from the band the runtime's own macOS layout
//! reserves for exactly that. The page inside belongs to the runtime; the shell
//! gives it host chrome and nothing else — one documented marker attribute and a
//! drag listener, both in `ui/harness-host.js`.
//!
//! @see docs/spec-shell-ux.md §2.5

use tauri::utils::config::WindowEffectsConfig;
use tauri::window::{Effect, EffectState, EffectsBuilder};
use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

/// The script that gives the runtime's page its host chrome.
///
/// Embedded rather than read at run time, so the shell's identity hash covers it
/// like every other page file (see `build.rs`), and so a window can never come up
/// with chrome older than the binary.
const HOST_SCRIPT: &str = include_str!("../../ui/harness-host.js");

/// The window label for one profile. One window per profile, always.
pub fn label(profile_id: &str) -> String {
    format!("harness:{profile_id}")
}

/// The material both windows use: the one macOS sidebars use, following window
/// focus so the glass and the traffic lights agree, with the radius the page's
/// corners have.
pub fn vibrancy() -> WindowEffectsConfig {
    EffectsBuilder::new()
        .effect(Effect::Sidebar)
        .state(EffectState::FollowsWindowActiveState)
        .radius(12.0)
        .build()
}

/// Open one profile's runtime GUI, or focus the window already showing it.
///
/// The window stays `Titled` with an invisible title bar, the same arrangement as
/// the shell window: AppKit keeps the rounded corners, the shadow, edge resizing
/// and ⌘W, while the page runs to the top edge. The traffic lights are the
/// system's, and the runtime's desktop layout reserves the band they sit in.
pub fn open(app: &tauri::AppHandle, profile_id: &str, url: &str) -> Result<(), String> {
    let label = label(profile_id);
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

    let builder = WebviewWindowBuilder::new(app, &label, WebviewUrl::External(parsed))
        .title(format!("DeepSeek Harness — {profile_id}"))
        .inner_size(1280.0, 860.0)
        .center()
        .transparent(true)
        .effects(vibrancy())
        // The whole of X1: the darwin marker, and the dragging that band needs.
        .initialization_script(HOST_SCRIPT)
        .on_navigation(move |target| {
            target.scheme() == allowed.0
                && target.host_str() == Some(allowed.1.as_str())
                && target.port_or_known_default() == allowed.2
        });

    #[cfg(target_os = "macos")]
    let builder = builder
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true);

    builder
        .build()
        .map_err(|error| format!("cannot open the harness window: {error}"))?;

    // Logged like `starting profile X` next to it: which windows exist is the
    // first question when a runtime comes up and nothing appears.
    eprintln!("[dsh-app] harness window {label} opened");
    Ok(())
}

/// Close one profile's window, if it is open.
pub fn close(app: &tauri::AppHandle, profile_id: &str) {
    if let Some(window) = app.get_webview_window(&label(profile_id)) {
        let _ = window.close();
    }
}
