/** Copied verbatim from the harness `packages/boot/app-boot/src/profile.ts`. */

/// The user patch layer: a top-level YAML array of loader patch entries.
pub const PROFILE_PATCH_TEMPLATE: &str = "# Your patch layer for this dsh profile, applied after every bundle layer:\n\
# a top-level YAML array of loader patch entries (id-targeted config\n\
# overrides, disables, and insert lists; `!!js` expressions allowed).\n\
[]\n";

/// pnpm settings for out-of-tree plugins.
///
/// `autoInstallPeers: false` is deliberate upstream: a profile's missing peers
/// fall through to the healed `$DSH_HOME/profiles/node_modules` installation
/// fallback, so every plugin shares the installation's single cordis instance.
/// The app-side runtime closure must NOT repeat this setting.
pub const PROFILE_PNPM_WORKSPACE: &str = "packages:\n  - .\n\nnodeLinker: hoisted\nautoInstallPeers: false\n";

/// Bundles a default dsh-app web preset boots.
pub const DEFAULT_WEB_BUNDLES: [&str; 2] = ["@deepseek-ai/dsh-base", "@deepseek-ai/dsh-web-app"];

/// The dsh version range every published release declares.
pub const NODE_REQUIREMENT: &str = "^22.19.0 || >=24.0.0";
