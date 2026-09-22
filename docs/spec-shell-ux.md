# Spec: shell UX — custom chrome, vibrancy, two-step onboarding, settings sheet

> Status: **proposal — nothing in this document is implemented yet.** This file is the
> review artifact; implementation starts after it is accepted.
>
> Amends [spec-external-dsh.md](spec-external-dsh.md): §4 and §9 Q4 (first-run guidance),
> §12 item 7 (the shell page), and the shape of `shell_state`. The resolution chain of
> §2 and the launch contract of §5 are **unchanged**.
>
> Related: [design-tauri-wrapper.md](design-tauri-wrapper.md) §18.1 (macOS-only product),
> §12.1 (capabilities are not optional), §12.2 (the page has no console).

---

## 0. The asks, mapped

| Ask (verbatim) | Answer | Section |
|---|---|---|
| 点进去优先让用户配置 DSH_HOME 和 dsh 命令位置 | the guide's step 1 is the runtime form, and it is the first thing rendered | §4 |
| dsh 命令不要缺省值 | no default, no placeholder path, no fallback: blank is invalid | §4.1, §8.3 |
| DSH_HOME 缺省可以 | the home field ships prefilled with `~/.dsh`; blank also means the default | §4.1 |
| 缩成一个右上角的设置按钮 | after the guide the form lives in a gear-triggered sheet | §6 |
| 不要系统 titlebar，红绿灯和标题栏都自定义 | the title bar is gone (borderless overlay); the **lights are the system's**, floating over the page's own header strip | §2.1, §2.2 |
| 想要有玻璃质感 | a real `NSVisualEffectView` under the shell window's transparent content area; panels carry alpha | §2.4, §7 |
| dsh 主渲染窗口也不要原生标题栏，也要半透明毛玻璃 | the same borderless treatment, through the marker the runtime documents for its own desktop app: native lights in the band it reserves, translucent sidebar, no title bar | §2.5 |
| 设计语言要和 dsh 统一 | the shell's palette *is* the runtime's: the `--dsw-*` token sheets are mirrored verbatim | §7 |
| 列表平铺两行，用亮/不亮/红色表示状态 | flat two-line rows, one light per row, no cards and no state words | §5 |
| 进去是分步的，两步走，等于引导页 | step 1 = runtime, step 2 = profile; the launcher comes after | §3 |
| 整体更简约，更统一 | one token set, one radius scale, one motion curve, one accent | §5 |

---

## 1. TL;DR

1. **The shell window is borderless with the system's lights.** `titleBarStyle: "Overlay"` +
   `hiddenTitle`: no title bar strip, the page runs to the top edge, and the three native
   buttons float over the page's own 40 px header strip, which reserves their width and is the
   drag region. Corners, shadow, zoom and ⌘W stay AppKit's.
2. **Launch is a two-step guide, not a list.** Step 1 collects the runtime
   (`dsh` executable, required and empty; harness home, prefilled with `~/.dsh`).
   Step 2 picks a profile. The profile *management* list is the surface you land on
   **after** the guide, never before it.
3. **Afterwards the form collapses into a gear** in the top-right corner. The gear opens
   a settings sheet holding the same two fields plus the diagnostics that today live in
   the launcher's "Runtime" disclosure.
4. **The host decides, the page draws.** Every verdict (does this path run, can this home
   be created, is this profile launchable) still comes from Rust. The page gains a view
   state machine, not harness knowledge.
5. **The shell window's content area is glass, in the harness's palette.** `windowEffects`
   puts a real `NSVisualEffectView` behind a transparent webview; the page's surfaces carry
   alpha instead of opaque fills, and every colour in it is one of the runtime's `--dsw-*`
   tokens (§7), so the two windows read as one product.
6. **The dsh window is borderless too, through the runtime's own door.** Its page gets host
   chrome and nothing else: the `data-platform="darwin"` marker the runtime documents for its
   desktop app — which is what reserves the band the native lights sit in and turns its sidebar
   translucent — plus a drag listener for that band. No stylesheet, no token override, no drawn
   chrome, and a test bounds the whole footprint (§2.5).

---

## 2. The windows

Both windows are **borderless with the system's own traffic lights**: no title bar strip
anywhere, the page runs to the top edge, and the three native buttons float over it. AppKit
still owns the corners, the shadow, dragging (through a drag region each window provides) and
resizing, and neither window draws a control of its own — the lights are the system's, which is
the fourth and final answer this document records to "whose chrome is it".

### 2.1 Configuration

`src-tauri/tauri.conf.json` — the shell window; the dsh window is created in Rust (§2.5):

```json
"app": {
  "withGlobalTauri": false,
  "macOSPrivateApi": true,
  "windows": [
    {
      "label": "shell",
      "title": "DeepSeek Harness",
      "width": 640, "height": 580,
      "minWidth": 520, "minHeight": 440,
      "resizable": true, "center": true,
      "titleBarStyle": "Overlay", "hiddenTitle": true,
      "transparent": true,
      "visible": false,
      "windowEffects": { "effects": ["sidebar"], "state": "followsWindowActiveState" }
    }
  ]
}
```

- `decorations` stays at its default, so the window is `Titled`: AppKit keeps the rounded
  corners, the shadow, edge resizing and ⌘W. `titleBarStyle: "Overlay"` + `hiddenTitle` make
  that title bar invisible — no strip, no title text, the content view spans the whole window —
  while the **native** traffic lights stay, floating over the page. That combination is what
  `decorations: false` cannot give: a borderless window has no standard buttons at all, and
  AppKit stops rounding it.
- `transparent` + `macOSPrivateApi` exist for the content area's vibrancy (§2.4), not for the
  window shape. `windowEffects` carries no `radius`: the corners are AppKit's, and a rounded
  effect view inside a titled window would show the desktop through the corner it cut away.
- `core:window:allow-start-dragging` comes back to the shell capability, because the page's
  header strip is now the drag region (the title bar that used to do that job is invisible).
- The height grew by the title bar (540 → 580) so the content keeps the size it was designed
  for.
- `src-tauri/Cargo.toml` still states the `macos-private-api` feature explicitly: this crate
  is built with plain cargo, not through the Tauri CLI, which would otherwise inject it.

### 2.2 The page, below the title bar

The window has no title bar at all, so the page's own 40 px header row is the top of the window:
the three native lights float over its left end, the mark and the name sit beside them, and the
current view's icon actions are on the right. The strip paints nothing — no fill, no separator —
so nothing in the window reads as chrome.

```
● ● ●  › DEEPSEEK HARNESS                ⟳      ⚙   ← .page-head, and the drag region
 Profiles                                        3
 ────────────────────────────────────────────────
 ●  web
    ~/.dsh/profiles/web
```

- The strip carries `data-tauri-drag-region="deep"` — that is what the shell capability's
  `start-dragging` permission is for — and reserves 78 px on the left so nothing is written
  under the lights. Dragging and double-click-to-zoom behave the way they do on any Mac window.
- The hover glyphs on the lights (×, −, ⤢) are the system's, revealed when the pointer is over
  the group. The dots themselves are always visible. "Dots only on hover" is not a macOS window
  behaviour: faking it means hiding the standard buttons through AppKit and re-showing them on
  pointer-enter — a page-side hover loop, to look less native than every other Mac app.

### 2.3 Window lifecycle

- The window is created hidden and the page shows it once it has rendered (`shell_ready`),
  with a 3 s watchdog that shows it anyway — an invisible app with a broken page is the worst
  failure mode this shell has. The page is told the platform in the payload rather than
  guessing at it.
- The single-instance handler shows and focuses the window, so a second launch during the
  hidden start-up window is not a no-op.

### 2.4 The glass layer (the shell window's content area)

The blur is AppKit's, not CSS. Tauri's `windowEffects` ends in
`window_vibrancy::apply_vibrancy`, which creates an `NSVisualEffectView` with
`blendingMode = behindWindow` **below** the webview. Three consequences:

- the webview has to be transparent for the material to show at all, which is what
  `transparent: true` + `macOSPrivateApi: true` buy;
- `state: "followsWindowActiveState"` makes the glass follow window focus, like the system's
  own title bar above it;
- everything above the glass belongs to the page: `html`/`body` paint nothing, panels carry
  alpha (§7), and the settings sheet adds the only second `backdrop-filter`. Text never sits
  on raw glass.

Material: `sidebar`. If the glass has to go (Q3), it is one config block plus one token
block; no component changes.

### 2.5 The dsh window: borderless too, through the runtime's own door

The dsh window is borderless in the same way as the shell window — `titleBarStyle: "Overlay"` +
`hiddenTitle`, `transparent: true`, the same `windowEffects` — and the runtime's traffic
lights float over its page. What makes that work is a single attribute the runtime documents
for exactly this case:

**X1 — the shell writes host chrome and nothing else.** Into the runtime's page it puts:

1. `document.documentElement.dataset.platform = 'darwin'`, the marker the runtime's own
   preload sets in its desktop app ("set only by the desktop preload", read at render time).
   Under it the runtime renders its macOS layout: **a 52 px band that clears the traffic
   lights**, a translucent sidebar over our window's material, and a transparent frame.
   Without it, the native lights would sit on top of the sidebar's own brand row.
2. a drag listener. The band is *documented* as the window's drag region, but it does that
   with `-webkit-app-region: drag` — an Electron/Chromium extension that WKWebView does not
   implement, so in our shell the band is inert and the host has to move the window itself.
   The rule is a screen region (the top 52 px, or anywhere with ⌘ held) plus "not on an
   interactive element"; it never inspects the runtime's markup.

It draws nothing, restyles nothing, reads no DOM. The capability it needs is one file,
`capabilities/harness.json`: `windows: ["harness:*"]`, the loopback origins as `remote.urls`,
and exactly two permissions — `core:window:allow-start-dragging` and
`core:window:allow-toggle-maximize`. **No application command is reachable from that page**:
the runtime's origin is *remote* to Tauri (`plugin_command.is_some() || has_app_acl_manifest ||
!is_local`), so `save_settings`, `start_profile` and the rest stay closed to it, and the host
script is pinned by a test to calling plugin commands only.

**What this does not do.** It does not restyle the runtime: no stylesheet, no token override,
no drawn chrome. The sidebar becomes translucent because that is the runtime's own darwin
design, switched on by its own marker; the centre column stays opaque because that is also its
design. If the harness should look different, that is a dsh theme plugin's job, not this
launcher's.

**Two earlier drafts**, page-drawn lights and a plain window with a visible title bar, are both
withdrawn — the first because the system's lights were wanted, the second because a visible
title bar is what this section exists to remove.

### 2.6 Rejected

| Alternative | Why not |
|---|---|
| Page-drawn traffic lights (`decorations: false`) | the window loses `Titled`, so AppKit stops rounding it, stops drawing its shadow and stops offering title-bar dragging — and the system's own lights were wanted in the end |
| A visible title bar (`titleBarStyle: "Visible"`) | it costs a strip of chrome the page does not need and makes the launcher look like a different application from the dsh window beside it |
| Hiding the standard buttons and revealing them on hover | not a window behaviour macOS offers; it takes AppKit calls and page-side pointer tracking to look *less* native |
| A visible title bar on the dsh window | it is what this design removes; with the marker set, the runtime reserves the band its own lights need, so there is nothing left to put a strip behind |
| Restyling the runtime's page (stylesheets, token overrides, drawn chrome) | X1: its appearance is its authors'; the marker is the one integration point it documents, and it is enough |
| `data-platform="darwin"` in the runtime's page | a real integration point, but it exists to serve a hidden title bar and a translucent window, both of which this design dropped |
| `movableByWindowBackground` as a drag substitute | every mouse-down over a page becomes a candidate window drag, which breaks selection and dragging inside the dsh UI |
| Resizing the window per view | a jumping window is worse than one size that scrolls |


## 3. The view state machine

One derived rule, evaluated on every state payload:

```
configured ? (onboarded ? launcher : guide-step-2) : guide-step-1
```

`configured` keeps its present meaning: *a usable dsh executable resolved* (settings file
or `DSH_APP_DSH`). `onboarded` is new and persisted (§8.1).

| # | `configured` | `onboarded` | View |
|---|---|---|---|
| 1 | false | false | guide step 1 — first run |
| 2 | false | true | guide step 1 in **repair** mode (title says the saved path stopped working; fields are prefilled with the saved values) |
| 3 | true | false | guide step 2 |
| 4 | true | true | launcher |

Transitions inside a session:

- step 1 **Continue** → on success the values are already saved; then
  `onboarded ? launcher : step 2`. (A repair visit therefore ends on the launcher, not on
  a profile chooser the user has already answered once.)
- step 2 **Start/Continue** → `finish_onboarding(profile)` → launcher.
- step 2 **Back** → step 1 with the saved values (no data is lost; saving already happened).
- The gear (launcher) → settings sheet; the sheet never changes the view.

Consequences worth stating:

- Quitting mid-guide is safe: step 1's values are already on disk, and the next launch
  resumes at step 2 (case 3).
- `DSH_APP_DSH=$(command -v dsh) dsh-app` makes the shell configured without a settings
  file, so a first run with that variable set starts at step 2. Step 1 is still reachable
  by clearing the variable or through the sheet, and the diagnostics name the source.
- A corrupt settings file is a problem *and* unconfigured: step 1 with the problem line
  shown. The file is only overwritten by a successful save.

---

## 4. The guide

Both steps share one scaffold: window padding 28 px/32 px, a 460 px content column, a
hero row (22 px rounded-square mark with a `›` glyph + the app name in muted caps), then
the step rail, then the content.

### 4.1 Step 1 — Runtime

```
 [■] DEEPSEEK HARNESS
 Point dsh-app at your runtime            ← h1, 22 px/600
 This app ships no runtime of its own. It launches the dsh
 executable you name, and drives one harness home.        ← 12.5 px, muted

 ●─○   Step 1 of 2 · Runtime               ← step rail

 dsh executable                           REQUIRED
 [                                        ]  ← empty, monospace, no placeholder path
 Run `which dsh` in a terminal and paste the result here, or
 drag the binary onto this window.
 ✓ /etc/profiles/per-user/me/bin/dsh 0.1.6-alpha.2 works   ← host verdict

 Harness home
 [ ~/.dsh                                 ]  ← prefilled default
 Profiles, sessions, and credentials live here; it is shared
 with the dsh CLI.

                                        [ Continue ]        ← primary, right-aligned
```

Field rules (the ask, in code):

1. **`dsh` has no default.** The field starts empty, carries no placeholder path, and a
   blank value is an error — `validate_paths` no longer falls back to the saved value.
   The only way a value appears there is the user having typed or dropped it, or a
   previous save of theirs.
2. **`home` has a default.** The field is prefilled with the literal `~/.dsh` (not the
   resolved absolute path, so a save never freezes this machine's answer into the file,
   §3 of the governing spec). Blank also means the default.
3. If `DSH_HOME` is set for the session, the field still shows the *stored* value and a
   note appears next to it: `DSH_HOME is set for this session; the shell is driving
   <path>.` A session value is never silently written to the machine's file.
4. Validation is host-side and unchanged in kind: exists, is a file, has the executable
   bit, runs `--version`; the home expands `~`, must be absolute, and is created. The
   `--version` string is recorded, never judged.
5. `Continue` calls `save_settings` and only advances when `report.ok`. Per-field verdicts
   render under each field, `aria-live="polite"`.

### 4.2 Step 2 — Profile

```
 [■] DEEPSEEK HARNESS
 Choose a profile
 Nothing starts on its own — pick what the shell should launch.

 ●─●   Step 2 of 2 · Profile

 ◉  web                                            web
    ~/.dsh/profiles/web
 ○  demo                                           task
    the @deepseek-ai/dsh-headless carrier is not implemented yet
                                               ← not selectable, host reason shown

 [ Back ]                             [ Start web ]     ← primary names the side effect
```

- The same two-line row as the launcher (§5), with the radio in place of the state light
  (nothing is running yet, so every light would be off). A profile that cannot be launched is
  shown but not selectable, with the host's reason on its second line.
- Empty home: `No profiles in <home>`, the two CLI hints that exist today
  (`dsh --profile web`, `dsh plugin --profile NAME add PACKAGE`), a `Reload list` button,
  and `Continue anyway` to finish the guide with no profile recorded.
- `Back` returns to step 1 (already saved values, re-validated on the next Continue).
- The primary button is `Start <name>`: finishing the guide selects the profile **and**
  starts it, so the guide ends on the "it works" moment rather than on another button.
  (Open question Q2 if the user prefers a neutral `Continue`.)

### 4.3 What the guide deliberately does not do

No PATH probing, no candidate path list, no install command execution, no profile
creation, no auto-start of anything the user did not click. Those are the governing
spec's invariants (§7) and none of them is weakened here.

---

## 5. The launcher

The guide ends and this is what is left: a flat list, two lines per profile, one light per row.
No cards, no badges, no counters except the header's total. The window's title bar is the
system's, above all of this (§2.2).

```
 ⚠ 2 problems from the last reload                     [Dismiss]   ← only when there is one
 › DEEPSEEK HARNESS                               ⟳   ⚙          ← .page-head
 Profiles                                                   3
 ──────────────────────────────────────────────────────────────
 ●  web                                             web       ← lit: ready
    ~/.dsh/profiles/web
 ──────────────────────────────────────────────────────────────
 ○  demo                                            task      ← off: idle, not launchable
    the @deepseek-ai/dsh-headless carrier is not implemented yet
 ──────────────────────────────────────────────────────────────
 ●  au-web                                          web       ← red: failed
    ~/.dsh/profiles/au-web
    ↳ stderr (last 4 lines)                  ← <details>, only after a failure
```

- **The light is the state.** Nothing else says it:

  | Light | Phase | Meaning |
  |---|---|---|
  | hollow ring, `--faint` | `idle` | discovered, not running |
  | hollow ring, pulsing | `starting` | booting |
  | filled `--accent` | `ready` | serving; a click opens it |
  | filled `--err` | `failed` / `exited` | it died, or never came up |

  No state word is printed — but the light is not decoration either: the dot is
  `role="img"` with `aria-label`/`title` carrying the phase, so a screen reader and a
  hovering pointer both get it. Red is the only colour that also gets text: a failed row
  prints the host's one-line reason under its path.
- **An idle row is quieter than a live one**: the name is `--muted` while nothing is running
  and `--fg` while something is — the same information as the light, one level softer.
- **Two lines**: the name first, with the profile's carrier as a muted tag at the right edge;
  the directory second, trimmed from the left so the last segment stays visible. A profile
  that cannot be launched prints the host's reason in place of the path and is not clickable.
- **The row is the button.** One click does the obvious thing — `Start` when idle or dead,
  `Open` when ready — and a press on a starting row does nothing. The secondary action appears
  in the right margin on hover or keyboard focus (`Stop` while it runs), so the resting list is
  nothing but names, paths and lights. Both are real `<button>`s with labels: revealing the
  secondary never removes the primary.
- `stderrTail` stays a `<details>` under its row, and only exists after a failure.
- Rows sit **directly on the glass** — no panel behind them. Hairlines separate rows, hover and
  focus paint a translucent fill, and the guide's chosen profile sorts first. The only surfaces
  that keep their own fill are the guide's fields and the settings sheet, because form controls
  need that contrast (§7).
- The banner is one line and only appears when the host reported problems (an unreadable
  profile directory, a failed launch): the count, and a `Dismiss` that clears the list
  (`dismiss_problems`).
- Empty home: one line of prose, the two CLI hints that exist today, and a `Reload list`
  button — no card around them.
- The "Runtime" `<details>` block moves into the settings sheet. The launcher keeps no
  diagnostics of its own.

---

## 6. The settings sheet

The gear (and `⌘,` / `Ctrl+,`) opens a centred sheet over the launcher: a scrim of
`rgb(0 0 0 / 22%)` that is also the app's only second blur
(`backdrop-filter: blur(30px) saturate(180%)`), a 460 px panel at `--panel` alpha,
radius 10 px, fade + 8 px translate, 160 ms.

```
 Settings                                                    ✕
 Runtime
   dsh executable   [ /etc/profiles/per-user/me/bin/dsh ]
   Harness home     [ ~/.dsh                            ]
   ✓ /etc/profiles/per-user/me/bin/dsh 0.1.6-alpha.2 works
                                        [ Cancel ] [ Save ]
 ───────────────────────────────────────────────────────────
 Diagnostics
   dsh        /etc/profiles/per-user/me/bin/dsh   (settings)
   home       /Users/me/.dsh                      (default)
   settings   /Users/me/.config/dsh-app/config.toml
   shell      a1b2c3d4e5f6
   Runtime installs and updates are managed by your dsh, not by this app.
```

- Fields are populated **when the sheet opens**, never on every status event: the page
  stops fighting the user's typing (the current `configTouched` flag exists only because
  the form was re-rendered on each event).
- `Save` calls the same `save_settings`; failures keep the sheet open with per-field
  errors; success closes it and the launcher re-renders from the new state.
- Diagnostics add a `homeSource` line (`DSH_HOME` / `settings` / `default`) mirroring
  `dshSource`, so "which home am I actually driving" is answerable in one line.
- Keyboard: `Esc` closes, focus moves to the first field on open and back to the gear on
  close, the sheet is `role="dialog" aria-modal="true"`, and Tab stays inside it.

---

## 7. Visual language

One token block, two schemes, no per-component colours:

The shell's tokens **are** the runtime's. `ui/shell.css` mirrors the `--dsw-*` sheets shipped
by `@deepseek-ai/dsh-client-ui-theme` — primitives first, then the alias layer, light and dark —
and only maps its own semantic names onto them, so a later diff against that sheet stays
mechanical. The values that matter:

| Alias (`--dsw-`) | Light | Dark | Shell name |
|---|---|---|---|
| `alias-bg-base` | `bluish-00` `#fff` | `bluish-950` `#151517` | `--bg` (only the fallback: the window is transparent over the material) |
| `alias-bg-layer-1` | `#fff` | `bluish-875` `#232324` | `--panel`, `--head` |
| `alias-bg-module-platform` | `bluish-60` `#f5f6f7` | `bluish-800` `#353638` | `--inset` |
| `alias-border-l1` / `l3` | `#0000000a` / `#0000001f` | `#ffffff0f` / `#ffffff29` | `--line` / `--line-strong` |
| `alias-label-primary` | `bluish-1000` `#0f1115` | `bluish-50` `#f9fafb` | `--fg` |
| `alias-label-secondary` / `tertiary` | `bluish-700` / `600` | `bluish-300` / `400` | `--muted` / `--faint` |
| `alias-interactive-bg-hover` | `#2631480f` | `#ffffff14` | `--hover` |
| `alias-link` | `deepseek-500` `#4176e6` | `deepseek-400` `#679efe` | `--accent` (links, focus, the starting state) |
| `alias-button-primary-fill` | `brand-primary` = `bluish-1000` | `bluish-50` | primary buttons — monochrome, as the runtime's are |
| `alias-state-success-primary` | `green-500` `#22c55e` | same | ready |
| `alias-state-warn-primary` | `amber-500` `#f59e0b` | same | starting |
| `alias-state-error-primary` | `red-600` `#ec1313` | `red-400` `#f25a5a` | failed / exited |

- Radii: panel 12 px, control 8 px, pills 999 px — the runtime's own scale. The window's corners
  belong to AppKit, so the page draws none.
- Glass discipline: nothing is painted on the raw desktop — the material is always in front
  of it. The list's text sits directly on the material (the way a macOS sidebar carries its
  rows); form surfaces — the guide's fields and the settings sheet — add their own ≥ 74 % fill
  because controls need it. The sheet is the only second `backdrop-filter`
  (`blur(30px) saturate(180%)`). Contrast is checked in both schemes over a light and a dark
  desktop, not only over the default wallpaper.
- The row's state light is the one place colour is allowed to mean something: `--accent` =
  live, `--err` = broken, `--faint` = idle. No green: this design has one accent, and a second
  "positive" colour would read as a second accent. It is a *state* dot inside the list, not a
  window control — those are the system's.
- Fallback: with transparency off the token block collapses to flat fills
  (`--bg: #f5f5f7` / `#1c1c1e`, panels `#ffffff` / `#242427`) and no component changes.
- Type: the runtime's `--dsw-font-family` verbatim (it carries the CJK fallbacks); 20/600 hero,
  13/400 body, 12/400 secondary, 11 muted caps with `.06em` tracking, `ui-monospace` 11.5 for
  every path and field value.
- Spacing on a 4 px scale; the window's own padding is 28 px top / 32 px sides.
- Motion: `cubic-bezier(.2,.8,.2,1)`, 140 ms for hover/focus, 200 ms for view and sheet
  transitions; `@media (prefers-reduced-motion: reduce)` turns all of it off.
- Icons are inline SVG (gear, reload, the mark). No icon font, no image file, no web font —
  the page must stay a handful of text files that the build script can hash (§9).
- Accessibility: icon buttons have `aria-label` + `title`, ≥24 px hit targets, focus-visible
  rings only (`0 0 0 3px color-mix(in srgb, var(--accent) 25%, transparent)`), status and
  problem lines are live regions, and the whole guide is operable with Tab/Enter alone.

---

## 8. Data and IPC contract

### 8.1 `config.toml`

```toml
schema = 1
dsh = "/etc/profiles/per-user/me/bin/dsh"
home = "~/.dsh"
onboarded = true          # new: written by finish_onboarding
profile = "web"           # new: the profile chosen in the guide
```

Both new fields are optional with defaults (`false`, absent), so files written by the
current build keep loading, and an older build reading a new file ignores them rather
than refusing it. `schema` stays **1**: it guards semantic breaks, and adding two
optional keys is not one. Writing stays atomic (temp + rename), and `save()` becomes
load-modify-write so editing the runtime in the sheet never clears the onboarding fields.

### 8.2 `shell_state`

Added to the payload: `platform` (`macos`|…), `onboarded`, `pickedProfile`,
`homeSource`. Everything else keeps its key and meaning — `configured`, `dsh`,
`dshSource`, `dshProblem`, `home`, `settingsFile`, `settings{dsh,home,problem}`,
`profiles[]`, `problems[]`, `uiVersion`.

`dshProblem` keeps its meaning: **only** present when a value the user named is unusable.
"Nothing configured" is still a state and still carries no problem.

### 8.3 Commands

| Command | Change |
|---|---|
| `shell_state` | payload additions above; **one** `settings::load` per call instead of two |
| `save_settings(dsh, home)` | blank `dsh` is now an error (no saved-value fallback); on success the state re-resolves the home and re-discovers profiles |
| `finish_onboarding(profile)` | **new** — writes `onboarded = true` + `profile` (unknown name → recorded as none) |
| `shell_ready()` | **new** — shows + focuses the window once, and stands the watchdog down |
| `dismiss_problems()` | **new** — clears the host problem list behind the banner's Dismiss |
| `start_profile`, `stop_profile`, `open_harness`, `reload_profiles`, `ui_probe` | unchanged |

The page sets `document.documentElement.dataset.view` to `onboarding-1`,
`onboarding-2`, or `launcher`, and the injected page-probe reports it, so a headless run
can assert which surface the guide produced.

---

## 9. Logic fixes that ride along

These are the "optimize the logic" half of the request; none of them is cosmetic.

1. **`AppConfig.home` is re-resolved after a save.** Today the home is resolved once in
   `run()` and never again, so saving a new home leaves the running shell driving the old
   one until restart — and the new guide makes that a *first-run* path (step 1 → step 2
   lists the profiles of the home the user just typed). `save_settings` must rebuild the
   config and re-discover profiles, exactly as `reload_profiles` does.
2. **One settings read per snapshot.** `snapshot()` currently loads the file twice (once
   inside `dsh_report`, once directly) plus two environment lookups, on *every* status
   event. Load once, derive both.
3. **Blank `dsh` stops meaning "keep the saved one".** That fallback is what a default
   looks like from the user's side, and the ask is explicitly no default. `home` keeps
   the blank-means-default rule, which is the asymmetry the user asked for.
4. **`problems` becomes dismissible.** The banner is only useful if it can go away; the
   host list is cleared by `dismiss_problems` and refreshed by `reload_profiles`.
5. **`build.rs` hashes every file under `ui/`** instead of a hard-coded five-file list.
   The page is about to gain files; a list that must be kept in sync by hand silently
   stops identifying the build the moment someone forgets it.
6. **The IPC scan test reads every `ui/*.js`** for the same reason: it is the only thing
   standing between a renamed command and a dead button (§13.5 of the governing spec).
7. **The page renders the sheet's inputs on open, not on every event** — the
   `configTouched` workaround disappears with it.
8. **`withGlobalTauri: false`.** Nothing in this app uses `window.__TAURI__` (the page
   deliberately uses `__TAURI_INTERNALS__`, and the harness runtime never references
   Tauri at all — verified), so the whole injected IIFE bundle is dead weight on every
   page load, in every window.
9. **The single-instance handler shows the window** before focusing it, so a second
   launch during the hidden-start window is not a no-op.
10. **The harness window is created by the shell, not by the config file.** It lives in a
    `harness_window` module that owns its chrome (titlebar style, material, the one host
    script), its navigation guard, and its label. That script is the only place in this crate
    that writes into a page it does not own, so a test bounds it (§11): the marker, the drag
    listener, no DOM drawing, and plugin commands only.
11. **App commands check who is calling.** The ACL does not gate app commands, so
    `save_settings`, `finish_onboarding` and `dismiss_problems` take
    `window: tauri::Window` and refuse anything but `shell`. The dsh page cannot reach them
    anyway — it is a remote origin, and nothing of ours runs in it — so this is the second
    lock, for the day a capability is widened.
12. **The window lifecycle is shared by both windows.** Hiding until the first paint, the
    watchdog, and "show + focus on a second launch" are properties of the app, not of one
    window.

---

## 10. Files

| File | Change |
|---|---|
| `src-tauri/tauri.conf.json` | window chrome + size, `visible: false`, `withGlobalTauri: false`, `macOSPrivateApi` |
| `src-tauri/Cargo.toml` | `macos-private-api` feature |
| `src-tauri/capabilities/shell.json` | `+ core:window:allow-start-dragging`, because the page's header strip is the drag region of a window whose title bar is invisible |
| `src-tauri/capabilities/harness.json` | **new** — `windows: ["harness:*"]`, the loopback origins, and two window controls. No application command is reachable from that page |
| `src-tauri/src/settings.rs` | `onboarded` + `profile`; load-modify-write `save`; `save_onboarding`; tests |
| `src-tauri/src/config.rs` | `homeSource`; no change to `resolve_dsh` |
| `src-tauri/src/lib.rs` | payload additions; `shell_ready`, `finish_onboarding`, `dismiss_problems`; home re-resolution; watchdog; blank-dsh rule; caller checks; tests |
| `src-tauri/src/harness_window.rs` | **new** — harness window creation: borderless chrome, vibrancy, navigation guard, the single `initialization_script` |
| `ui/harness-host.js` | **new** — the host script injected into the runtime's page: the darwin marker and window dragging. X1's entire footprint |
| `src-tauri/build.rs` | hash every file under `ui/` |
| `ui/index.html` | page skeleton: header row with the mark and the icon actions, view host, banner, sheet |
| `ui/shell.css` | token set + components (rewritten) |
| `ui/views.js` | **new** — guide steps 1/2, launcher, settings sheet |
| `ui/shell-page.js` | controller: state machine, IPC, events (all `invoke` calls stay reachable by the scan test) |
| `ui/tauri.js` | unchanged |
| `docs/spec-external-dsh.md` | §4/§9 Q4 updated to point here; §12 item 7 marked done |

---

## 11. Verification

Build and unit tests (from the devshell, per AGENTS.md):

```sh
# The crate lives in `src-tauri`, so the manifest path is not optional: a bare
# `cargo build` from the repository root finds no manifest (the AGENTS.md line
# that says otherwise is stale).
nix develop --command cargo build --manifest-path src-tauri/Cargo.toml
nix develop --command cargo test  --manifest-path src-tauri/Cargo.toml
```

New/updated tests:

- blank `dsh` is rejected and does not fall back to the saved value (replaces the test
  that pinned the old fallback);
- `save`/load round-trips `onboarded` + `profile`, and a runtime-only save preserves them;
- `save_settings` re-resolves the home and re-discovers profiles from it;
- `finish_onboarding` sets the flag and clamps an unknown profile;
- the IPC scan covers every `ui/*.js`, and the page's `invoke` names all exist;
- `harness_window.rs` injects exactly one script and never evals; that script sets the
  documented marker, drags the window, and does nothing else — no `document.body`, no
  `getComputedStyle`, no `styleSheets`, no `createElement("style")`, no `.append(`, no
  `innerHTML`, and plugin commands only (`the_harness_window_writes_only_host_chrome`).

Scripted GUI checks (the shell has no terminal — §13.6 of the governing spec — so every
view has to be reachable from a script):

```sh
# 1. first run: step 1, empty dsh field, prefilled home
XDG_CONFIG_HOME=$(mktemp -d) DSH_APP_DEBUG_STATE=1 ./target/debug/dsh-app
#    expect: page-probe {"view":"onboarding-1", …}, no page error, and
#            "no dsh is configured" on stderr

# 2. resume: a configured runtime, guide not finished → step 2
cfg=$(mktemp -d); mkdir -p "$cfg/dsh-app"
printf 'schema = 1\ndsh = "%s"\n' "$(command -v dsh)" > "$cfg/dsh-app/config.toml"
XDG_CONFIG_HOME=$cfg DSH_APP_DEBUG_STATE=1 ./target/debug/dsh-app
#    expect: page-probe {"view":"onboarding-2", …}

# 3. finished: onboarded = true → launcher, and the old selftest still starts a profile
printf 'onboarded = true\n' >> "$cfg/dsh-app/config.toml"
XDG_CONFIG_HOME=$cfg DSH_APP_DEBUG_STATE=1 DSH_APP_SELFTEST=web ./target/debug/dsh-app
#    expect: page-probe {"view":"launcher", "selftest":"invoked", "phase":"ready", …}
```

The harness window's check is a *bounded footprint*, and it is a source scan rather than a
runtime probe: the page is a remote origin, so the shell may not run its own commands in it.
One test asserts the shape of the whole footprint — one script, no `eval`, the marker present,
the drag listener present, and none of the calls that would mean drawing or reading the
runtime's UI. If a later change needs one of those, the test is where the argument has to
happen.

The end-to-end check that remains is the one that matters: with a real dsh and an isolated
home, `DSH_APP_DEBUG_STATE=1 DSH_APP_SELFTEST=web` reaches `phase: ready` and prints

    [dsh-app] starting profile web with … (settings)
    [dsh:web] dsh web: http://127.0.0.1:63299/?token=…
    [dsh-app] harness window harness:web opened

i.e. the runtime came up and its window opened — with nothing of ours in it.

Manual checklist (macOS, both schemes):

- shell window: no title bar strip; the three native lights float over the page's header strip
  without covering the mark; drag by that strip, double-click it to zoom, resize from every
  edge, min size holds; the lights hover, dim when unfocused, and close/minimize/zoom;
- glass: move the shell window over a light and a dark background — the blur follows the
  window, panels stay readable, and window focus dims the material and the lights together;
- harness window: no title bar; the system's lights float in the band the runtime reserves for
  them, clear of its sidebar's own brand row; the sidebar is translucent over the material and
  the centre column stays opaque; dragging works from that band and from ⌘-drag anywhere;
  double-click zooms; native corners, shadow and edge resizing all survive;
- palette: the two windows side by side read as one product — same neutrals, same hairlines,
  same monochrome primary button, same state colours;
- launch: the window does not flash white, and it still appears if the page is broken
  (break the page deliberately once, confirm the watchdog);
- guide: it survives a quit between the steps, and a repaired path lands on the launcher;
- sheet: `⌘,` opens it, `Esc` closes it, focus returns to the gear, Tab stays inside;
- dsh field: drag a binary onto step 1 and confirm the field fills and validates;
- both schemes, `prefers-reduced-motion` (no transforms), and the flat fallback
  (`transparent` + `windowEffects` removed) — the shell window still renders, flat;
- harness window: the Inspector shows the runtime's own DOM and nothing of ours — no injected
  node, no marker attribute, no stylesheet.

---

## 12. Non-goals

- No runtime, no install, no update, no probing, no candidate path table (governing spec §7).
- No auto-start of anything the user did not click: the launcher never boots a profile on
  its own, and the guide starts one only from the button that names it.
- **X1: host chrome, never a UI patch.** Into the runtime's page the shell writes exactly two
  things: the `data-platform` marker the runtime documents for a macOS host, and a drag
  listener for the band that marker reserves — plus two window controls in its capability. Its
  DOM, stylesheets and tokens are untouched; if the harness itself should look different, that
  is a dsh theme plugin's job, not this launcher's.
- No private palette: the shell's colours and radii are the runtime's `--dsw-*` tokens (§7),
  mirrored rather than invented.
- No new dependency (the dialog plugin is deliberately not used — see Q4).
- No Tauri `unstable` feature and no second webview: the window is the system's.
- No change to the launch contract, the resolution chain, or the env whitelist.

---

## 13. Open questions

| # | Question | Recommendation |
|---|---|---|
| Q1 | Does "no default for dsh" also mean removing `DSH_APP_DSH` (the session-level override)? | **Keep it.** It is not a default but an explicit per-session value; it is what makes `DSH_APP_SELFTEST` and isolated runs possible, and the sheet names it as the source. Removing it would amend §2 of the governing spec. |
| Q2 | Step 2's primary: start the profile, or only record it? | **Start it** — the button says so, and the guide should end by proving it works. |
| Q3 | Glass or flat for the **shell** window's content area? (`transparent` + `windowEffects` — §2.4) | **Glass.** If visual QA shows artefacts, dropping `transparent` + `macOSPrivateApi` + `windowEffects` and collapsing the alpha tokens is a two-block fallback with no component changes (§7). |
| Q4 | A native file picker (`tauri-plugin-dialog`)? | **Not now.** Paste + drag-and-drop cover it with no new dependency; the crate is in the local cargo cache, so it stays a cheap follow-up if pasting annoys people. The Nix build runs `cargo build --release --offline`, so a new crate also means a lockfile update that has to resolve offline. |
| Q5 | Persist the chosen profile and pin it in the launcher? | **Yes** — it is one optional field, and it makes the second launch feel like it remembered. |

Q6 (the chrome of both windows) took four passes during review. What is implemented is the
last: **both windows are borderless**, both keep the system's native lights, and the dsh window
gets there through the marker the runtime documents for its own desktop app. Page-drawn lights,
a visible title bar on the shell, and a visible title bar on the dsh window all exist only in
this document's history — §2, §2.5 and §2.6 say what each alternative cost.

Q1–Q5 stand as recommended. Q1 and Q4 are the only ones whose answer would ripple if it changed,
and both are cheap to change before the code that depends on them lands.

