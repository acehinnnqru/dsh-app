//! The window that hosts one profile's runtime GUI.
//!
//! A standard macOS window: the system draws the title bar and the traffic
//! lights, AppKit handles dragging, zooming and resizing, and the page inside is
//! the runtime's own — the shell writes nothing into it. That is the whole of the
//! design. The runtime's UI is not restyled, marked or overlaid, and the host
//! chrome this module used to inject (a darwin marker, three drawn lights, a drag
//! band) is gone with it.
//!
//! @see docs/spec-shell-ux.md §2.5

use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

/// The window label for one profile. One window per profile, always.
pub fn label(profile_id: &str) -> String {
    format!("harness:{profile_id}")
}

/// Open one profile's runtime GUI, or focus the window already showing it.
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

    WebviewWindowBuilder::new(app, &label, WebviewUrl::External(parsed))
        .title(format!("DeepSeek Harness — {profile_id}"))
        .inner_size(1280.0, 860.0)
        .center()
        .on_navigation(move |target| {
            target.scheme() == allowed.0
                && target.host_str() == Some(allowed.1.as_str())
                && target.port_or_known_default() == allowed.2
        })
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
