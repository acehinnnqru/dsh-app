//! Runtime capability probing for a candidate Node executable.
//!
//! A version string is not enough to accept a Node. The harness needs
//! `require-builtin` access for `patchReload: live` profiles, and
//! `node-addon-require-builtin` obtains it by matching the running binary's
//! machine code — so a distribution-repackaged Node can load the addon and
//! still refuse every id. That is why the capability is exercised here rather
//! than inferred, and why the shell knows whether it must fall back to
//! `--expose-internals`.
//!
//! @see docs/design-tauri-wrapper.md D2, §11.3

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::SystemTime;

/// What one Node executable can actually do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeCapability {
    /// `process.versions.node`.
    pub version: String,
    /// Whether the Node build is one the harness is exercised against.
    pub supported: bool,
    /// `node-addon-require-builtin` returned the internal loader.
    pub require_builtin: bool,
    /// Why `require_builtin` is false, for the UI and logs.
    pub detail: Option<String>,
}

impl NodeCapability {
    /// Whether this Node can serve a profile that mounts the HMR row.
    pub fn internals_available(&self) -> bool {
        self.require_builtin
    }
}

/// Probe results, keyed by executable path together with the closure it was
/// probed against, plus that executable's modified time.
#[derive(Default)]
pub struct NodeProbeCache {
    entries: Mutex<HashMap<(PathBuf, PathBuf), (SystemTime, NodeCapability)>>,
}

impl NodeProbeCache {
    /// Probe `node` against `closure`, reusing a cached verdict while neither
    /// the binary nor the closure path changed.
    pub fn probe(&self, node: &Path, closure: &Path) -> NodeCapability {
        let key = (node.to_path_buf(), closure.to_path_buf());
        let modified = std::fs::metadata(node).and_then(|meta| meta.modified()).ok();
        if let (Some(modified), Ok(entries)) = (modified, self.entries.lock()) {
            if let Some((cached_at, capability)) = entries.get(&key) {
                if *cached_at == modified {
                    return capability.clone();
                }
            }
        }
        let capability = probe_uncached(node, closure);
        if let (Some(modified), Ok(mut entries)) = (modified, self.entries.lock()) {
            entries.insert(key, (modified, capability.clone()));
        }
        capability
    }
}

/// Run the probe in a child process so an addon crash cannot take the shell down.
///
/// The addon is resolved through the closure's own `node_modules`, because that
/// is where the launcher will resolve it from — probing against anything else
/// would answer a different question.
fn probe_uncached(node: &Path, closure: &Path) -> NodeCapability {
    let script = r#"
      const { createRequire } = require('node:module')
      const { join } = require('node:path')
      const closure = process.argv[1]
      const requireFromClosure = createRequire(join(closure, 'node_modules', '@deepseek-ai', 'dsh', 'lib', 'bin.js'))
      const report = { version: process.versions.node, requireBuiltin: false, detail: null }
      try {
        const addon = requireFromClosure('node-addon-require-builtin')
        addon.requireBuiltin('internal/modules/esm/loader')
        report.requireBuiltin = true
      } catch (error) {
        report.detail = String(error && error.message ? error.message : error)
      }
      process.stdout.write(JSON.stringify(report))
    "#;
    match Command::new(node).args(["-e", script, &closure.display().to_string()]).output() {
        Ok(output) if output.status.success() => {
            let text = String::from_utf8_lossy(&output.stdout);
            match serde_json::from_str::<ProbeReport>(&text) {
                Ok(report) => NodeCapability {
                    version: report.version,
                    supported: report.require_builtin,
                    require_builtin: report.require_builtin,
                    detail: report.detail,
                },
                Err(error) => NodeCapability {
                    version: "unknown".to_owned(),
                    supported: false,
                    require_builtin: false,
                    detail: Some(format!("could not read the probe report: {error}")),
                },
            }
        }
        Ok(output) => NodeCapability {
            version: "unknown".to_owned(),
            supported: false,
            require_builtin: false,
            detail: Some(
                String::from_utf8_lossy(&output.stderr)
                    .trim()
                    .lines()
                    .last()
                    .unwrap_or("the probe process failed")
                    .to_owned(),
            ),
        },
        Err(error) => NodeCapability {
            version: "unknown".to_owned(),
            supported: false,
            require_builtin: false,
            detail: Some(format!("cannot execute {}: {error}", node.display())),
        },
    }
}

#[derive(serde::Deserialize)]
struct ProbeReport {
    version: String,
    #[serde(rename = "requireBuiltin")]
    require_builtin: bool,
    detail: Option<String>,
}
