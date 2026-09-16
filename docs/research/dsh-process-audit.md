# Running the dsh host as a supervised OS process, and packaging it — audit

Audited tree: `/Users/rqc/r/github.com/deepseek-ai/deepseek-harness` (read-only). All paths below are relative to that root unless absolute. `node_modules` was ignored except to confirm native prebuilds.

## Bottom line

- **Two runtime contracts already exist in-tree**, and both are production-shaped:
  1. **Pipe mode** (`apps/desktop-host`, Electron's child): `node <profile>/node_modules/@deepseek-ai/dsh-desktop-host/lib/index.js <projectDir>` with **no listening socket**, 4 stdio pipes (fd 3 request, fd 4 response, **fd 5 = Node IPC**) carrying framed bytes + lifecycle. Ready is signalled as an IPC message `{type:'ready',protocolVersion:3,dshVersion}`. This is the best fit for a Rust/Tauri supervisor — **except** that the entry hard-requires `process.send` to exist (`index.ts:378`), so a non-Node parent must provide `NODE_CHANNEL_FD` with Node's newline-delimited JSON IPC framing, or the entry must be patched.
  2. **Loopback mode** (`dsh web`): binds `127.0.0.1:3080` by default, prints `dsh web: http://127.0.0.1:<port>/?token=<32-byte base64url>` on **stdout after the whole tree has settled** (`packages/bundle/web-app/src/index.ts:252-279`), handles SIGTERM→exit 0 / SIGINT→exit 130 with a 5 s grace (`apps/cli/src/profile-boot.ts:309-310`, `apps/cli/src/process-shutdown.ts:4`), and authenticates `/api` + the WebSocket mux with a Host/Origin fence plus a signed `HttpOnly; SameSite=Strict` cookie minted from that process token.
- **Nothing about boot needs a TTY, network downloads, or system tooling.** `engines` is a single range: `^22.19.0 || >=24.0.0` (`package.json:8-10`). Boot requires the Node runtime plus three native addons (`node-pty`, `koffi`, `node-addon-require-builtin` + its OS/CPU optional package) — all shipped as prebuilds, none compiled with node-gyp.
- **All mutable state is under `$DSH_HOME` (default `~/.dsh`)**; nothing is written into the install directory at startup. A read-only, root-owned prefix (Homebrew/nix) works for `dsh web` as long as `$DSH_HOME` is the invoking user's and writable.
- **Three self-containment recipes already exist**: `pnpm deploy --legacy --prod` closure (`scripts/build-exe-for-python-sdk.ts`), a single-file `@yao-pkg/pkg --sea` executable with `-rg`/`-spawn-helper` sidecars (same script, for the Python wheel), and the Electron "signed seed" (offline pnpm store shards + local tarballs that a bundled Node+pnpm install into `$DSH_HOME` on first launch — `apps/desktop/scripts/prepare-seed.ts`, `apps/desktop/src/project-manager.ts`).
- **Biggest deployment traps**: (a) the `web` profile is `patchReload: 'live'` and mounts `@deepseek-ai/cordis-plugin-hmr`, which **throws unless `--expose-internals` or the `node-addon-require-builtin` addon is present** (`vendor/hmr/src/index.ts:121`); (b) `dsh plugin …` shells out to a **PATH `pnpm`** (`apps/cli/src/plugin.ts:134`) — there is no read-only switch, plugin management simply fails with exit 127 when pnpm is absent; (c) a running profile writes symlinks into `$DSH_HOME/profiles/node_modules` pointing at the **absolute install path**, so an in-place Homebrew upgrade relies on the fallback's self-healing (`packages/boot/app-boot/src/profile.ts:508-528, 552-565`).

---

## 1. Process entry points

### 1.1 `apps/desktop-host/src/index.ts` — the pipe-mode host (recommended model)

- Module doc (`index.ts:1-5`): "Electron child-process entry: boots the desktop project without a listening socket and carries API plus validated Web assets over framed byte pipes."
- **argv contract** (`index.ts:376-384`): `argv[2]` = project directory (required); `argv[3]` = optional `--allow-linked-profile` (dev only; workspace-linked bundle packages allowed outside the project). Any other argv[3] is a fatal error. If `process.send === undefined` it throws immediately — **a Node IPC channel is mandatory**.
- **Descriptors** (`apps/desktop-host/src/wire.ts:6-13`): `DESKTOP_REQUEST_PIPE_FD = 3` (read), `DESKTOP_RESPONSE_PIPE_FD = 4` (write); `apps/desktop/src/host-protocol.ts:13` adds `DESKTOP_CONTROL_IPC_FD = 5` for Node's IPC channel. Chunk cap 64 KiB (`wire.ts:13`), control payload cap 1 MiB (`wire.ts:17`).
- **Composition** (`index.ts:278-307`): `loadLayeredEnv('dsh desktop')` → `loadProfileDirectory` over the profile project → `boot(...)` with patch layers = profile bundles' patches + profile patch + `config/desktop.cordis.patch.yml` → then requires services `connection`, `clientModules`, `typertGateway`. Handlers: `/api` (Connection's shared fetch), everything else = SPA assets (`index.ts:305-307, 185-221`), plus a private `POST /.dsh/remote-stream` NDJSON stream endpoint (`index.ts:97, 223-265`).
- **Startup emission** (`index.ts:407-411`): over IPC only — `{ type: 'ready', protocolVersion: 3, dshVersion }`. Fatal errors are `{ type: 'fatal', message }` (also via IPC; stderr only when there is no channel, `index.ts:581-586`). Nothing is guaranteed on stdout except whatever the downstream plugins print; Electron pipes the child's stdout to its own (`apps/desktop/src/host-process.ts:125`).
- **Shutdown** (`index.ts:425-449, 567-577`): IPC `{type:'shutdown'}`, parent disconnect, `SIGTERM`, and `SIGINT` all converge on one stop routine: pause request pipe, abort in-flight requests, `fiber.dispose()`, drain the response pipe, `process.disconnect()`, exit with the recorded code. `to kill -9` is the parent's fallback after 10 s then 5 s (`host-process.ts:200-220`).
- **Parent spawn recipe to copy** (`host-process.ts:99-119`): `spawn(node, [entry, projectDir], { cwd: projectDir, stdio: ['ignore','pipe','pipe','pipe','pipe','ipc'], env: process.env − NODE_OPTIONS − /^DSH_DESKTOP_/ − /^(npm|pnpm|corepack)_/i })`. Note it strips `NODE_OPTIONS` deliberately (test asserts this: `apps/desktop/tests/host-process.spec.ts:99-110`).
- It also validates self-containment: every profile bundle layer must resolve inside the project dir unless `--allow-linked-profile` (`index.ts:152-159`).

### 1.2 `apps/cli` — `dsh`

- Source entry `apps/cli/src/bin.ts`: `#!/usr/bin/env node`, reads `../package.json` for `--version` (`bin.ts:17-22`), dispatches three modes (`bin.ts:31-61`): `profile` (boot a profile), `plugin` (pnpm forwarder), `dump-config`. Built artifact is a single ESM `apps/cli/lib/bin.js` (≈9 KB, plus a few tsdown chunks) with bare imports resolved from the installation's `node_modules`; published `bin.dsh = lib/bin.js` (`apps/cli/package.json:12-14`).
- **arg grammar** (`apps/cli/src/args.ts`): launcher flags come first and end at the first unknown token (`args.ts:137-161`); everything after is the app's own argv. `web` is a hardcoded alias for `--profile web` (`args.ts:175-188`). `--profile desktop` is rejected — "managed exclusively by the Electron application" (`args.ts:68-72`).
- The URL/ready behaviour, the HMR watch-only fallback, the home patch layer, and signal handling all live in `apps/cli/src/profile-boot.ts` (see §1.4). Launch-time readiness is also exposed in-process as an `AppReady` service committed only after boot + watcher setup (`profile-boot.ts:44-65, 386-390`) — useful if a supervisor ever embeds rather than spawns.
- Failure policy: `installFailLoud` turns a late unhandled plugin-init rejection into one stderr diagnostic and `exit(1)` after a 2 s release grace (`packages/boot/app-boot/src/index.ts:608-621`).

### 1.3 Other launchable host/server entries

| Entry | Transport | Notes |
|---|---|---|
| `apps/desktop-host` | fd 3/4 byte pipes + fd 5 IPC | §1.1; no port. |
| `dsh web` (`--profile web`) | `node:http` on loopback | §1.4. |
| `dsh --profile sdk` | stdio JSON-RPC | `@deepseek-ai/dsh-sdk-jsonrpc-server` — "Stdio JSON-RPC server plugin for out-of-process DeepSeek Harness SDK clients" (`packages/sdk/server/package.json:2-3`), reads plain stdin (`packages/sdk/server/src/index.ts:53`). |
| `dsh --profile acp` | stdio JSON-RPC (ACP) | `@deepseek-ai/dsh-acp` — "Automation-only Agent Client Protocol server" (`packages/acp/acp/package.json:2-3`). |
| `@deepseek-ai/dsh-web-app` (plugin) | HTTP routes on `webServer` | Only when an HTTP carrier is composed (`packages/host/webserver`). Also used by the Electron shell through `clientModules.fetchBundle` directly, no socket. |
| `packages/webhook/webhook-github` | HTTP exact route | Requires a `webServer`; disabled in desktop. |

There is exactly one HTTP server implementation: `packages/host/webserver/src/index.ts` (node:http, optional gzip, one fallback seat, exact-path upgrade routes).

### 1.4 `dsh web` specifics

**Host/port config.** The `webserver` row in `packages/bundle/web-app/cordis.patch.yml:135-143`:

```yaml
- id: webserver
  name: '@deepseek-ai/dsh-host-webserver'
  inject: [webStartup]
  config:
    host: !!js ctx.webStartup.host ?? '127.0.0.1'
    port: !!js ctx.webStartup.port ?? 3080
```

Flags come from the app's own command (`packages/bundle/web-app/src/startup.ts:46-86`): `--host <host>`, `--port <port>` (`0` = OS-assigned), `--no-open`, `--trusted-host <authority...>`. The webserver schema allows only `'127.0.0.1' | '0.0.0.0'` (`packages/host/webserver/src/index.ts:126`), and **`--host 0.0.0.0` is refused outright** by the app with a safety error (`startup.ts:74-76`), so the shipped web profile is loopback-only. `--port` must be numeric (`startup.ts:77-79`). A profile patch layer can pin `host`/`port` by overriding the same row.

**URL token on stdout.** `packages/bundle/web-app/src/index.ts:252-279`: after `loader.await()` settles, it prints

```
dsh web: http://127.0.0.1:3080/?token=<base64url>      (LAN: …)   ← LAN only when bound to 0.0.0.0
```

The token is a fresh 32-byte random value per process (`packages/client/connection/src/browser-auth.ts:52-58`), regenerated on every start. With `openBrowser` true (default; suppressed under SSH via `launchedThroughSsh`, `index.ts:229`) it also prints `dsh web: opening the default browser; pass --no-open to disable` and spawns `process.execPath` running the `open` module (`index.ts:86-113, 173-182`).

**Shutdown.** `apps/cli/src/profile-boot.ts:300-310`: `SIGTERM → interrupt(0)`, `SIGINT → interrupt(130)`; `createProcessShutdown` disposes the whole fiber tree then exits, force-exiting after `PROCESS_SHUTDOWN_TIMEOUT_MS = 5_000`; a second signal forces immediately (`apps/cli/src/process-shutdown.ts:52-76`). The webserver teardown calls `closeAllConnections()` **and** destroys tracked upgraded (WebSocket) sockets (`packages/host/webserver/src/index.ts:304-314`).

**Authentication / CSRF / origin.**

- Index/HTML: `authorizeIndex` (`browser-auth.ts:240-282`) — `GET /?token=…` with exactly one token and a matching Host mints a cookie and 303-redirects to clean `/`; requests without a valid cookie get `401 "dsh web authentication required; reopen the URL printed by dsh web."`. The cookie is `v1.<payload>.<HMAC-SHA256>` and is **bound to the request authority**; attributes `Max-Age` (default 30 days, `connection.cookieMaxAgeDays`), `Path=/`, `HttpOnly`, `SameSite=Strict` (`browser-auth.ts:120-123, 289-302`). The signing secret persists in the credentials store (`AUTH_RECORD_KEY`, `browser-auth.ts:12, 161-178`), so cookies survive restarts.
- `/api` (unary RPC) and `/api/remote.mux` (WebSocket upgrade): Host/Origin fence + cookie check (`packages/client/connection/src/rpc-host.ts:96-100`; `packages/api/gateway/src/index.ts:206-224`). The fence refuses non-loopback, non-`trustedHosts` Hosts, `Sec-Fetch-Site: cross-site`, and mismatched/non-canonical `Origin` (`packages/client/connection/src/api-request-trust.ts:91-118`). No CORS headers are emitted anywhere, so cross-origin reads fail by default.
- **Not authenticated**: `/plugins/<id>/client.js` bundles and `/plugins/events` (SSE) are plain webserver routes with no fence/cookie check (`packages/client/modules/src/index.ts:570-575`; `packages/client/hmr/src/index.ts:175-200`), and all non-index static assets are public by design (`packages/host/frontend-static/src/index.ts:1-12, 88-95`: "Non-index assets stay public"). They leak the client module graph (ids, revisions) to anything that can reach the port; they do not expose session data. In desktop mode these two routes are not reachable at all (no socket).
- LAN trust is derived only from an all-interfaces bind (`packages/bundle/web-app/src/index.ts:125-132`) plus explicit `--trusted-host` authorities, validated to be bare canonical authorities (`api-request-trust.ts:49-53`).

---

## 2. Runtime requirements

**Node.** One `engines` range for the whole app: root `package.json:8-10` `"node": "^22.19.0 || >=24.0.0"` (Node 23 excluded; floor forced to 22.19 by `@earendil-works/pi-ai`). No `engines` in any `apps/*` or `packages/**` manifest; native packages declare `>=20` (`native/system/packages/entry/package.json:33-35`). API usage is within that range: `node:sqlite` unflagged (web profile defers it entirely: `openAt: never`, `packages/bundle/web-app/cordis.patch.yml:25-30`), `import.meta.main` (`apps/cli/src/bin.ts:64`), `import.meta.resolve`, `Promise.withResolvers`, `structuredClone`; `using` is lowered at build (target `es2024`). `--experimental-sqlite` appears nowhere. Verified locally on Node v22.22.3: `require('node:sqlite')` works without a flag (ExperimentalWarning only) and `import.meta.main` exists.

**Native addons (no node-gyp at runtime, all prebuilt).**

- **`node-pty`** — hard dependency of `@deepseek-ai/dsh-subprocess-local` (`package.json:47`) and **statically imported** by it (`src/index.ts:15`). That package is a base-bundle row (`packages/bundle/base/cordis.patch.yml:199-200`), so every profile, including `web`, loads the addon at boot. Ships `prebuilds/<platform>-<arch>/pty.node` (+ macOS `spawn-helper`).
- **`koffi`** — same package (`package.json:46`), statically imported in `src/linux-execve.ts:4` and `src/windows-inspector.ts:13`; prebuilt platform packages `@koromix/koffi-*`.
- **`node-addon-require-builtin`** (+ optional `node-addon-require-builtin-{darwin,linux,win32}-*` with `prebuilt/<platform>-napi-v9.node`) — a **runtime dependency of `@deepseek-ai/dsh`** (`apps/cli/package.json:101`) used by the vendored loader to reach Node's internal ESM loader (`vendor/loader/src/internal.ts:110-117`).
- **`@deepseek-ai/node-addon-system`** — family of 4 OS/CPU-gated optional platform packages (`native/system/packages/entry/package.json:40-45`), all documented in `native/system/docs/packaging.md`: darwin-arm64, darwin-x64 (prebuild `bin/system.node`, an async `flock(LOCK_EX|LOCK_NB)` binding), linux-arm64, linux-x64 (adds the static-musl `bin/landlock-run` launcher and `bin/glibc|musl/system.node`). **Windows has neither package.** The flock binding is *not* a host single-instance lock — its only consumer is the per-session write lease `packages/session/session-persistence-jsonl/src/lease.ts:34,94` (Windows peer: a koffi named semaphore). Landlock is consumed by `packages/sandbox/sandbox-local` and `packages/shell/bash-sandbox`.
- `@vscode/ripgrep` is a dependency of `packages/fs/tool-fs-search` (shipped as `@vscode/ripgrep-<platform>-<arch>` optional packages) — needed only when the agent actually runs `grep`/`glob`.
- No `.node` binaries are checked in except one test fixture (`native/system/packages/darwin-arm64/bin/system.node`); no node-gyp/prebuildify in the repo.

**The one flag-shaped requirement.** The `web` profile template is `patchReload: 'live'` (`packages/boot/app-boot/src/profile.ts:115-118`), and `profile-boot.ts:366-371` mounts `@deepseek-ai/cordis-plugin-hmr` (watch-only, `root: []`) whenever no `hmr` service exists. That package's constructor throws `"--expose-internals is required for HMR service"` when `ctx.loader.internal` is undefined (`vendor/hmr/src/index.ts:121`), and the failure is re-thrown out of boot (`profile-boot.ts:382-384`). `Loader.internal` is populated by `ModuleLoader.fromInternal()` (`vendor/loader/src/index.ts:73`), which needs either `--expose-internals` in `process.execArgv` or the native addon above. The shipped bin does **not** pass the flag (only the source-run script does: `package.json:178`), so the addon is load-bearing for `dsh web`. *(Read from source; not empirically exercised in this audit. The desktop pipe path never mounts HMR, so it is immune.)*

**System tools.** Boot requires none: the web server is `node:http`; the only spawn during normal startup is `process.execPath` for the browser opener. Agent-level tools do shell out — `bash`, `pwsh`, `terminal_*` (node-pty), `osascript`/Zenity/KDialog, `xdg-open`, macOS `open -b`, an optional Python runtime defaulting to `python3`, optional tmux, and the bundled `@openai/codex`/Claude Agent SDK for those subagents. `git` is never spawned by runtime source. `pnpm` is required only by `dsh plugin` (§5).

**First-run downloads: none.** No release/CDN/pip/npx fetching in `packages/**/src` or `apps/**/src`. The only downloads are at build/packaging time (e.g. upstream Node from `https://nodejs.org/download/release/v24.17.0`, `apps/desktop/scripts/prepare-runtime.ts:14, 39-53`).

**TTY.** No shipped profile requires one: zero `isTTY` in `packages/**/src`/`apps/**/src`, no `setRawMode`; ACP/SDK read plain stdin pipes. `tui` is not a shipped template (`profile.ts:110-131` — only `acp`, `web`, `headless`, `sdk`, `sdk-minimal`), so any TUI lives outside this checkout.

---

## 3. Self-containment of a built tree

**What `pnpm run build` produces.** `package.json:20` → `tsx scripts/build.ts`, which runs (`scripts/build.ts:43-47`):

1. `build:native-system` (`tsx native/system/scripts/build.ts --host-addon-only`) → `native/system/packages/*/bin/{system.node,landlock-run}`.
2. `build:lib` = `build:lib:host` (`tsc -b tsconfig.host.json` then `tsdown --env.DSH_BUILD_FACE host`) + `build:lib:client` (`tsc -b tsconfig.client.json` then `tsdown --env.DSH_BUILD_FACE client`). Result: per-package `lib/*.js` host entries plus `lib/client.js` browser bundles, and `lib/types/**/*.d.ts`. `tsdown.config.ts:16-31` defines the workspace globs (`vendor/*`, `packages/*/*`, `apps/cli`, `apps/desktop`, `apps/desktop-host`), ESM, target `es2024`, typert plugin on the host face.
3. `build:web` → `apps/web` Vite build → `apps/web/dist/**` (≈13 MB here), published as `@deepseek-ai/dsh-web-frontend/dist` (`apps/web/package.json:2-25`).
4. Writes `.dsh-build/client-build-environment.json` (build record) via `scripts/client-build-environment.ts:32`. `.dsh-build/` otherwise is benchmark/dev scratch (`.gitignore:39`).

Two build facts that matter for reproducible/packaged builds: the client build shells out to `git rev-parse HEAD` unless `DSH_CLIENT_COMMIT_HASH` is set (`scripts/client-build-environment.ts:56-70`), and `build:official` (`scripts/build.ts` + `--profile official`) bakes `DSH_CLIENT_BUILD_PROFILE=official`, `DSH_CLIENT_TITLE=DeepSeek Harness` into browser artifacts.

**Bundled single-file / pruned tree — yes, three recipes.**

1. **Pruned production tree via `pnpm deploy`** — `scripts/build-exe-for-python-sdk.ts:289-299`:
   `pnpm --filter dsh-python-runtime-closure deploy --legacy --prod --config.node-linker=hoisted --config.auto-install-peers=false --config.link-workspace-packages=true <staging>`, then `runtime-bootstrap.mjs` is installed as the carrier root, symlinks are **materialised into real files** (`.bin` removed; any remaining symlink fails the build, `:354-379`), and direct workspace packages omitted by the legacy hoister are restored (`:315-352`). The deploy root `python/sdk-runtime/package.json` (126 `workspace:` deps) is the single source of truth for "which plugins ship"; `scripts/verify-runtime-closure.ts` proves the closure over every agent preset.
2. **Single-file executable** — `@yao-pkg/pkg --sea` (exact-pinned root devDependency `6.21.0`, locally patched by `patches/@yao-pkg__pkg@6.21.0.patch` to stop rewriting spawned `node` commands). One invocation per target, output `dist-exe/deepseek-harness-sdk-runtime-<platform>-<arch>`, plus a required `-rg` ripgrep sidecar, plus a macOS `-spawn-helper`; native `pty.node` is staged per target (`scripts/build-exe-for-python-sdk.ts:418-511`). Injected pkg asset globs cover dynamic reads: all `node_modules/**/*.{js,cjs,mjs,json,yaml,yml,md,node,so,dylib,dll,wasm}`, `@deepseek-ai/dsh-web-frontend/dist/**`, `dsh-skill-badge/assets/**` (`:41-62`). Rationale and measured constraints: `.agents/notes/implemented/architecture/2026-07-10-single-file-executable-sdk-runtime-distribution.md` (≈174 MB artifacts; "enhanced SEA" keeps real ESM resolution against a `/snapshot` VFS).
3. **Electron signed seed** (the "install kit", not a ready `node_modules`):
   - `apps/desktop/scripts/prepare-runtime.ts` — downloads + SHA-256-verifies upstream **Node 24.17.0** into `<target>/runtime/node/node`, copies the pinned **pnpm 11.7.0** package into `runtime/pnpm`, and records `runtime/versions.json`. (Note the fresh-write trick at `:65-66` to avoid macOS invalid code-signature vnode state.)
   - `apps/desktop/scripts/prepare-package-set.ts` — packs/collects the first-party closure rooted at `@deepseek-ai/dsh` and the private `@deepseek-ai/dsh-desktop-host` (dependencies + peerDependencies + optionalDependencies; any `@deepseek-ai/*` name not present is a hard error), copies the `.tgz`s under `desktop-packages/` and writes `desktop-packages.json` with size + `sha512-…` integrity (`apps/desktop/src/core-package-set.ts`). It also proves the Host tarball contains `lib/index.js` and `config/desktop.cordis.patch.yml`.
   - `apps/desktop/scripts/prepare-seed.ts` — builds the seed using the **same embedded Node+pnpm that first launch will use**: creates project metadata, runs `pnpm install --lockfile-only`, verifies the lockfile resolved every packaged core name locally (`verifyDesktopCoreLockfile`), runs `pnpm install --prod --frozen-lockfile --trust-lockfile --ignore-scripts`, deletes `node_modules` and all temp pnpm state, proves a full **`pnpm install --offline --frozen-lockfile --trust-lockfile`** works, on macOS Developer-ID-signs every Mach-O object in the pnpm CAS and rewrites the affected SHA-512 index rows (`apps/desktop/scripts/macos-seed-store.ts`, `signMacOSSeedStore`), removes pnpm project registrations, then replaces the loose store with **16 deterministic uncompressed tar shards** + `store-archives.json`, and finally writes `integrity.json` (sha256 per file).
   - `electron-builder.config.mjs:60-63` ships `runtime/` and `seed/` as `extraResources`; on first launch `DesktopProjectManager.applyRelease` extracts the shards into `$DSH_HOME/desktop/pnpm/store`, merges the SQLite package index, stages a project, and runs the bundled pnpm offline (`apps/desktop/src/project-manager.ts:394-437, 508-517`).

So: the repo has **no** "one big JS bundle for the host" (tsdown bundles per package, externalising bare imports), but it does have a proven offline, version-locked, integrity-verified distribution pipeline whose *payload* is "Node + pnpm + tarballs + pnpm store shards", and a proven single-file route if a self-contained binary is wanted.

---

## 4. State and configuration layout

**`$DSH_HOME`** (`packages/util/home-paths/src/index.ts`): `DSH_HOME` env var, else `~/.dsh` (`:12, 61-63, 87-91`); blank/whitespace `DSH_HOME` is treated as unset; `~`, `~/`, `~\` expand, `~user` does not; the module itself performs no `mkdir` and no legacy migration. It is created lazily by the first consumer.

**Profile layout** (`$DSH_HOME/profiles/<name>/`, `packages/boot/app-boot/src/profile.ts`):

| Path | Written by | Notes |
|---|---|---|
| `package.json` | `initProfile` `:176-185` | `dsh-profile-<name>`, `private`, `dependencies`, `dsh.profile.{bundles,patchReload}` |
| `cordis.patch.yml` | `initProfile` `:186-187` | the user layer; hot-reloaded for `live` profiles |
| `pnpm-workspace.yaml` | `initProfile` `:188-189` | `nodeLinker: hoisted`, `autoInstallPeers: false` |
| `cordis.yml` | `profile-boot.ts:187-191` | root include; **deliberately rewritten on every boot** |
| `node_modules/` | pnpm (plugin installs) + `healProfileModuleFallback` `:616-650` | |
| `.dsh-module-fallback/` | same | owned projection dir for bundle-only packages |
| `$DSH_HOME/profiles/node_modules/` | `healProfilesModuleFallback` `:552-565` | symlinks to the installation (plain Node) or generated ESM proxy packages when `process.pkg` is set (`:319-322, 407-453`), guarded by a `wx` lock on `profiles/node_modules.lock` |

`dsh.profile.bundles` is an **ordered bundle-layer list**; each named package must declare `dsh.bundle.patch`; layers are applied over an empty root in order, then the profile patch, then `$DSH_HOME/cordis.patch.yml` (read-only, never created), then `--patch` overlays, then the telemetry switch (`profile-boot.ts:206-244`). Shipped templates: `acp`, `web`, `headless`, `sdk`, `sdk-minimal` (`profile.ts:110-131`); `desktop` is reserved for Electron and rejected by the CLI; an unknown profile name errors with a hint to use `dsh plugin`.

**No prior state.** First boot of `dsh web` creates `~/.dsh/`, `profiles/web/{package.json,cordis.patch.yml,pnpm-workspace.yaml,cordis.yml}`, `profiles/node_modules/**` fallback links, then lazily `settings.yaml`, `.credentials.yaml`, `sessions/**`, `storages/`, `attachments/v1/`, `.anonymous-user-id`, `llm-deepseek/files-v3.json`. `.agent-presets/`, `skills/`, `AGENTS.md`, and `$DSH_HOME/.env` are read-only inputs. **Nothing is written into the installation directory at startup.**

**Environment variables that matter for a packaged app.**

- `DSH_HOME` — the only one a launcher really must set (per-user writable state).
- `NODE_OPTIONS` — never trusted from a file (`BOOTSTRAP_NAMES`, `app-boot/src/index.ts:93-118`), and the Electron shell strips it from the child's env; a supervisor should do the same.
- `.env` layering: inherited env > `<cwd>/.env` (project) > `$DSH_HOME/.env` (user) (`app-boot/src/index.ts:195-216`); **every `DSH_*`, `XDG_*`, `DYLD_*`, `BASH_FUNC_*` name is refused inside a `.env`**, with the sole exception of proxy names in the home file (`:93-138, 169-170`).
- Policy switches a packager may want to pin: `DSH_PERMISSION_MODE`, `DSH_TELEMETRY_MODE` / `DSH_TELEMETRY_OTLP_URL` / `DSH_TELEMETRY_DISABLED`, `DSH_TOOLS_MODE`, `DSH_AGENTS_HOME`, `DSH_BUNDLED_SKILL_DIR`, `DSH_WEB_SEARCH_PROVIDER`/`DSH_WEB_FETCH_PROVIDER`.
- Dev/QA-only (ignored by a packaged app): `DSH_DESKTOP_{NODE_BINARY,PNPM_ENTRY,SEED_DIR,DEV_PROJECT_DIR,HOST_INSPECT_PORT,OPEN_DEVTOOLS,DIAGNOSTIC_FILE}`.
- `DSH_LAUNCH_ENVIRONMENT_KEY` is a Cordis context key, not an env var (`packages/util/launch-environment/src/index.ts:106`).

**Locks / single instance.** There is **no `$DSH_HOME` lock and no profile lock**: two `dsh web` processes on the same profile are permitted, and exclusivity is only the port bind. Serialisation that does exist: `withFileLock` (`<file>.lock` with pid content, `wx`, 20→200 ms backoff, no stale recovery — `packages/util/atomic-write/src/index.ts:114-183`) on `profiles/node_modules`, settings, credentials, and the deepseek upload index; the per-session `flock` lease; and, for desktop, a separate `$DSH_HOME/desktop/lock` transaction lock plus Electron's `app.requestSingleInstanceLock()` keyed on app identity, not on `$DSH_HOME` (`apps/desktop/src/single-instance.ts:20`). A Tauri shell must therefore implement its own single-instance lock (Tauri has one) or accept concurrent backend profiles.

**Credentials.** `$DSH_HOME/.credentials.yaml`, dirs `0700` / file `0600`, owner-only mode asserted at boot (startup aborts with a chmod hint if group/other bits are set; skipped on Windows) — `packages/credentials/credentials-local/src/index.ts:61, 90, 127-146, 683-699`.

---

## 5. Plugin/package mutation at runtime

- **A running profile never runs pnpm by itself.** Boot only *reads*; the only writers are `initProfile`, the module-fallback healer, and the always-rewritten `cordis.yml`. Installing plugins is an explicit user action: `dsh plugin --profile <name> add|remove|update …` forwards argv verbatim to **pnpm on PATH**, with cwd = the profile directory, then reconciles `dsh.profile.bundles` against what actually installed (`apps/cli/src/plugin.ts:120-163`). Missing pnpm prints `pnpm not found on PATH — install pnpm to manage profile plugins` and returns **exit code 127** (`:139-146`). Relative specs are anchored to the invoking cwd (`:104-112`); git-hosted plugins' `prepare` scripts are gated by pnpm's `allowBuilds`, and the failure message points at `profiles/<name>/pnpm-workspace.yaml` (`:151-160`).
- **Desktop** never uses system pnpm: it bundles pnpm and runs it under its own store with `--config.registry=https://registry.npmjs.org/`, a private `userconfig`, and `XDG_*` redirected under `$DSH_HOME/desktop/pnpm` (`apps/desktop/src/project-manager.ts:551-587`). Mutations go through staging + health check + journaled directory swap + rollback (`:439-549`), and are exposed only to the shell's own renderer over IPC (`apps/desktop/src/main.ts:233-263`).
- **Read-only mode for a Homebrew-managed install:** there is **no flag** and no documented "no-mutation" mode. In practice a read-only prefix is fine — the app never writes to its install tree — but two consequences follow: (1) `dsh plugin` will still try to write into `$DSH_HOME/profiles/<name>` (not into the prefix), so it fails only if `$DSH_HOME` is unwritable; (2) if you want to forbid plugin installation entirely, you must not ship/expose pnpm (the desktop profile does exactly this by owning package transactions itself and by refusing `--profile desktop` in the CLI).

---

## 6. Client asset serving contract

**Served (HTTP) mode.**

- `index.html` comes from `@deepseek-ai/dsh-web-frontend/dist/index.html`, resolved at runtime via `createRequire(import.meta.url).resolve(...)` (`packages/bundle/web-app/src/index.ts:162-170`) and served by the `frontend-static` fallback (`packages/host/frontend-static/src/index.ts:113-142`) with `<base href="/">` injected (`:120-123`).
- Before serving, the webserver renders **index injections**: structured rows collected from the `webserver/index-inject` event (`packages/host/webserver/src/index.ts:347-361`), then raw taps. `client-modules` pushes (a) an inline bootstrap `<script>` defining `window.__ModuleLoader__`, (b) `<script>`/`<link rel=modulepreload>` tags for the bootstrap and application batches, and (c) a **`window.__DSH_BOOT__` global holding the composed client graph** (`packages/client/modules/src/index.ts:498-508`). `Connection` adds `__DSH_CONNECTION_RECOVERY__` (`packages/client/connection/src/index.ts:121-123`).
- Plugin bundles: `GET /plugins/<id>/client.js` (and `.map`), plus combo URLs `/plugins/??<id>/client.js,…&rev=<rev>` served from an in-memory table with immutable caching (`packages/client/modules/src/index.ts:240-241, 293, 405, 1007-1044`).
- Reload stream: `GET /plugins/events` = SSE (`text/event-stream`) emitting a `graph` frame on connect and `rebuilt` frames per rebuilt plugin (`packages/client/hmr/src/index.ts:158-200`; endpoint constant `packages/client/hmr/src/events.ts:44`). The browser half fetches bundles as same-origin classic `<script>` tags unless a transport supplies `loadBundle` (`packages/client/modules/src/client/system.ts:17-30`).
- Transport: unary RPC is `POST /api/<endpoint>` with `{type:'client-request', rpcId, method, payload}` → `{type:'server-response', rpcId, result:{ok:true,value}|{ok:false,error:{code,message,details}}}` against `location.origin` (`packages/client/connection/src/client/rpc.ts:31-71, 105-111`); remote streams use the WebSocket `/api/remote.mux` (`packages/api/gateway/src/stream-protocol.ts:6`; `ws` package), heartbeat-monitored (`packages/api/gateway/src/stream-server.ts:47-64`).

**"No listening port" mode (`dsh-app://`).** Electron registers a privileged custom scheme `dsh-app` (`standard`, `secure`, `supportFetchAPI`, `stream`, `codeCache`, `corsEnabled:false`) — `apps/desktop/src/main.ts:30-40` — and routes it: `dsh-app://shell/*` serves the shell's own renderer files, `dsh-app://app/*` is forwarded verbatim to the child host (`main.ts:224-231`). The child exposes exactly three things behind that URL space: `/api/*` (Connection's shared fetch handler), `/.dsh/remote-stream` (POST NDJSON), and everything else = the SPA dist (`apps/desktop-host/src/index.ts:305-307, 338-343`). The served index gets one extra inline script, `globalThis.__DSH_TRANSPORT__ = { ownsHost: true, async *openStream(endpoint,payload,signal){…} }`, which routes Remote streams through `POST /.dsh/remote-stream` and parses newline-delimited JSON back (`index.ts:99-120`). `ownsHost: true` makes `ctx.connection.isLoopback` true without a loopback origin (`packages/client/connection/src/client/index.ts:99-108, 227`).

**Wire protocol to reimplement (protocol version 3)** — from `apps/desktop-host/src/wire.ts` and `apps/desktop/src/host-protocol.ts`:

- Frame header, both directions: 13 bytes — `u32be magic = 0x44534833` ("DSH3"), `u8 type`, `u32be streamId` (1…0xffffffff, monotonically increasing per direction), `u32be payloadLength`; then payload. Data frames cap 64 KiB; control frames cap 1 MiB (JSON).
- Request pipe (parent → child, fd 3): `1 start` = JSON `{url, method, headers:[[k,v],…], hasBody}`, `2 data` = raw body bytes, `3 end` (empty), `4 cancel` (empty).
- Response pipe (child → parent, fd 4): `1 start` = JSON `{status, headers, hasBody}`, `2 data` = raw bytes, `3 end` (empty), `4 error` = JSON `{message}`.
- Control channel (fd 5, Node IPC): parent → child `{type:'shutdown'}`; child → parent `{type:'ready', protocolVersion:3, dshVersion}` | `{type:'fatal', message}`.
- Flow control exists on both sides (pause the pipe when a stream's `desiredSize <= 0`; `host-process.ts:322-327, 384-386`, `desktop-host/src/index.ts:512-516, 421-423`). Body and response may interleave across streams; reusing/reordering a stream id is a protocol error.
- **For a Rust/Tauri shell the practical choices are**: (a) implement this framing and supply a Node-IPC channel (set `NODE_CHANNEL_FD` to the fd and write newline-delimited JSON — Node's pipe IPC framing), or patch `apps/desktop-host/src/index.ts:378` to make the IPC channel optional and use SIGTERM for shutdown; (b) skip pipes entirely and use loopback `dsh web` (simplest; you then own the browser tab/webview URL with the `?token=` URL, and must handle `/api` cookie auth by navigating the webview to that URL once); or (c) reimplement the HTTP surface behind Tauri's own custom protocol and inject a `__DSH_TRANSPORT__` shim like Electron does — the contract above is complete enough to do so, since bundle delivery, `window.__DSH_BOOT__`, RPC envelopes and stream framing are all documented in-tree.

---

## 7. Distribution-affecting constraints

**Things that are fine (verified):**

- No runtime writes into the install directory; every write goes to `$DSH_HOME` (`packages/boot/app-boot/src/profile.ts` is the only startup writer, plus the desktop host's `desktop.cordis.yml` inside the profile dir).
- Installation anchors are derived from `import.meta.url` (`apps/cli/src/profile-boot.ts:78`, `packages/bundle/web-app/src/index.ts:34, 162-170`), so a relocated prefix works, and the module-fallback links are re-healed when the prefix path changes (`profile.ts:508-528`).
- No postinstall script in any published package except `@deepseek-ai/dsh-subprocess-local`'s `node scripts/ensure-spawn-helper.mjs` (chmod +x on node-pty's macOS `spawn-helper`; `packages/subprocess/subprocess-local/package.json:36`). The repo-root `postinstall` (`node scripts/install-lefthook.mjs`) is dev-only and not part of a published install.
- No `engines`-violating native compile step: all addons ship as prebuilds with `os`/`cpu` gates, so an install on a supported platform never invokes a compiler.

**Things that will break a naive "copy the tree and run it":**

1. **Absolute path assumptions in state.** `$DSH_HOME/profiles/node_modules/*` symlinks and `.dsh-module-fallback` entries point at whatever prefix was running when they were created. They self-heal on the next boot from the new anchor, but a copied tree whose `$DSH_HOME` is *also* copied keeps stale links until the first boot. (The pkg/SEA variant avoids OS symlinks entirely by writing ESM proxy modules — `profile.ts:319-322, 407-453`.)
2. **Hard requirement of `process.send` in pipe mode** (see §1.1) — a Rust supervisor must emulate Node IPC or the entry must change.
3. **`dsh web` needs internals access** (native `node-addon-require-builtin` platform package, or `--expose-internals`). Pruning optionalDependencies, or an OS/CPU with no matching optional package, breaks the web profile's live-reload mount.
4. **`dsh plugin` requires a PATH `pnpm`** and writes into `$DSH_HOME/profiles/<name>`; a "root-owned read-only prefix + read-only `$DSH_HOME`" deployment cannot manage plugins, and there is no supported read-only flag.
5. **Node itself is a runtime dependency.** A Homebrew/nix formula must depend on Node ≥22.19 (or ship one). The Electron pipeline downloads Node 24.17.0 from nodejs.org and verifies its SHA-256 (`prepare-runtime.ts:39-53`); the SEA pipeline embeds Node 24 in the executable. Native addons are ABI-stable Node-API, so any Node ≥22.19 works — but `@yao-pkg/pkg` outputs are bound to the target Node major.
6. **macOS signing/notarization of every native binary**, not just the app: the Electron pipeline signs each Mach-O object inside the pnpm CAS and rewrites the store index hashes (`apps/desktop/scripts/macos-seed-store.ts`), enforces `flags=0x…(runtime)` + secure timestamp (`verify-macos-signature.mjs:29-42`), and requires an exact `Authority=Developer ID Application: …` leaf + Team ID on a deep-strict check. A Tauri build that embeds a Node binary + `node-pty`/`koffi`/`ripgrep`/`landlock-run` inherits the same obligation (hardened runtime, per-file signatures, staple).
7. **Windows**: no `@deepseek-ai/node-addon-system` platform package (Landlock/flock unavailable — Windows uses a koffi named semaphore and an ACL sandbox instead), and the desktop target set is `mac-arm64 | mac-x64 | win-x64` only (`apps/desktop/scripts/desktop-build-paths.mjs:7`). Linux desktop releases are explicitly unsupported (`apps/desktop/README.md`).
8. **Build-time, not runtime, network**: `pnpm install` + `pnpm run build` need the pnpm store/network and dev toolchain (`tsx`, `tsdown`, `typescript`, `vite`), and the client build shells out to `git` unless `DSH_CLIENT_COMMIT_HASH` is provided (`scripts/client-build-environment.ts:56-70`). A nix/Homebrew *source* build must vendor the store or use a fixed-output derivation.
9. **Version coupling**: Electron, `@deepseek-ai/dsh`, the private `dsh-desktop-host`, the seed, and the embedded Node/pnpm are qualified as one version (`apps/desktop/scripts/prepare-seed.ts:51-65` fails if they differ). If you adopt the seed model for Tauri, you inherit that lock-step release discipline; if you adopt the npm/CLI model, you inherit the `$DSH_HOME/profiles/<name>` mutable profile instead.
10. **First-run disk + time**: the desktop seed extracts the store into `$DSH_HOME/desktop/pnpm/store` and installs a profile before the first window opens (`apps/desktop/README.md`, "Seed installation"), i.e. first launch is a package installation, not just a process start. A Tauri shell should show progress and expect a health-check boot before showing UI.
11. **Update channel is bespoke**: electron-updater plus Tencent COS (`apps/desktop/scripts/desktop-auto-update-environment.mjs:1-40`, publish provider `generic` at `https://download.deepseek.com`). A Tauri updater would replace this entirely.

---

## Appendix: the Python runtime wheel (how it packages `dsh`)

`python/` holds two distributions: `python/sdk` (pure-Python client, `py3-none-any`) and `python/sdk-runtime` (`deepseek-harness-runtime-bin`). The runtime wheel is **platform-tagged** (`py3-none-macosx_14_0_arm64`, `…manylinux_2_28_x86_64`, `py3-none-win_amd64`, …; `python/sdk-runtime/hatch_build.py:60-98`) and contains exactly the single-file executable plus its arch-matched ripgrep sidecar (`<exe>-rg`, `<exe>-rg.exe`) and, on macOS, the node-pty `<exe>-spawn-helper`; the Hatch hook rejects sdists, universal tags, mixed-platform payloads, missing/extra sidecars, and non-executable bits. `scripts/build-python-release.py` stamps both wheels from the root `package.json` version and makes the SDK depend exactly on the matching runtime wheel (`docs/user/guide/python-sdk.md:162` notes the wheel still carries the full `web` profile and frontend assets, so `dsh web` works from it against an explicit `DSH_HOME`). The wheel's executables are produced by `scripts/build-exe-for-python-sdk.ts` (see §3), and the Python client launches `dsh` with an explicit `--profile` (default `sdk`) and Harness home — "Python never silently reads `~/.dsh`" (`python/README.md:16`), which is the closest thing in the repo to a prescriptive packaged-app state policy.
