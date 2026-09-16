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
  runtime: document.getElementById('fact-runtime'),
  node: document.getElementById('fact-node'),
  home: document.getElementById('fact-home'),
  ui: document.getElementById('fact-ui'),
};

/** Phases where the runtime is up but not usable yet. */
const WORKING = new Set(['materializing', 'starting']);

let state = { presets: [], problems: [] };

/** One preset row. Rebuilt on every refresh, so it stays declarative. */
function presetRow(preset) {
  const li = document.createElement('li');
  li.className = 'preset';

  const head = document.createElement('div');
  head.className = 'preset-head';

  const name = document.createElement('span');
  name.className = 'preset-name';
  name.textContent = preset.label;

  const badge = document.createElement('span');
  badge.className = 'badge';
  badge.dataset.phase = preset.phase;
  badge.textContent = preset.phase;

  head.append(name, badge);
  li.append(head);

  const meta = document.createElement('p');
  meta.className = 'preset-meta';
  const bits = [`kind ${preset.kind}`, `profile ${preset.profile}`, `patchReload ${preset.patchReload}`];
  if (preset.patchFiles.length > 0) bits.push(`${preset.patchFiles.length} patch layer(s)`);
  if (preset.needsInternals) bits.push('needs internals');
  meta.textContent = bits.join(' · ');
  li.append(meta);

  const actions = document.createElement('div');
  actions.className = 'preset-actions';

  const running = preset.phase !== 'idle' && preset.phase !== 'exited' && preset.phase !== 'failed';
  const ready = preset.phase === 'ready';

  const start = document.createElement('button');
  start.type = 'button';
  start.textContent = running ? 'Restart' : 'Start';
  start.disabled = WORKING.has(preset.phase) || preset.kind !== 'web';
  start.title = preset.kind === 'web' ? '' : `kind "${preset.kind}" is not implemented yet`;
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

  el.runtime.textContent = state.runtimeDir ?? '—';
  el.node.textContent = state.node ?? '—';
  el.home.textContent = state.home ?? '—';
  // Proves which page build is running, so "fields stayed empty" can never be
  // diagnosed from a screenshot of stale UI again.
  el.ui.textContent = state.uiVersion ?? '—';
}

async function refresh() {
  try {
    state = await invoke('shell_state');
  } catch (error) {
    state = { presets: [], problems: [String(error)] };
  }
  render();
}

el.reload.addEventListener('click', async () => {
  try {
    await invoke('reload_presets');
  } catch (error) {
    state.problems = [...(state.problems ?? []), String(error)];
  }
  await refresh();
});

await listen('dsh://status', () => {
  void refresh();
});

await refresh();
