/**
 * Host chrome for the harness window.
 *
 * Injected as the webview's initialization script, i.e. before the runtime's page
 * is parsed. This file is the whole of what the shell writes into a page it does
 * not own, and it is deliberately small (docs/spec-shell-ux.md §2.5, X1):
 *
 *   1. it sets the one marker attribute the runtime documents for a macOS host
 *      ("set only by the desktop preload", read at render time), which is what
 *      switches on its desktop layout: a hidden title bar, a 52 px band that
 *      clears the traffic lights, and a translucent sidebar over our window's
 *      vibrancy;
 *   2. it moves the window: the band the runtime reserves is *documented* as the
 *      window's drag region, but it does that with `-webkit-app-region: drag`,
 *      which only Electron implements — in a WKWebView the band is inert.
 *
 * It never restyles or restructures the runtime's UI, never draws anything, and
 * never reads its DOM. The rule it applies is about a screen region (the top
 * 52 px, or anywhere with Command held), not about the runtime's markup.
 */

(() => {
  'use strict';

  /** The band the runtime's macOS layout reserves above the sidebar. */
  const BAND = 52;

  /** Elements a drag must not steal a press from. */
  const INTERACTIVE =
    'a,button,input,select,textarea,label,summary,[role="button"],[role="link"],[contenteditable="true"]';

  const invoke = (command, args) => {
    const internals = window.__TAURI_INTERNALS__;
    if (internals === undefined) throw new Error('dsh-app: the Tauri bridge is missing');
    return internals.invoke(command, args);
  };

  /** This window's own label; the window commands take one, and it must be ours. */
  const label = () => window.__TAURI_INTERNALS__?.metadata?.currentWindow?.label ?? null;

  /**
   * A window-plugin command aimed at this window.
   *
   * Plugin commands rather than this app's own, because the runtime's page is a
   * *remote* origin as far as Tauri is concerned: custom commands are refused
   * there, and the capability this window is granted names exactly two controls
   * (capabilities/harness.json). The app's own commands stay unreachable from a
   * page we did not write, which is the point.
   */
  const windowCommand = (command, args = {}) =>
    invoke(`plugin:window|${command}`, { label: label(), ...args });

  /* 1. The marker. The runtime reads it at render time and tolerates it arriving
   *    as late as DOMContentLoaded, so setting it at every milestone is cheap. */
  const mark = () => {
    if (document.documentElement !== null) {
      document.documentElement.dataset.platform = 'darwin';
    }
  };
  mark();
  document.addEventListener('readystatechange', mark);
  document.addEventListener('DOMContentLoaded', mark);

  /* 2. Dragging. */
  const draggable = (event) => {
    if (event.type === 'mousedown' && event.button !== 0) return false;
    if (event.target instanceof Element && event.target.closest(INTERACTIVE) !== null) return false;
    return event.clientY < BAND || event.metaKey;
  };

  document.addEventListener(
    'mousedown',
    (event) => {
      if (!draggable(event)) return;
      event.preventDefault();
      event.stopPropagation();
      try {
        windowCommand('start_dragging').catch(() => {});
      } catch (error) {
        window.__DSH_HOST_ERROR__ = String(error);
      }
    },
    true,
  );

  document.addEventListener(
    'dblclick',
    (event) => {
      if (!draggable(event)) return;
      event.preventDefault();
      try {
        windowCommand('toggle_maximize').catch(() => {});
      } catch (error) {
        window.__DSH_HOST_ERROR__ = String(error);
      }
    },
    true,
  );
})();
