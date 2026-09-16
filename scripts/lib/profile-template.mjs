/**
 * The profile files dsh-app materializes on a preset's first boot.
 *
 * These mirror `packages/boot/app-boot/src/profile.ts` in the harness
 * (`PROFILE_PATCH_TEMPLATE` / `PROFILE_PNPM_WORKSPACE` / `initProfile`). dsh-app
 * writes them itself instead of invoking `dsh plugin`, because that command
 * forwards to PATH `pnpm` and would make first boot depend on a package
 * manager being resolvable.
 *
 * Keeping the shapes in one module means the M1 probe, the Rust
 * implementation, and any future migration read the same definition.
 *
 * @see docs/design-tauri-wrapper.md D3
 */

/** The user patch layer: a top-level YAML array of loader patch entries. */
export const PROFILE_PATCH_TEMPLATE = `# Your patch layer for this dsh profile, applied after every bundle layer:
# a top-level YAML array of loader patch entries (id-targeted config
# overrides, disables, and insert lists; \`!!js\` expressions allowed).
[]
`

/**
 * pnpm settings for out-of-tree plugins.
 *
 * `autoInstallPeers: false` is deliberate upstream, and the reason matters
 * here: a profile's missing peers fall through to the healed
 * `$DSH_HOME/profiles/node_modules` installation fallback, so every plugin
 * shares the installation's single cordis instance. The closure on the app
 * side therefore must NOT repeat that setting — it installs peers for real.
 */
export const PROFILE_PNPM_WORKSPACE = `packages:
  - .

nodeLinker: hoisted
autoInstallPeers: false
`

/** The bundles a default dsh-app web preset boots. */
export const DEFAULT_WEB_BUNDLES = ['@deepseek-ai/dsh-base', '@deepseek-ai/dsh-web-app']

/**
 * The profile manifest, matching the shape `initProfile` writes.
 * @param name - the profile name (the `--profile` value).
 * @param bundles - ordered `dsh.profile.bundles`.
 * @param patchReload - `startup` for the packaged default, `live` for dev.
 */
export function profileManifest(name, bundles = DEFAULT_WEB_BUNDLES, patchReload = 'startup') {
  return `${JSON.stringify({
    name: `dsh-profile-${name}`,
    private: true,
    dependencies: {},
    dsh: { profile: { bundles: [...bundles], patchReload } },
  }, undefined, 2)}\n`
}
