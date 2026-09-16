/**
 * The shell page's Tauri bridge.
 *
 * Deliberately not `window.__TAURI__`: that global is produced by the Tauri CLI
 * at build time and is simply absent when the crate is built with plain
 * `cargo build` (which is what the Nix derivation does). `__TAURI_INTERNALS__`
 * is injected by the runtime on every load regardless of build method, so using
 * it keeps the page working either way and needs no bundler.
 *
 * @see crates/tauri/src/manager/webview.rs — the runtime initialization scripts
 */

const internals = window.__TAURI_INTERNALS__;

if (internals === undefined) {
  // Nothing can work without the bridge; say so on the page instead of letting
  // every later call fail with an unrelated error.
  document.body.textContent =
    'This page was not loaded by the dsh-app shell (the Tauri bridge is missing).';
  throw new Error('dsh-app: window.__TAURI_INTERNALS__ is missing');
}

/** Invoke a command registered by the Rust side. */
export const invoke = (command, args) => internals.invoke(command, args);

/**
 * Subscribe to an event emitted by the Rust side.
 * @returns a promise for the unlisten function.
 */
export const listen = (event, handler) =>
  internals.invoke('plugin:event|listen', {
    event,
    target: { kind: 'Any' },
    handler: internals.transformCallback(handler),
  }).then((id) => () => internals.invoke('plugin:event|unlisten', { event, eventId: id }));
