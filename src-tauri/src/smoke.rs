//! The activation smoke test.
//!
//! The shell no longer moves in lock-step with the runtime (design doc D6/D10),
//! so nothing else proves that a given runtime can actually be launched by this
//! shell. This is that proof: boot the candidate, wait for its readiness line,
//! and stop it — before it is allowed to become the active runtime.
//!
//! A candidate that fails here is discarded and the installation is left alone.

use std::path::{Path, PathBuf};

use crate::preset::Preset;
use crate::supervisor::{parse_ready_line, Phase, SupervisedChild};

/// How long the candidate may take to announce readiness.
const SMOKE_TIMEOUT_SECS: u64 = 90;

/// The outcome of one smoke test.
#[derive(Debug, Clone)]
pub struct SmokeReport {
    /// Whether the candidate reached readiness and stopped cleanly.
    pub ok: bool,
    /// The URL it announced, with the launch token removed.
    pub url: Option<String>,
    /// Diagnostic tail when it failed.
    pub detail: Option<String>,
}

/// Boot `runtime_dir` with its own profile and report whether it comes up.
///
/// The profile is a scratch one under `home`, never a caller's: a smoke test
/// must not be able to disturb a profile the user is running.
pub fn smoke(
    runtime_dir: &Path,
    node: &Path,
    home: &Path,
    node_args: Vec<String>,
) -> SmokeReport {
    let manifest = match crate::config::RuntimeManifest::load(runtime_dir) {
        Ok(manifest) => manifest,
        Err(error) => {
            return SmokeReport {
                ok: false,
                url: None,
                detail: Some(error),
            }
        }
    };

    let preset = Preset {
        id: "smoke".to_owned(),
        label: Some("Runtime smoke test".to_owned()),
        kind: "web".to_owned(),
        profile: Some("dsh-app-smoke".to_owned()),
        bundles: None,
        // `startup` on purpose: internals access is a property of the profile,
        // not of the runtime, and this test must not require it.
        patch_reload: Some("startup".to_owned()),
        env: Default::default(),
    };

    if let Err(error) = crate::profile::materialize(home, &preset) {
        return SmokeReport {
            ok: false,
            url: None,
            detail: Some(error),
        };
    }

    let child = SupervisedChild::start(
        preset,
        node.to_path_buf(),
        manifest.entry_path(runtime_dir),
        runtime_dir.to_path_buf(),
        home.to_path_buf(),
        Vec::new(),
        node_args,
    );

    let mut child = match child {
        Ok(child) => child,
        Err(error) => {
            return SmokeReport {
                ok: false,
                url: None,
                detail: Some(error),
            }
        }
    };

    let snapshot = child.snapshot();
    let ready = matches!(snapshot.phase, Phase::Ready { .. });
    let url = match &snapshot.phase {
        Phase::Ready { url } => Some(redact(url)),
        _ => None,
    };
    // Diagnostics matter most on the failing path, so read them before shutdown
    // clears nothing but does stop the process producing them.
    let detail = if ready {
        None
    } else {
        let tail = snapshot.stderr_tail;
        Some(if tail.is_empty() {
            format!("timed out after {SMOKE_TIMEOUT_SECS}s without a readiness line")
        } else {
            tail.join("\n")
        })
    };
    child.shutdown();

    SmokeReport { ok: ready, url, detail }
}

/// Strip the launch token: it is a per-process credential (see the harness's
/// browser-token note), so it does not belong in a stored report.
fn redact(url: &str) -> String {
    match url.split_once("token=") {
        Some((head, tail)) => {
            let remainder = tail.split_once('&').map(|(_, rest)| rest).unwrap_or("");
            if remainder.is_empty() {
                format!("{head}token=<redacted>")
            } else {
                format!("{head}token=<redacted>&{remainder}")
            }
        }
        None => url.to_owned(),
    }
}

/// Whether `text` looks like a readiness line, used by callers that stream.
pub fn announces_readiness(text: &str) -> bool {
    parse_ready_line(text).is_some()
}

/// The scratch home a smoke test runs against, under the caller's home.
pub fn smoke_home(home: &Path) -> PathBuf {
    home.join("dsh-app").join("smoke")
}
