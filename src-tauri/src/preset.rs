//! The preset type: the ordered bundle stack plus the invocation facts it implies.
//!
//! A preset is a thin wrapper. Every field maps onto an existing dsh concept —
//! `profile` → `--profile`, `patches` → `--patch`, `env` → the child
//! environment — so dsh-app never invents a second configuration language.
//!
//! @see docs/design-tauri-wrapper.md D3
//!

use std::collections::BTreeMap;

use serde::Serialize;

/// One preset: the ordered bundle stack plus the invocation facts it implies.
///
/// Not `Deserialize`: the preset *store* owns file parsing, so this type is only
/// constructed in Rust and serialized for the UI.
#[derive(Debug, Clone, Serialize)]
pub struct Preset {
    /// Stable id, also the default profile name.
    pub id: String,
    /// Human-facing label.
    pub label: Option<String>,
    /// `web` is the only kind this milestone implements.
    pub kind: String,
    /// Profile directory name under `<home>/profiles`.
    pub profile: Option<String>,
    /// Ordered `dsh.profile.bundles`; materialized on first boot only.
    pub bundles: Option<Vec<String>>,
    /// `startup` (default) or `live`; `live` makes internals capability required.
    pub patch_reload: Option<String>,
    /// Whitelisted environment additions.
    pub env: BTreeMap<String, String>,
    /// Node executable this preset uses, when the user has chosen one.
    ///
    /// Deliberately explicit rather than auto-detected. A macOS app started from
    /// Finder does not inherit the login shell's `PATH` (it gets
    /// `/usr/bin:/bin:/usr/sbin:/sbin`), so "find node on PATH" would silently
    /// pick whatever that minimal environment happens to contain — which may be
    /// absent, the wrong version, or a build whose `require-builtin` support is
    /// missing. Naming the executable is the only reliable answer.
    pub node: Option<String>,
}

impl Preset {
    /// The profile name this preset owns.
    pub fn profile_name(&self) -> &str {
        self.profile.as_deref().unwrap_or(&self.id)
    }

    /// The bundle stack to write when the profile is first materialized.
    pub fn bundle_list(&self) -> Vec<String> {
        self.bundles
            .clone()
            .unwrap_or_else(|| crate::profile_template::DEFAULT_WEB_BUNDLES.iter().map(|s| (*s).to_owned()).collect())
    }

    /// Effective patch-reload lifecycle.
    pub fn patch_reload_value(&self) -> &str {
        self.patch_reload.as_deref().unwrap_or("startup")
    }

    /// Whether this preset's profile mounts the HMR row and therefore needs
    /// internals access (`--expose-internals` or the native addon).
    pub fn needs_internals(&self) -> bool {
        self.patch_reload_value() == "live"
    }
}

