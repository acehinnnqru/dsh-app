/**
 * Shell page: a launcher for DeepSeek Harness profiles.
 *
 * The page owns no harness knowledge — it renders the state the Rust side
 * reports and forwards button presses back to it. It ships no runtime either, so
 * the one thing it has to collect is the path of the dsh executable.
 */

import { invoke, listen } from './tauri.js';

const el = {
  profiles: document.getElementById('profiles'),
  empty: document.getElementById('empty'),
  problems: document.getElementById('problems'),
  reload: document.getElementById('reload'),
  dsh: document.getElementById('fact-dsh'),
  dshSource: document.getElementById('fact-dsh-source'),
  home: document.getElementById('fact-home'),
  ui: document.getElementById('fact-ui'),
  config: document.getElementById('config'),
  configTitle: document.getElementById('config-title'),
  configHint: document.getElementById('config-hint'),
  settings: document.getElementById('fact-settings'),
  dshPath: document.getElementById('dsh-path'),
  dshStatus: document.getElementById('dsh-status'),
  dshHome: document.getElementById('dsh-home'),
  homeStatus: document.getElementById('home-status'),
  configSave: document.getElementById('config-save'),
};

/** Phases where the child exists but its URL has not arrived yet. */
const WORKING = new Set(['starting']);

let state = { profiles: [], problems: [] };

/** Failures this page raised itself, kept beside the host's problem list. */
let clientProblems = [];

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
}

/** One launchable row. Rebuilt on every refresh, so it stays declarative. */
function profileRow(profile) {
  const li = document.createElement('li');
  li.className = 'profile';

  const head = document.createElement('div');
  head.className = 'profile-head';

  const title = document.createElement('span');
  title.className = 'profile-title';

  const name = document.createElement('span');
  name.className = 'profile-name';
  name.textContent = profile.label;

  title.append(name);

  const badge = document.createElement('span');
  badge.className = 'badge';
  badge.dataset.phase = profile.phase;
  badge.textContent = profile.phase;

  head.append(title, badge);
  li.append(head);

  const meta = document.createElement('p');
  meta.className = 'profile-meta';
  const bits = ['kind ' + profile.kind, 'profile ' + profile.profile];
  bits.push(profile.bundles.length + ' bundle(s)');
  meta.textContent = bits.join(' · ');
  li.append(meta);

  // Where the row comes from. The stylesheet trims it from the left, because the
  // last segment is the part that answers "which directory do I edit".
  const source = document.createElement('p');
  source.className = 'profile-source mono';
  source.textContent = profile.source;
  source.title = profile.source;
  li.append(source);

  // The host decides whether a row is startable and why not; the page only
  // reports it, so there is exactly one definition of "this can be launched".
  if (profile.startNote) {
    const note = document.createElement('p');
    note.className = 'profile-note';
    note.textContent = profile.startNote;
    li.append(note);
  }

  const actions = document.createElement('div');
  actions.className = 'profile-actions';

  const running = profile.phase !== 'idle' && profile.phase !== 'exited' && profile.phase !== 'failed';
  const ready = profile.phase === 'ready';

  const start = document.createElement('button');
  start.type = 'button';
  start.textContent = running ? 'Restart' : 'Start';
  start.disabled = WORKING.has(profile.phase) || Boolean(profile.startNote);
  start.title = profile.startNote ?? '';
  start.addEventListener('click', () =>
    act(async () => {
      if (running) await invoke('stop_profile', { profileId: profile.id });
      await invoke('start_profile', { profileId: profile.id });
    }),
  );

  const open = document.createElement('button');
  open.type = 'button';
  open.textContent = 'Open';
  open.disabled = !ready;
  open.addEventListener('click', () => act(() => invoke('open_harness', { profileId: profile.id })));

  const stop = document.createElement('button');
  stop.type = 'button';
  stop.textContent = 'Stop';
  stop.disabled = !running;
  stop.addEventListener('click', () => act(() => invoke('stop_profile', { profileId: profile.id })));

  actions.append(start, open, stop);
  li.append(actions);

  const tail = profile.stderrTail ?? [];
  if (tail.length > 0) {
    const details = document.createElement('details');
    const summary = document.createElement('summary');
    summary.textContent = 'stderr (last ' + tail.length + ' lines)';
    const pre = document.createElement('pre');
    pre.textContent = tail.join('\n');
    details.append(summary, pre);
    li.append(details);
  }

  return li;
}

function render() {
  el.profiles.replaceChildren(...state.profiles.map(profileRow));
  el.empty.hidden = state.profiles.length > 0;

  const problems = [...(state.problems ?? []), ...clientProblems];
  el.problems.hidden = problems.length === 0;
  el.problems.textContent = problems.join('\n');

  renderConfig();

  el.dsh.textContent = state.dsh ?? '—';
  el.dshSource.textContent = state.dshSource ?? '—';
  el.home.textContent = state.home ?? '—';
  // Where to edit these by hand. The file is TOML precisely so that this is a
  // reasonable thing to do.
  el.settings.textContent = state.settingsFile ?? '—';
  // Proves which page build is running, so "fields stayed empty" can never be
  // diagnosed from a screenshot of stale UI again.
  el.ui.textContent = state.uiVersion ?? '—';
}

/* The dsh form.
 *
 * Only the shell page can render this: with nothing configured there is no
 * runtime to draw a settings page, and this app ships none. Nothing here
 * validates on its own — every verdict comes from the host, so there is exactly
 * one definition of "this path works".
 */

let configTouched = false;

function setStatus(element, text, kind) {
  element.textContent = text;
  element.dataset.kind = kind ?? '';
  element.hidden = text === '';
}

/**
 * Show the form whenever the shell cannot launch anything, or when the path the
 * user saved does not work. "Nothing configured yet" is not an error: it is the
 * prompt.
 */
function renderConfig() {
  const settings = state.settings ?? {};
  const needsConfig = !state.configured || Boolean(state.dshProblem) || Boolean(settings.problem);

  el.config.hidden = !needsConfig;
  if (!needsConfig) return;

  el.configTitle.textContent = state.configured ? 'Runtime problem' : 'A dsh executable is required';

  const hint = [];
  hint.push(
    state.configured
      ? 'Currently launching ' + state.dsh + ' (chosen by ' + (state.dshSource ?? 'unknown') + ').'
      : 'This app ships no runtime. Name the dsh executable it should launch.',
  );
  if (settings.problem) hint.push(settings.problem);
  if (state.dshProblem) hint.push(state.dshProblem);
  el.configHint.textContent = hint.join('\n');

  // Never fight the user's typing: the payload also arrives on every status
  // event, including ones unrelated to this form.
  if (!configTouched) {
    el.dshPath.value = settings.dsh ?? '';
    // Prefilled with what is actually in use, so the field answers "which home am
    // I running against" instead of asking the user to know it. A bare ~/.dsh is
    // accepted and expanded.
    el.dshHome.value = settings.home ?? state.home ?? '';
    if (el.dshStatus.textContent === '' && state.configured) {
      setStatus(el.dshStatus, state.dsh + ' works', 'ok');
    }
  }
}

for (const field of [el.dshPath, el.dshHome]) {
  field.addEventListener('input', () => {
    configTouched = true;
  });
}

/** One validated field, as a status line. */
function checkLine(label, check) {
  if (!check) return 'Not saved: both fields are required.';
  if (check.error) return check.path + ': ' + check.error;
  return check.path + (check.version ? ' ' + check.version : '') + ' works';
}

el.configSave.addEventListener('click', async () => {
  const dsh = el.dshPath.value.trim();
  const dshHome = el.dshHome.value.trim();
  setStatus(el.dshStatus, 'Checking…', undefined);
  setStatus(el.homeStatus, '', undefined);
  try {
    const report = await invoke('save_settings', { dsh, home: dshHome });
    setStatus(el.dshStatus, checkLine('dsh', report.dsh), report.dsh?.ok ? 'ok' : 'err');
    if (report.home) {
      setStatus(el.homeStatus, checkLine('home', report.home), report.home.ok ? 'ok' : 'err');
    }
    configTouched = false;
  } catch (error) {
    setStatus(el.dshStatus, String(error), 'err');
  }
  await refresh();
});

async function refresh() {
  try {
    state = await invoke('shell_state');
  } catch (error) {
    state = { profiles: [], problems: [String(error)] };
  }
  render();
}

el.reload.addEventListener('click', () => act(() => invoke('reload_profiles')));

await listen('dsh://status', (event) => {
  // Starting a profile without a configured executable is guidance, not a
  // failure: the host says so with this phase instead of pushing an error into
  // the problems list.
  if (event?.payload?.phase === 'unconfigured') {
    el.config.hidden = false;
    el.configTitle.textContent = 'A dsh executable is required';
    el.configHint.textContent = event.payload.error ?? 'Name the dsh executable this app should launch.';
    el.dshPath.focus();
    return;
  }
  void refresh();
});

await refresh();
