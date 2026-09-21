/**
 * Shell page: a preset manager for the supervised harness runtimes.
 *
 * The page owns no harness knowledge — it renders the state the Rust side
 * reports and forwards button presses back to it.
 */

import { invoke, listen } from './tauri.js';

const el = {
  presets: document.getElementById('presets'),
  empty: document.getElementById('empty'),
  problems: document.getElementById('problems'),
  reload: document.getElementById('reload'),
  active: document.getElementById('fact-active'),
  dsh: document.getElementById('fact-dsh'),
  previous: document.getElementById('fact-previous'),
  channel: document.getElementById('fact-channel'),
  note: document.getElementById('update-note'),
  check: document.getElementById('check-update'),
  apply: document.getElementById('apply-update'),
  node: document.getElementById('fact-node'),
  rollback: document.getElementById('rollback'),
  home: document.getElementById('fact-home'),
  ui: document.getElementById('fact-ui'),
  config: document.getElementById('config'),
  configTitle: document.getElementById('config-title'),
  configHint: document.getElementById('config-hint'),
  nodePath: document.getElementById('node-path'),
  nodeStatus: document.getElementById('node-status'),
  pnpmPath: document.getElementById('pnpm-path'),
  pnpmStatus: document.getElementById('pnpm-status'),
  configSave: document.getElementById('config-save'),
};

/** Phases where the runtime is up but not usable yet. */
const WORKING = new Set(['materializing', 'starting']);

let state = { presets: [], problems: [] };

/** One launchable row. Rebuilt on every refresh, so it stays declarative. */
function presetRow(preset) {
  const li = document.createElement('li');
  li.className = 'preset';

  const head = document.createElement('div');
  head.className = 'preset-head';

  const title = document.createElement('span');
  title.className = 'preset-title';

  const name = document.createElement('span');
  name.className = 'preset-name';
  name.textContent = preset.label;

  // A preset is a file the user wrote; a profile is a directory the harness
  // already had. They behave the same here, but they are not the same thing.
  const origin = document.createElement('span');
  origin.className = 'tag';
  origin.dataset.origin = preset.origin;
  origin.textContent = preset.origin;

  title.append(name, origin);

  const badge = document.createElement('span');
  badge.className = 'badge';
  badge.dataset.phase = preset.phase;
  badge.textContent = preset.phase;

  head.append(title, badge);
  li.append(head);

  const meta = document.createElement('p');
  meta.className = 'preset-meta';
  const bits = [`kind ${preset.kind}`, `profile ${preset.profile}`, `patchReload ${preset.patchReload}`];
  if (preset.origin === 'profile') bits.push(`${preset.bundles.length} bundle(s)`);
  if (preset.patchFiles.length > 0) bits.push(`${preset.patchFiles.length} patch layer(s)`);
  if (preset.needsInternals) bits.push('needs internals');
  meta.textContent = bits.join(' · ');
  li.append(meta);

  // Where the row comes from. The stylesheet trims it from the left, because
  // the last segment is the part that answers "which file do I edit".
  const source = document.createElement('p');
  source.className = 'preset-source mono';
  source.textContent = preset.source;
  source.title = preset.source;
  li.append(source);

  // The host decides whether a row is startable and why not; the page only
  // reports it, so there is exactly one definition of "this can be launched".
  if (preset.startNote) {
    const note = document.createElement('p');
    note.className = 'preset-note';
    note.textContent = preset.startNote;
    li.append(note);
  }

  const actions = document.createElement('div');
  actions.className = 'preset-actions';

  const running = preset.phase !== 'idle' && preset.phase !== 'exited' && preset.phase !== 'failed';
  const ready = preset.phase === 'ready';

  const start = document.createElement('button');
  start.type = 'button';
  start.textContent = running ? 'Restart' : 'Start';
  start.disabled = WORKING.has(preset.phase) || Boolean(preset.startNote);
  start.title = preset.startNote ?? '';
  start.addEventListener('click', async () => {
    if (running) await invoke('stop_preset', { presetId: preset.id });
    await invoke('start_preset_command', { presetId: preset.id });
  });

  const open = document.createElement('button');
  open.type = 'button';
  open.textContent = 'Open';
  open.disabled = !ready;
  open.addEventListener('click', () => invoke('open_harness', { presetId: preset.id }));

  const stop = document.createElement('button');
  stop.type = 'button';
  stop.textContent = 'Stop';
  stop.disabled = !running;
  stop.addEventListener('click', () => invoke('stop_preset', { presetId: preset.id }));

  actions.append(start, open, stop);
  li.append(actions);

  const tail = preset.stderrTail ?? [];
  if (tail.length > 0) {
    const details = document.createElement('details');
    const summary = document.createElement('summary');
    summary.textContent = `stderr (last ${tail.length} lines)`;
    const pre = document.createElement('pre');
    pre.textContent = tail.join('\n');
    details.append(summary, pre);
    li.append(details);
  }

  return li;
}

function render() {
  el.presets.replaceChildren(...state.presets.map(presetRow));
  el.empty.hidden = state.presets.length > 0;

  const problems = state.problems ?? [];
  el.problems.hidden = problems.length === 0;
  el.problems.textContent = problems.join('\n');

  renderConfig();

  const runtime = state.runtime ?? {};
  el.active.textContent = runtime.active ?? '—';
  el.dsh.textContent = runtime.active_dsh_version ?? '—';
  el.previous.textContent = runtime.previous ?? '(none)';
  el.node.textContent = state.node ?? '—';
  el.home.textContent = state.home ?? '—';

  // A version change happens with everything stopped (design doc D3.4), and the
  // host enforces the same rule, so the button says so rather than failing late.
  const anyRunning = state.presets.some((preset) => preset.phase !== 'idle');
  const channel = state.channel ?? {};
  el.channel.textContent = channel.url ?? '(none configured)';

  // An update is offered only when one exists and nothing is running: the
  // runtime in use cannot be replaced under a live child (design doc D3.4).
  const check = channel.lastCheck;
  const available = Boolean(check?.update_available);
  el.apply.disabled = !available || anyRunning;
  el.check.disabled = !channel.url;

  const notes = [];
  if (check) {
    if (check.update_available) {
      notes.push(`Update available: ${check.current ?? '(none)'} → ${check.available}`);
    } else if (check.available) {
      notes.push(`Up to date (${check.available}).`);
    } else if (check.detail) {
      notes.push(check.detail);
    }
  }
  if (available && anyRunning) notes.push('Stop every running preset to install it.');
  notes.push(`platform ${channel.platform ?? '?'} · shellContract ${channel.shellContract ?? '?'}`);
  el.note.hidden = false;
  el.note.textContent = notes.join(' ');

  el.rollback.disabled = !runtime.previous || anyRunning;
  el.rollback.title = !runtime.previous
    ? 'No previous runtime is available'
    : anyRunning
      ? 'Stop every running preset first'
      : 'Switch back to the previous runtime';
  // Proves which page build is running, so "fields stayed empty" can never be
  // diagnosed from a screenshot of stale UI again.
  el.ui.textContent = state.uiVersion ?? '—';
}

/* The Node / pnpm form.
 *
 * Only the shell page can render this: a `slim` build has no Node until the form
 * is filled in, so the runtime that would normally draw a settings page does not
 * exist yet. Nothing here validates on its own — every verdict comes from the
 * host, so there is exactly one definition of "this path works".
 */

let configTouched = false;

/** Text for one validated path, as a status line. */
function checkText(label, check) {
  if (check === undefined || check === null) return '';
  if (check.error) return `${check.path}: ${check.error}`;
  const version = check.version ? ` ${check.version}` : '';
  const warnings = check.warnings ?? [];
  return [`${label}${version} works`, ...warnings].join('\n');
}

function setStatus(element, text, kind) {
  element.textContent = text;
  element.dataset.kind = kind ?? '';
  element.hidden = text === '';
}

/**
 * Show the form whenever the shell cannot launch a runtime, or when something the
 * user saved does not work. "Nothing configured yet" is not an error: it is the
 * prompt (docs/spec-slim-runtime-config.md §1).
 */
function renderConfig() {
  const settings = state.settings ?? {};
  const needsConfig = !state.configured || Boolean(state.nodeProblem) || Boolean(settings.problem);

  el.config.hidden = !needsConfig;
  if (!needsConfig) return;

  el.configTitle.textContent = state.configured ? 'Runtime problem' : 'A Node runtime is required';

  const hint = [];
  hint.push(
    state.configured
      ? `Currently using ${state.node} (${state.nodeSource ?? 'unknown source'}).`
      : 'This build carries no Node. Name the Node this app should run the harness with.',
  );
  if (settings.problem) hint.push(settings.problem);
  if (state.nodeProblem) hint.push(state.nodeProblem);
  el.configHint.textContent = hint.join('\n');

  // Never fight the user's typing: the payload also arrives on every status
  // event, including ones unrelated to this form.
  if (!configTouched) {
    el.nodePath.value = settings.node ?? '';
    el.pnpmPath.value = settings.pnpm ?? '';
    if (el.nodeStatus.textContent === '') {
      setStatus(
        el.nodeStatus,
        state.configured ? `${state.node} works` : '',
        state.configured ? 'ok' : undefined,
      );
    }
    if (settings.pnpm === null || settings.pnpm === undefined) {
      setStatus(
        el.pnpmStatus,
        state.bundledPnpm ? 'Using the pnpm this app carries.' : '',
        undefined,
      );
    }
    if (!state.configured && !settings.pnpm && !state.bundledPnpm) {
      setStatus(el.pnpmStatus, 'Plugins stay unavailable until a pnpm is named.', 'warn');
    }
  }
}

el.configSave.addEventListener('click', async () => {
  const node = el.nodePath.value.trim();
  const pnpm = el.pnpmPath.value.trim();
  setStatus(el.nodeStatus, 'Checking…', undefined);
  setStatus(el.pnpmStatus, '', undefined);
  try {
    const report = await invoke('save_settings', { node, pnpm: pnpm === '' ? null : pnpm });
    setStatus(
      el.nodeStatus,
      checkText('Node', report.node) || 'Not saved: the Node path is required.',
      report.node?.ok ? 'ok' : 'err',
    );
    if (report.pnpm) {
      setStatus(el.pnpmStatus, checkText('pnpm', report.pnpm), report.pnpm.ok ? 'ok' : 'err');
    }
    configTouched = false;
  } catch (error) {
    setStatus(el.nodeStatus, String(error), 'err');
  }
  await refresh();
});

async function refresh() {
  try {
    state = await invoke('shell_state');
  } catch (error) {
    state = { presets: [], problems: [String(error)] };
  }
  render();
}

el.check.addEventListener('click', async () => {
  el.note.hidden = false;
  el.note.textContent = 'Checking…';
  try {
    await invoke('check_update');
  } catch (error) {
    state.problems = [...(state.problems ?? []), String(error)];
  }
  await refresh();
});

el.apply.addEventListener('click', async () => {
  el.note.hidden = false;
  el.note.textContent = 'Downloading and verifying… the window stays usable.';
  try {
    const outcome = await invoke('apply_update');
    state.problems = [];
    el.note.textContent = `Activated ${outcome.activated} (replaced ${outcome.replaced ?? 'nothing'}).`;
  } catch (error) {
    state.problems = [...(state.problems ?? []), String(error)];
  }
  await refresh();
});

el.rollback.addEventListener('click', async () => {
  try {
    await invoke('rollback_runtime');
  } catch (error) {
    state.problems = [...(state.problems ?? []), String(error)];
  }
  await refresh();
});

el.reload.addEventListener('click', async () => {
  try {
    await invoke('reload_presets');
  } catch (error) {
    state.problems = [...(state.problems ?? []), String(error)];
  }
  await refresh();
});

await listen('dsh://status', (event) => {
  // Starting a preset without a Node is guidance, not a failure: the host says so
  // with this phase instead of pushing an error into the problems list
  // (docs/spec-slim-runtime-config.md §3.3).
  if (event?.payload?.phase === 'unconfigured') {
    el.config.hidden = false;
    el.configTitle.textContent = 'A Node runtime is required';
    el.configHint.textContent = event.payload.error ?? 'Name the Node this app should use.';
    el.nodePath.focus();
    return;
  }
  void refresh();
});

await refresh();
