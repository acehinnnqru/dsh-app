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

/// The bundle that gives a profile a served web GUI, and therefore the only
/// carrier this shell implements.
pub const WEB_BUNDLE: &str = "@deepseek-ai/dsh-web-app";
/// The one-shot task carrier.
pub const HEADLESS_BUNDLE: &str = "@deepseek-ai/dsh-headless";
/// The stdio carriers (upstream `acp` and `sdk` templates).
pub const ACP_BUNDLE: &str = "@deepseek-ai/dsh-acp-app";
pub const SDK_BUNDLE: &str = "@deepseek-ai/dsh-sdk-app";
pub const SDK_MINIMAL_BUNDLE: &str = "@deepseek-ai/dsh-sdk-minimal";

/// The carrier a bundle stack implies, for a profile the shell did not create.
///
/// A preset states its `kind` itself, but a profile discovered under
/// `profiles/` does not: its bundles are the only statement of what it can be
/// launched as. Web wins when several carriers are present, because the shell
/// can only drive that one (design doc D3.3).
pub fn kind_for_bundles(bundles: &[String]) -> &'static str {
    let has = |wanted: &str| bundles.iter().any(|bundle| bundle == wanted);
    if has(WEB_BUNDLE) {
        "web"
    } else if has(HEADLESS_BUNDLE) {
        "task"
    } else if has(ACP_BUNDLE) || has(SDK_BUNDLE) || has(SDK_MINIMAL_BUNDLE) {
        "server"
    } else {
        "none"
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn bundles(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    /// A discovered profile has no `kind` of its own, so its bundles are the
    /// only statement of what the launcher can do with it (design doc D3.3).
    #[test]
    fn a_discovered_profile_is_classified_by_its_bundles() {
        assert_eq!(kind_for_bundles(&bundles(&["@deepseek-ai/dsh-base", WEB_BUNDLE])), "web");
        assert_eq!(kind_for_bundles(&bundles(&["@deepseek-ai/dsh-base", HEADLESS_BUNDLE])), "task");
        assert_eq!(kind_for_bundles(&bundles(&["@deepseek-ai/dsh-base", ACP_BUNDLE])), "server");
        assert_eq!(kind_for_bundles(&bundles(&[SDK_BUNDLE])), "server");
        assert_eq!(kind_for_bundles(&bundles(&[SDK_MINIMAL_BUNDLE])), "server");
        // The one carrier this shell can drive wins over the others.
        assert_eq!(kind_for_bundles(&bundles(&[HEADLESS_BUNDLE, WEB_BUNDLE])), "web");
        // A bare installation layer is not a carrier at all.
        assert_eq!(kind_for_bundles(&bundles(&["@deepseek-ai/dsh-base"])), "none");
        assert_eq!(kind_for_bundles(&[]), "none");
    }
}

