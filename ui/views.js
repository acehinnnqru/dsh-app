/**
 * The shell page's views.
 *
 * Pure DOM builders: each one takes the host's payload and a bag of handlers and
 * returns an element. Nothing here invokes a command or reads state of its own,
 * so a view can be rebuilt on every payload without losing anything the user
 * typed — the controller keeps what must survive (a selection, a half-filled
 * form) and hands it back in.
 *
 * @see docs/spec-shell-ux.md §4 (the guide), §5 (the launcher), §6 (the sheet)
 */

/** A tiny element builder: enough to read, not enough to be a framework. */
function el(tag, props = {}, ...children) {
  const node = document.createElement(tag);
  for (const [key, value] of Object.entries(props)) {
    if (value === null || value === undefined || value === false) continue;
    if (key === 'class') node.className = value;
    else if (key === 'text') node.textContent = value;
    else if (key.startsWith('on')) node.addEventListener(key.slice(2).toLowerCase(), value);
    else node.setAttribute(key, value === true ? '' : String(value));
  }
  for (const child of children.flat(Infinity)) {
    if (child === null || child === undefined || child === false) continue;
    node.append(child instanceof Node ? child : document.createTextNode(String(child)));
  }
  return node;
}

/**
 * A profile directory, shortened the way a shell would print it.
 *
 * The LRM in front is not decoration: the stylesheet trims these paths from the
 * left with `direction: rtl`, and without it the bidi algorithm moves a leading
 * slash to the far end of the line.
 */
function shortPath(path, home) {
  if (typeof home !== 'string' || home === '') return `\u200e${path}`;
  const prefix = home.endsWith('/') ? home : `${home}/`;
  const text = path.startsWith(prefix) ? `~/${path.slice(prefix.length)}` : path;
  return `\u200e${text}`;
}

/** The two-step rail. Labels are the step's own name, never a bare number. */
const rail = (step, label) =>
  el(
    'div',
    { class: 'rail' },
    el('span', { class: 'rail-pill', 'data-done': step === 1 ? 'true' : 'false' }),
    el('span', { class: 'rail-pill', 'data-done': step === 2 ? 'true' : 'false' }),
    el('span', { text: `Step ${step} of 2 · ${label}` }),
  );

const hero = (title, lede) => el('div', { class: 'hero' }, el('h1', { text: title }), el('p', { class: 'lede', text: lede }));

/** A field: label, input, hint, and the host's verdict line. */
function field({ id, label, required, value, hint, status }) {
  const input = el('input', {
    id,
    type: 'text',
    spellcheck: 'false',
    autocomplete: 'off',
    value,
  });
  const node = el(
    'label',
    { class: 'field', for: id },
    el(
      'span',
      { class: 'field-label' },
      el('span', { text: label }),
      required ? el('span', { class: 'required', text: 'required' }) : null,
    ),
    input,
    hint ? el('p', { class: 'hint', html: hint }) : null,
    status,
  );
  return { node, input, status };
}

/** The status line under one field, from the host's PathCheck. */
export function paintCheck(node, check) {
  if (!check) {
    node.textContent = '';
    node.dataset.kind = '';
    return;
  }
  if (check.error) {
    node.textContent = check.error;
    node.dataset.kind = 'err';
    return;
  }
  node.textContent = `${check.path}${check.version ? ' ' + check.version : ''} works`;
  node.dataset.kind = 'ok';
}

/* ------------------------------------------------------------ the guide */

/**
 * Step 1: the runtime. The executable starts empty and has no fallback — that is
 * the whole point of this screen — while the home ships with the harness's own
 * default filled in, because it has one.
 */
export function runtimeStep(state, handlers) {
  const settings = state.settings ?? {};
  const repairing = Boolean(state.dshProblem);
  const status = el('p', { class: 'status', 'aria-live': 'polite' });
  const homeStatus = el('p', { class: 'status', 'aria-live': 'polite' });

  const dsh = field({
    id: 'dsh-path',
    label: 'dsh executable',
    required: true,
    value: settings.dsh ?? '',
    hint: 'Run <code>which dsh</code> in a terminal and paste the result here, or drag the binary onto this window.',
    status,
  });
  const home = field({
    id: 'dsh-home',
    label: 'Harness home',
    value: settings.home ?? '~/.dsh',
    hint:
      state.homeSource === 'DSH_HOME'
        ? `<code>DSH_HOME</code> is set for this session; the shell is driving <code>${state.home}</code>.`
        : 'Profiles, sessions, and credentials live here; it is shared with the dsh CLI.',
    status: homeStatus,
  });

  const submit = el('button', { class: 'primary', type: 'submit', text: 'Continue' });
  const form = el(
    'form',
    {
      class: 'card',
      onSubmit: (event) => {
        event.preventDefault();
        handlers.onContinue({ dsh: dsh.input.value, home: home.input.value, dshStatus: status, homeStatus, submit });
      },
    },
    dsh.node,
    home.node,
    el('div', { class: 'actions' }, el('span', { class: 'spacer' }), submit),
  );

  return el(
    'div',
    { class: 'shift' },
    hero(
      repairing ? 'Fix the dsh path' : 'Point dsh-app at your runtime',
      repairing
        ? state.dshProblem
        : 'This app ships no runtime of its own. It launches the dsh executable you name, and drives one harness home.',
    ),
    rail(1, 'Runtime'),
    form,
  );
}

/**
 * Step 2: the profile, in the same two-line row as the launcher but with a radio
 * where the state light goes — nothing is running yet, so every light would be
 * off and say nothing.
 */
export function profileStep(state, handlers) {
  const selectable = state.profiles.filter((profile) => !profile.startNote);

  if (state.profiles.length === 0) {
    return el(
      'div',
      { class: 'shift' },
      hero('No profiles yet', `Nothing under ${state.home} yet.`),
      rail(2, 'Profile'),
      el(
        'div',
        { class: 'card' },
        el('p', { class: 'hint', html: 'Create one with the dsh CLI: <code>dsh --profile web</code>, or <code>dsh plugin --profile NAME add PACKAGE</code>.' }),
        el(
          'div',
          { class: 'actions' },
          el('button', { class: 'ghost', type: 'button', text: 'Reload list', onClick: handlers.onReload }),
          el('span', { class: 'spacer' }),
          el('button', { class: 'primary', type: 'button', text: 'Continue anyway', onClick: () => handlers.onStart(null) }),
        ),
      ),
    );
  }

  const rows = el(
    'ul',
    { class: 'rows' },
    state.profiles.map((profile) =>
      profileRow(profile, {
        radio: true,
        selected: profile.id === handlers.selection,
        onSelect: profile.startNote ? null : () => handlers.onSelect(profile.id),
        home: state.home,
      }),
    ),
  );

  const chosen = selectable.find((profile) => profile.id === handlers.selection) ?? null;
  return el(
    'div',
    { class: 'shift' },
    hero('Choose a profile', 'Nothing starts on its own — pick what the shell should launch.'),
    rail(2, 'Profile'),
    el('div', { class: 'card', style: 'padding:0' }, rows),
    el(
      'div',
      { class: 'actions' },
      el('button', { class: 'ghost', type: 'button', text: 'Back', onClick: handlers.onBack }),
      el('span', { class: 'spacer' }),
      el('button', {
        class: 'primary',
        type: 'button',
        text: chosen ? `Start ${chosen.label}` : 'Continue',
        disabled: chosen === null,
        onClick: () => handlers.onStart(chosen?.id ?? null),
      }),
    ),
  );
}

/* --------------------------------------------------------- the launcher */

/**
 * One profile, as the shell's flat two-line row: a light, the name, the carrier,
 * and the directory — or the host's reason instead of a directory when the
 * profile cannot be launched at all.
 */
function profileRow(profile, options = {}) {
  const {
    radio = false,
    selected = false,
    onSelect = null,
    primary = null,
    onPrimary = null,
    secondary = null,
    onSecondary = null,
    home = null,
  } = options;
  const clickable = radio ? onSelect !== null : onPrimary !== null;

  const leading = el(radio ? 'span' : 'span', {
    class: radio ? 'radio' : 'dot',
    'data-selected': radio ? String(selected) : null,
    role: radio ? 'radio' : 'img',
    'aria-checked': radio ? String(selected) : null,
    'aria-label': radio ? null : profile.phase,
    title: radio ? null : profile.phase,
  });

  // No carrier tag: when every profile is the same kind the tag is noise, and a
  // profile that cannot be launched says why on its second line anyway.
  const body = [
    leading,
    el('span', { class: 'name', text: profile.label }),
    profile.startNote
      ? el('span', { class: 'why', text: profile.startNote })
      : el('span', { class: 'path', text: shortPath(profile.source, home), title: profile.source }),
  ];

  const hit = clickable
    ? el(
        'button',
        {
          class: 'row-hit',
          type: 'button',
          'aria-label': radio ? `Choose ${profile.label}` : primary.label,
          disabled: radio ? false : Boolean(primary?.disabled),
          onClick: radio ? onSelect : onPrimary,
        },
        body,
      )
    : el('div', { class: 'row-hit', 'aria-disabled': 'true' }, body);

  const tail = [];
  if (secondary && onSecondary) {
    tail.push(
      el('span', { class: 'row-actions' }, el('button', { class: 'ghost', type: 'button', text: secondary, onClick: onSecondary })),
    );
  }
  const tailLines = [];
  if (profile.stderrTail?.length) {
    tailLines.push(
      el(
        'details',
        {},
        el('summary', { text: `stderr (last ${profile.stderrTail.length} lines)` }),
        el('pre', { text: profile.stderrTail.join('\n') }),
      ),
    );
  }

  return el(
    'li',
    { class: 'row', 'data-phase': profile.phase, 'data-clickable': String(clickable), 'data-radio': String(radio) },
    hit,
    ...tail,
    ...tailLines,
  );
}

/** What one row's click should do, in this state. */
export function rowActions(profile, handlers) {
  const running = profile.phase !== 'idle' && profile.phase !== 'exited' && profile.phase !== 'failed';
  const ready = profile.phase === 'ready';
  if (profile.startNote) return { primary: null, secondary: null };
  return {
    primary: {
      label: ready ? `Open ${profile.label}` : `Start ${profile.label}`,
      disabled: profile.phase === 'starting',
      run: () => (ready ? handlers.onOpen(profile.id) : handlers.onStart(profile.id)),
    },
    secondary: running ? { label: 'Stop', run: () => handlers.onStop(profile.id) } : null,
  };
}

export function launcher(state, handlers) {
  const build = (profile) => {
    const actions = rowActions(profile, handlers);
    return profileRow(profile, {
      primary: actions.primary,
      onPrimary: actions.primary ? actions.primary.run : null,
      secondary: actions.secondary?.label ?? null,
      onSecondary: actions.secondary ? actions.secondary.run : null,
      home: state.home,
    });
  };

  const pinned = state.pickedProfile;
  const ordered = [...state.profiles].sort((left, right) => {
    if (left.id === pinned) return -1;
    if (right.id === pinned) return 1;
    return 0;
  });

  const head = el(
    'div',
    { class: 'list-head' },
    el('span', { text: 'Profiles' }),
    el('span', { text: String(state.profiles.length) }),
  );

  if (state.profiles.length === 0) {
    return el(
      'div',
      { class: 'shift' },
      head,
      el(
        'div',
        { class: 'empty' },
        el('p', { html: `No profiles found under <code>${state.home}</code>.` }),
        el('p', { html: 'Create one with the dsh CLI (<code>dsh --profile web</code>, or <code>dsh plugin --profile NAME add PACKAGE</code>), then reload.' }),
        el('div', { class: 'actions' }, el('button', { class: 'ghost', type: 'button', text: 'Reload list', onClick: handlers.onReload })),
      ),
    );
  }

  return el('div', { class: 'shift' }, head, el('ul', { class: 'rows' }, ordered.map(build)));
}

/* ------------------------------------------------------------- the sheet */

export function settingsSheet(state, handlers) {
  const settings = state.settings ?? {};
  const status = el('p', { class: 'status', 'aria-live': 'polite' });
  const homeStatus = el('p', { class: 'status', 'aria-live': 'polite' });
  const dsh = field({
    id: 'sheet-dsh-path',
    label: 'dsh executable',
    required: true,
    value: settings.dsh ?? '',
    status,
  });
  const home = field({
    id: 'sheet-dsh-home',
    label: 'Harness home',
    value: settings.home ?? '~/.dsh',
    status: homeStatus,
  });

  const save = el('button', { class: 'primary', type: 'submit', text: 'Save' });
  const form = el(
    'form',
    {
      class: 'card',
      onSubmit: (event) => {
        event.preventDefault();
        handlers.onSave({ dsh: dsh.input.value, home: home.input.value, dshStatus: status, homeStatus, submit: save });
      },
    },
    dsh.node,
    home.node,
    el(
      'div',
      { class: 'actions' },
      el('button', { class: 'ghost', type: 'button', text: 'Cancel', onClick: handlers.onClose }),
      el('span', { class: 'spacer' }),
      save,
    ),
  );

  const diagnostics = el(
    'dl',
    { class: 'diagnostics' },
    el('dt', { text: 'dsh' }),
    el('dd', { text: `${state.dsh ?? '—'}${state.dshSource ? ` (${state.dshSource})` : ''}` }),
    el('dt', { text: 'home' }),
    el('dd', { text: `${state.home ?? '—'} (${state.homeSource ?? '—'})` }),
    el('dt', { text: 'settings' }),
    el('dd', { text: state.settingsFile ?? '—' }),
    el('dt', { text: 'shell' }),
    el('dd', { text: state.uiVersion ?? '—' }),
  );

  const panel = el(
    'div',
    { class: 'sheet-panel', role: 'dialog', 'aria-modal': 'true', 'aria-label': 'Settings' },
    el(
      'div',
      { class: 'sheet-head' },
      el('h2', { text: 'Settings' }),
      el('button', { class: 'icon', type: 'button', 'aria-label': 'Close settings', title: 'Close settings', onClick: handlers.onClose, html: '&#10005;' }),
    ),
    form,
    el('hr'),
    el('p', { class: 'sheet-note', text: 'Diagnostics' }),
    diagnostics,
    el('p', { class: 'sheet-note', text: 'Runtime installs and updates are managed by your dsh, not by this app.' }),
  );

  return el('div', { class: 'sheet', onClick: (event) => { if (event.target === event.currentTarget) handlers.onClose(); } }, panel);
}
