/**
 * Build one self-contained dsh runtime closure from the npm registry.
 *
 * The closure is a production `node_modules` tree rooted at `@deepseek-ai/dsh`
 * plus a `runtime.json` manifest. It deliberately contains no Node runtime and
 * no platform-specific prebuilt addon choices: those are resolved per machine
 * by the installer (see the design doc, D2/D7).
 *
 * Output: .dsh-build/runtime/<channel>-<version>/
 *   runtime.json            manifest (channel, version, platform, entry, sha256)
 *   package.json            deployed root manifest
 *   node_modules/@deepseek-ai/dsh/lib/bin.js
 *   ...
 *
 * What it ships is decided by `runtime-omit.json`: every entry names a package
 * this app deliberately does not carry, and says what stops working without it.
 * The policy is applied as a pnpm `overrides` removal, so an omitted package is
 * never resolved — neither it nor its private dependencies are downloaded,
 * deployed, hashed or signed. `--full` ignores the policy and is the build that
 * can still see the omitted packages, which is what keeps the list honest.
 *
 * Usage:
 *   node scripts/prepare-runtime.mjs [--version <v>] [--channel release] [--force]
 *                                    [--keep-staging] [--full]
 */

import { createHash } from 'node:crypto'
import { cp, mkdir, mkdtemp, readFile, readdir, readlink, rename, rm, stat, writeFile } from 'node:fs/promises'
import { existsSync } from 'node:fs'
import { execFile } from 'node:child_process'
import { promisify } from 'node:util'
import { dirname, join, relative, resolve, sep } from 'node:path'
import { fileURLToPath } from 'node:url'

const run = promisify(execFile)

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..')
// `DSH_BUILD_ROOT` redirects the output tree, so a packaging build (Nix, CI) can
// reuse this recipe instead of re-implementing the closure layout — the deploy
// flags and the manifest shape are the parts most likely to rot if copied.
const BUILD = process.env.DSH_BUILD_ROOT ?? join(ROOT, '.dsh-build')
const RUNTIMES = join(BUILD, 'runtimes')
const STAGING_ROOT = join(BUILD, 'staging')

/** Read the pinned runtime version and package name from the root manifest. */
async function readPin() {
  const manifest = JSON.parse(await readFile(join(ROOT, 'package.json'), 'utf8'))
  const pin = manifest.dshRuntime
  if (pin === undefined || typeof pin.version !== 'string' || typeof pin.package !== 'string') {
    throw new Error('prepare-runtime: package.json needs a dshRuntime { package, version } pin')
  }
  return pin
}

/** Parse argv. Unknown flags fail loudly rather than silently changing the build. */
function parseArgs(argv) {
  const options = { version: undefined, channel: 'release', force: false, keepStaging: false, full: false }
  for (let index = 0; index < argv.length; index += 1) {
    const token = argv[index]
    switch (token) {
      case '--version': {
        const value = argv[index + 1]
        if (value === undefined || value.startsWith('--')) throw new Error('prepare-runtime: --version needs a value')
        options.version = value
        index += 1
        break
      }
      case '--channel': {
        const value = argv[index + 1]
        if (value !== 'release' && value !== 'master') throw new Error('prepare-runtime: --channel must be release or master')
        options.channel = value
        index += 1
        break
      }
      case '--force':
        options.force = true
        break
      case '--keep-staging':
        options.keepStaging = true
        break
      case '--full':
        options.full = true
        break
      default:
        throw new Error(`prepare-runtime: unknown argument ${JSON.stringify(token)}`)
    }
  }
  return options
}

/** Node's architecture tag, as the runtime manifest records it. */
function platformKey() {
  if (process.platform !== 'darwin') {
    throw new Error(`prepare-runtime: ${process.platform} is not a supported target yet (macOS only)`)
  }
  return `${process.platform}-${process.arch}`
}

/** Run one command, surfacing the tail of stdout and stderr on failure. */
async function exec(command, args, options = {}) {
  try {
    return await run(command, args, { cwd: ROOT, maxBuffer: 64 * 1024 * 1024, ...options })
  } catch (error) {
    const tail = (value) => {
      const text = typeof value === 'string' ? value.trim() : ''
      if (text === '') return ''
      return text.split('\n').slice(-25).join('\n')
    }
    const sections = [
      `prepare-runtime: \`${command} ${args.join(' ')}\` failed`,
      tail(error.stdout) === '' ? '' : `--- stdout ---\n${tail(error.stdout)}`,
      tail(error.stderr) === '' ? '' : `--- stderr ---\n${tail(error.stderr)}`,
    ].filter((section) => section !== '')
    throw new Error(sections.join('\n'))
  }
}

/** Materialize every symlink under a tree, and reject entries pnpm leaves behind. */
async function materializeLinks(dir, base = dir) {
  const entries = await readdir(dir, { withFileTypes: true })
  for (const entry of entries) {
    const path = join(dir, entry.name)
    if (entry.isSymbolicLink()) {
      const target = await readlink(path)
      const resolved = resolve(dirname(path), target)
      const info = await stat(resolved).catch(() => undefined)
      if (info === undefined) {
        throw new Error(`prepare-runtime: dangling symlink ${relative(base, path)} -> ${target}`)
      }
      await rm(path, { force: true })
      if (info.isDirectory()) {
        await cp(resolved, path, { recursive: true, dereference: true })
        await materializeLinks(path, base)
      } else {
        await cp(resolved, path)
      }
      continue
    }
    if (entry.isDirectory()) await materializeLinks(path, base)
  }
}

/** Remove pnpm's bin shims; the closure is launched by explicit entry path. */
async function removeBinDirs(dir) {
  const entries = await readdir(dir, { withFileTypes: true })
  for (const entry of entries) {
    const path = join(dir, entry.name)
    if (entry.isDirectory() && entry.name === '.bin') {
      await rm(path, { recursive: true, force: true })
      continue
    }
    if (entry.isDirectory()) await removeBinDirs(path)
  }
}

/** Sorted [{ path, size }] for every regular file under one directory. */
async function listFiles(dir, base = dir) {
  const out = []
  const entries = await readdir(dir, { withFileTypes: true })
  for (const entry of entries) {
    const path = join(dir, entry.name)
    if (entry.isDirectory()) {
      out.push(...await listFiles(path, base))
      continue
    }
    if (!entry.isFile()) continue
    const info = await stat(path)
    out.push({ path: relative(base, path).split(sep).join('/'), size: info.size })
  }
  return out.sort((left, right) => (left.path < right.path ? -1 : left.path > right.path ? 1 : 0))
}

/** sha256 over a deterministic digest of every file path and its contents. */
async function hashTree(dir) {
  const hash = createHash('sha256')
  for (const file of await listFiles(dir)) {
    hash.update(file.path)
    hash.update('\0')
    hash.update(String(file.size))
    hash.update('\0')
    hash.update(await readFile(join(dir, file.path)))
    hash.update('\0')
  }
  return hash.digest('hex')
}

/** Verify the closure actually exposes the launcher entry the shell spawns. */
async function assertEntrypoint(closure) {
  const manifestPath = join(closure, 'node_modules', '@deepseek-ai', 'dsh', 'package.json')
  if (!existsSync(manifestPath)) {
    throw new Error('prepare-runtime: closure is missing node_modules/@deepseek-ai/dsh/package.json')
  }
  const manifest = JSON.parse(await readFile(manifestPath, 'utf8'))
  const entry = join(closure, 'node_modules', '@deepseek-ai', 'dsh', 'lib', 'bin.js')
  if (!existsSync(entry)) {
    throw new Error(`prepare-runtime: closure is missing the launcher entry ${relative(closure, entry)} (bin field: ${JSON.stringify(manifest.bin)})`)
  }
  for (const bundle of ['dsh-base', 'dsh-web-app', 'dsh-web-frontend']) {
    if (!existsSync(join(closure, 'node_modules', '@deepseek-ai', bundle))) {
      throw new Error(`prepare-runtime: closure is missing @deepseek-ai/${bundle}; the web carrier boot would fail`)
    }
  }
  return { entry: relative(closure, entry).split(sep).join('/'), dshVersion: manifest.version }
}

/**
 * The closure's exclude policy, validated rather than trusted.
 *
 * `why` and `effect` are required, not decorative: they are the review surface
 * for a change that silently removes a feature, and the only place a later
 * reader can learn what an omission costs without re-deriving it.
 */
async function readOmitPolicy() {
  const path = join(ROOT, 'runtime-omit.json')
  const text = await readFile(path, 'utf8').catch(() => undefined)
  if (text === undefined) return []
  let parsed
  try {
    parsed = JSON.parse(text)
  } catch (error) {
    throw new Error(`prepare-runtime: ${relative(ROOT, path)} is not valid JSON: ${error.message}`)
  }
  if (parsed.schema !== 1) throw new Error(`prepare-runtime: ${relative(ROOT, path)} has unsupported schema ${parsed.schema}`)
  const entries = Array.isArray(parsed.omit) ? parsed.omit : []
  for (const entry of entries) {
    for (const field of ['name', 'why', 'effect']) {
      if (typeof entry?.[field] !== 'string' || entry[field].trim() === '') {
        throw new Error(`prepare-runtime: every omit entry needs a non-empty "${field}"; offending entry: ${JSON.stringify(entry)}`)
      }
    }
  }
  const names = new Set(entries.map((entry) => entry.name))
  if (names.size !== entries.length) throw new Error('prepare-runtime: runtime-omit.json lists the same package twice')
  return entries
}

/**
 * The staging workspace manifest, including the omit policy.
 *
 * Removals go through pnpm's `overrides` syntax (`'-'` drops the dependency),
 * which is the only supported way to keep a package out of the closure: the
 * resolver never sees it, so it is never downloaded either. The setting has to
 * live in `pnpm-workspace.yaml` — pnpm 11 ignores `pnpm.overrides` in
 * package.json and only prints a warning about it.
 */
function workspaceManifest(omit, full) {
  const lines = ['packages:', "  - 'runtime'", '']
  if (!full && omit.length > 0) {
    lines.push('overrides:')
    for (const entry of omit) lines.push(`  '${entry.name}': '-'`)
    lines.push('')
  }
  return lines.join('\n')
}

/** Regex-escape one package name, so a scope's dots and slashes stay literal. */
function escapeRegExp(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/gu, '\\$&')
}

/**
 * The gate that keeps an omission from breaking the boot.
 *
 * An omitted package disappears from the resolver, so a kept package that
 * imports it statically (or resolves it by a literal specifier) fails at load
 * time — long after the build said everything was fine. The plugin loader
 * resolves bundles by *name* at run time, which rules out a reachability
 * analysis, but it also means the dangerous references are literal strings in
 * the published files; those are cheap to find.
 *
 * Computed specifiers (`require.resolve(\`${prefix}-${target}/package.json\`)`)
 * are deliberately not matched: a package that goes to that trouble is the one
 * designed to be optional, and `pnpm runtime:trace` plus the smoke test are
 * what check those.
 */
async function staticReferences(closure, omit) {
  const names = omit.map((entry) => entry.name)
  const hits = []
  for (const file of await listFiles(closure)) {
    if (!/\.(?:js|mjs|cjs)$/u.test(file.path)) continue
    const text = await readFile(join(closure, file.path), 'utf8').catch(() => '')
    for (const name of names) {
      if (!text.includes(name)) continue
      if (new RegExp(`(?:from|require\\(|import\\(\\s*)['"]${escapeRegExp(name)}(?:/[^'"]*)?['"]`, 'u').test(text)) {
        hits.push({ file: file.path, name })
      }
    }
  }
  return hits
}

/**
 * Both halves of "the policy is real".
 *
 * A `--full` closure is the only build that can still see an omitted package,
 * so it is the one that fails when an entry has gone stale (renamed upstream,
 * dropped from the dependency graph, typo) — otherwise a rotted entry would
 * look like a working policy while the bytes ship anyway. A pruned build
 * asserts the opposite: the package really is gone.
 */
async function assertOmissions(closure, omit, full) {
  const nm = join(closure, 'node_modules')
  for (const entry of omit) {
    const present = existsSync(join(nm, entry.name))
    if (full && !present) {
      throw new Error(
        `prepare-runtime: ${relative(ROOT, join(ROOT, 'runtime-omit.json'))} omits ${entry.name}, but a --full closure does not contain it; ` +
          'the entry is stale (renamed, dropped, or never a dependency) and must be removed or corrected',
      )
    }
    if (!full && present) {
      throw new Error(`prepare-runtime: ${entry.name} is in runtime-omit.json and still present in the closure; the override did not apply`)
    }
  }
  if (full) return
  const hits = await staticReferences(closure, omit)
  if (hits.length > 0) {
    const detail = hits.slice(0, 10).map((hit) => `  ${hit.name} <- ${hit.file}`).join('\n')
    throw new Error(
      `prepare-runtime: ${hits.length} kept file(s) reference an omitted package statically; the boot would fail:\n${detail}`,
    )
  }
}

async function main() {
  const options = parseArgs(process.argv.slice(2))
  const pin = await readPin()
  const version = options.version ?? pin.version
  const platform = platformKey()
  const id = `${options.channel}-${version}`
  const output = join(RUNTIMES, id)
  const omit = await readOmitPolicy()
  if (omit.length > 0) {
    const names = omit.map((entry) => entry.name).join(', ')
    console.log(
      options.full
        ? `prepare-runtime: --full: shipping everything, policy not applied (${names})`
        : `prepare-runtime: omitting ${omit.length} package(s): ${names}`,
    )
  }

  if (existsSync(join(output, 'runtime.json')) && !options.force) {
    const existing = JSON.parse(await readFile(join(output, 'runtime.json'), 'utf8'))
    if (existing.dshVersion === version && existing.platform === platform) {
      console.log(`prepare-runtime: reusing ${relative(ROOT, output)} (dsh ${version}, ${platform})`)
      console.log(`prepare-runtime: pass --force to rebuild`)
      return
    }
  }

  await mkdir(STAGING_ROOT, { recursive: true })
  const staging = await mkdtemp(join(STAGING_ROOT, `${id}-`))
  // `pnpm deploy` refuses the workspace root, so the closure project is a
  // workspace member (the same shape upstream's Python runtime uses).
  const project = join(staging, 'runtime')
  const projectName = 'dsh-app-runtime'
  // Absolute: the path handed to pnpm and the path checked afterwards must be
  // the same string, or a differing cwd turns a success into a false failure.
  const deploy = resolve(staging, 'closure')
  console.log(`prepare-runtime: staging ${relative(ROOT, staging)}`)

  try {
    const spec = `${pin.package}@${version}`
    await mkdir(project, { recursive: true })
    await writeFile(join(staging, 'pnpm-workspace.yaml'), workspaceManifest(omit, options.full), 'utf8')
    await writeFile(join(project, 'package.json'), `${JSON.stringify({
      name: projectName,
      private: true,
      version: '0.0.0',
      description: `${pin.package} production closure for the dsh-app desktop shell`,
    }, null, 2)}\n`, 'utf8')

    console.log(`prepare-runtime: adding ${spec} (exact, scripts disabled)`)
    await exec('pnpm', ['--filter', projectName, 'add', '--save-exact', '--ignore-scripts', spec], { cwd: staging })

    console.log('prepare-runtime: deploying production closure (hoisted, links materialized)')
    const deployed = await exec('pnpm', [
      '--filter', projectName,
      'deploy',
      '--legacy',
      '--prod',
      '--ignore-scripts',
      '--config.node-linker=hoisted',
      // NOT auto-install-peers=false: the published packages declare 24
      // harness packages as peers (the profile's shared fallback provides them
      // at runtime), so disabling peer installation strips them from the
      // closure and the first boot dies on ERR_MODULE_NOT_FOUND.
      '--config.auto-install-peers=true',
      '--config.link-workspace-packages=true',
      deploy,
    ], { cwd: staging })
    if (deployed.stdout.trim() !== '') console.log(deployed.stdout.trim())
    if (deployed.stderr.trim() !== '') console.error(deployed.stderr.trim())
    // `pnpm deploy` can exit 0 without producing anything when its arguments are
    // not the ones it expects, which would otherwise surface as an unrelated
    // ENOENT three steps later.
    if (!existsSync(deploy)) {
      throw new Error(
        `pnpm deploy reported success but produced no directory at ${deploy}; ` +
          `pnpm ${process.env.npm_config_user_agent ?? '(unknown version)'} was invoked with: ` +
          'deploy --legacy --prod --ignore-scripts --config.node-linker=hoisted ' +
          '--config.auto-install-peers=true --config.link-workspace-packages=true',
      )
    }

    await materializeLinks(deploy)
    await removeBinDirs(deploy)
    const { entry, dshVersion } = await assertEntrypoint(deploy)
    await assertOmissions(deploy, omit, options.full)

    const files = await listFiles(deploy)
    const bytes = files.reduce((total, file) => total + file.size, 0)
    console.log(`prepare-runtime: closure has ${files.length} files, ${(bytes / 1024 / 1024).toFixed(1)} MiB`)

    const sha256 = await hashTree(deploy)
    const manifest = {
      schema: 1,
      channel: options.channel,
      dshVersion,
      requestedVersion: version,
      platform,
      source: { kind: 'npm', spec },
      entry: { kind: 'node', path: entry },
      node: { requirement: '^22.19.0 || >=24.0.0' },
      files: { count: files.length, bytes },
      // What this closure deliberately does not carry, so a diagnostic can say
      // "this build has no X" instead of leaving a missing feature unexplained.
      omitted: options.full ? [] : omit.map((entry) => ({ name: entry.name, why: entry.why, effect: entry.effect })),
      sha256,
    }
    await writeFile(join(deploy, 'runtime.json'), `${JSON.stringify(manifest, null, 2)}\n`, 'utf8')

    await mkdir(RUNTIMES, { recursive: true })
    await rm(output, { recursive: true, force: true })
    await rename(deploy, output)
    console.log(`prepare-runtime: published ${relative(ROOT, output)}`)
    console.log(`prepare-runtime: sha256 ${sha256}`)
  } finally {
    if (!options.keepStaging) await rm(staging, { recursive: true, force: true })
    else console.log(`prepare-runtime: kept staging at ${relative(ROOT, staging)}`)
  }
}

await main()
