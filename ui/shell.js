/**
 * Shell page entry.
 *
 * The page body lives in `shell-page.js` and is loaded through a dynamic import
 * wrapped in a try/catch. That is deliberate: an ES module that throws while
 * evaluating reports nothing to the host, so a broken bridge once hid behind a
 * screen of placeholder values with no diagnostic anywhere. Catching it here
 * turns any such failure into a visible message on the page.
 */

const report = (message) => {
  const target = document.getElementById('fatal');
  if (target !== null) {
    target.hidden = false;
    target.textContent = message;
  }
  // Readable from the host when the shell runs with DSH_APP_DEBUG_STATE=1.
  window.__DSH_PAGE_ERROR__ = message;
};

try {
  await import('./shell-page.js');
} catch (error) {
  report(`The shell page failed to start: ${error?.stack ?? error}`);
}
