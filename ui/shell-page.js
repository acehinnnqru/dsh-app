/**
 * Shell page controller.
 *
 * One payload in, one view out. The host owns every verdict (does this path run,
 * can this home be created, is this profile launchable); this file owns only
 * which surface is on screen and what a button means there.
 *
 * The view is derived, not navigated: the payload says whether a runtime is
 * configured and whether the guide has been finished, and those two facts are
 * the whole state machine. `step` exists only to hold the guide in place while
 * the user walks it.
 *
 * @see docs/spec-shell-ux.md §3 (the state machine), §11 (the probe reads
 * `document.documentElement.dataset.view` and checks it against these names)
 */

import { invoke, listen } from './tauri.js';
import { launcher, paintCheck, profileStep, runtimeStep, settingsSheet } from './views.js';

const el = {
  reload: document.getElementById('reload'),
  settings: document.getElementById('settings'),
  banner: document.getElementById('banner'),
  view: document.getElementById('view'),
  sheet: document.getElementById('sheet'),
};

let state = { profiles: [], problems: [] };

/** Failures this page raised itself, kept beside the host's problem list. */
let clientProblems = [];

/** The guide's step while it is being walked; null means "derive it". */
let step = null;

/** The profile the guide has selected. Survives a re-render, by design. */
let selection = null;

let sheetOpen = false;

const currentView = () => step ?? (state.onboarded ? 'launcher' : state.configured ? 'profile' : 'runtime');

/**
 * Run one host call and surface a rejection.
 *
 * A command that does not exist, or one that throws, otherwise fails silently:
 * the button looks dead and nothing anywhere says why — which is exactly how a
 * misnamed invoke hid for a whole test round.
 */
async function act(work) {
  clientProblems = [];
  try {
    await work();
  } catch (error) {
    clientProblems = [String(error)];
  }
  await refresh();
  render();
}

/* ------------------------------------------------------------ the guide */

const guideHandlers = {
  onContinue: async ({ dsh, home, dshStatus, homeStatus, submit }) => {
    submit.disabled = true;
    try {
      const report = await invoke('save_settings', { dsh: dsh.trim(), home: home.trim() });
      paintCheck(dshStatus, report.dsh);
      paintCheck(homeStatus, report.home);
      // A failed verdict leaves the form exactly as it is: the status line says
      // why, and rebuilding it would throw away what the user typed.
      if (!report.ok) return;
      await refresh();
      // A repair visit ends on the launcher: this user already answered the
      // profile question once.
      step = state.onboarded ? null : 'profile';
      render();
    } catch (error) {
      paintCheck(dshStatus, { error: String(error) });
    } finally {
      submit.disabled = false;
    }
  },
  onBack: () => {
    step = 'runtime';
    render();
  },
  onSelect: (profileId) => {
    selection = profileId;
    render();
  },
  onReload: () => act(() => invoke('reload_profiles')),
  onStart: async (profileId) => {
    await act(async () => {
      await invoke('finish_onboarding', { profile: profileId });
      if (profileId !== null) await invoke('start_profile', { profileId });
    });
    step = null;
    selection = profileId ?? selection;
    render();
  },
};

/* --------------------------------------------------------- the launcher */

const launcherHandlers = {
  onReload: () => act(() => invoke('reload_profiles')),
  onStart: (profileId) => act(() => invoke('start_profile', { profileId })),
  onStop: (profileId) => act(() => invoke('stop_profile', { profileId })),
  onOpen: (profileId) => act(() => invoke('open_harness', { profileId })),
};

/* ------------------------------------------------------------- the sheet */

function openSheet() {
  if (sheetOpen) return;
  sheetOpen = true;
  el.sheet.replaceChildren(
    settingsSheet(state, {
      onClose: closeSheet,
      onSave: async ({ dsh, home, dshStatus, homeStatus, submit }) => {
        submit.disabled = true;
        try {
          const report = await invoke('save_settings', { dsh: dsh.trim(), home: home.trim() });
          paintCheck(dshStatus, report.dsh);
          paintCheck(homeStatus, report.home);
          if (!report.ok) return;
          closeSheet();
          await refresh();
          render();
        } catch (error) {
          paintCheck(dshStatus, { error: String(error) });
        } finally {
          submit.disabled = false;
        }
      },
    }),
  );
  el.sheet.hidden = false;
  el.sheet.querySelector('input')?.focus();
}

function closeSheet() {
  if (!sheetOpen) return;
  sheetOpen = false;
  el.sheet.hidden = true;
  el.sheet.replaceChildren();
  el.settings.focus();
}

/* --------------------------------------------------------------- render */

function render() {
  const view = currentView();
  if (view === 'launcher') step = null;
  document.documentElement.dataset.view =
    view === 'runtime' ? 'onboarding-1' : view === 'profile' ? 'onboarding-2' : 'launcher';

  el.reload.hidden = view === 'runtime';
  el.settings.hidden = view !== 'launcher';

  const problems = [...(state.problems ?? []), ...clientProblems];
  const bannerText = problems.length === 1 ? problems[0] : `${problems.length} problems from the last load`;
  el.banner.hidden = problems.length === 0;
  el.banner.replaceChildren(
    document.createTextNode(bannerText),
    Object.assign(document.createElement('span'), { className: 'spacer' }),
    Object.assign(document.createElement('button'), {
      type: 'button',
      textContent: 'Dismiss',
      onclick: () => act(() => invoke('dismiss_problems')),
    }),
  );

  if (view === 'runtime') {
    el.view.replaceChildren(runtimeStep(state, guideHandlers));
  } else if (view === 'profile') {
    const selectable = (state.profiles ?? []).filter((profile) => !profile.startNote);
    if (selection === null || !selectable.some((profile) => profile.id === selection)) {
      selection = selectable[0]?.id ?? null;
    }
    el.view.replaceChildren(profileStep(state, { ...guideHandlers, selection }));
  } else {
    el.view.replaceChildren(launcher(state, launcherHandlers));
  }

  // The sheet is not re-rendered from here: a status event must not fight what
  // the user is typing into it. Its values are read when it opens, and Save is
  // the only thing that writes.
  if (sheetOpen && view !== 'launcher') closeSheet();
}

async function refresh() {
  try {
    state = await invoke('shell_state');
  } catch (error) {
    state = { profiles: [], problems: [String(error)] };
  }
}

/* ------------------------------------------------------- window wiring */

/**
 * `⌘,` opens the settings sheet; `Esc` closes whatever is on top.
 *
 * The window's own controls belong to AppKit now: the title bar drags, the
 * traffic lights close, minimize and zoom, and the page handles nothing of that.
 */
function trackShortcuts({ onSettings, onEscape }) {
  window.addEventListener('keydown', (event) => {
    if (event.key === ',' && (event.metaKey || event.ctrlKey)) {
      event.preventDefault();
      onSettings();
      return;
    }
    if (event.key === 'Escape') onEscape();
  });
}

/**
 * Accept a dropped file: the guide's executable field is a path nobody wants to
 * type, and Finder is where that path usually is.
 */
async function trackFileDrops({ onPath, onHighlight }) {
  await listen('tauri://drag-enter', () => onHighlight(true));
  await listen('tauri://drag-leave', () => onHighlight(false));
  await listen('tauri://drag-drop', (event) => {
    onHighlight(false);
    const path = event?.payload?.paths?.[0];
    if (typeof path === 'string' && path !== '') onPath(path);
  });
}

/* ------------------------------------------------------------------ boot */

trackShortcuts({
  onSettings: () => {
    if (currentView() === 'launcher') openSheet();
  },
  onEscape: () => closeSheet(),
});

el.reload.addEventListener('click', () => act(() => invoke('reload_profiles')));
el.settings.addEventListener('click', () => openSheet());

await refresh();
render();

// The window is created hidden, and this is what shows it. Not inside
// requestAnimationFrame: a hidden window is not guaranteed to run animation
// frames at all, and the first thing that happened when this was moved there was
// the watchdog stepping in three seconds later. The DOM is built by the line
// above, so there is nothing left to wait for.
invoke('shell_ready').catch((error) => {
  // Not silent: the watchdog will show the window anyway, but the reason belongs
  // in the log next to it.
  console.error('shell_ready failed:', error);
});

// Subscriptions and drop wiring come after the first paint: the page is already
// usable by then, and a rejected listen must not cost it its render.
listen('dsh://status', () => {
  void refresh().then(render);
}).catch(() => {});

// The guide's executable field is a path nobody wants to type, so a dropped file
// fills whichever field is on screen.
trackFileDrops({
  onHighlight: (dropping) => {
    document.documentElement.dataset.dropping = String(dropping);
  },
  onPath: (path) => {
    const input = (sheetOpen ? el.sheet : el.view).querySelector('input');
    if (input === null) return;
    input.value = path;
    input.focus();
  },
}).catch(() => {});
