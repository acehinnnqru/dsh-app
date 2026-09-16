/**
 * Decide whether one Node executable can actually run a prepared dsh closure.
 *
 * A version string alone is not enough: the closure depends on prebuilt native
 * addons, and the `live` patch-reload profile additionally needs internals
 * access. This probe answers all three questions by loading the modules in a
 * child process, so an ABI mismatch fails here instead of halfway through boot.
 *
 * Usage:
 *   node scripts/probe-runtime.mjs [--runtime <dir>] [--node <exe>]
 *                                  [--require-internals] [--json]
 */

import { access, readFile } from 'node:fs/promises'
import { constants } from 'node:fs'
import { execFile } from 'node:child_process'
import { promisify } from 'node:util'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const run = promisify(execFile)

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..')

/** The Node range every dsh release declares (root package.json engines). */
const DEFAULT_REQUIREMENT = '^22.19.0 || >=24.0.0'

/**
 * Does `version` satisfy `range` for the subset dsh uses (`^x.y.z`, `>=x.y.z`,
 * `||`)? Implementing the two operators beats taking a dependency for them.
 * @param {string} version - a `process.versions.node` style value.
 * @param {string} range - the recorded requirement.
 */
export function satisfies(version, range) {
  const parse = (text) => text.split('.').map((part) => Number.parseInt(part, 10))
  const [major, minor, patch] = parse(version)
  const compare = (left, right) => {
    for (let index = 0; index < 3; index += 1) {
      const a = left[index] ?? 0
      const b = right[index] ?? 0
      if (a !== b) return a < b ? -1 : 1
    }
    return 0
  }
  return range.split('||').some((clause) => {
    const term = clause.trim()
    if (term === '') return false
    const caret = term.startsWith('^')
    const gte = term.startsWith('>=')
    const floor = parse(caret || gte ? term.slice(caret ? 1 : 2).trim() : term)
    const candidate = [major, minor, patch]
    if (compare(candidate, floor) < 0) return false
    if (!caret) return true
    // ^x.y.z allows changes that do not modify the left-most non-zero digit.
    if (floor[0] > 0) return major === floor[0]
    if (floor[1] > 0) return major === 0 && minor === floor[1]
    return major === 0 && minor === 0 && patch === floor[2]
  })
}

/** Parse argv; unknown flags fail loudly. */
function parseArgs(argv) {
  const options = { runtime: undefined, node: process.execPath, requireInternals: false, json: false }
  for (let index = 0; index < argv.length; index += 1) {
    const token = argv[index]
    const value = () => {
      const next = argv[index + 1]
      if (next === undefined || next.startsWith('--')) throw new Error(`probe-runtime: ${token} needs a value`)
      index += 1
      return next
    }
    switch (token) {
      case '--runtime': options.runtime = value(); break
      case '--node': options.node = value(); break
      case '--require-internals': options.requireInternals = true; break
      case '--json': options.json = true; break
      default: throw new Error(`probe-runtime: unknown argument ${JSON.stringify(token)}`)
    }
  }
  return options
}

/** The newest prepared runtime, so the probe works without an explicit path. */
async function newestRuntime() {
  const { readdir } = await import('node:fs/promises')
  const dir = join(ROOT, '.dsh-build', 'runtimes')
  const entries = await readdir(dir, { withFileTypes: true }).catch(() => [])
  const candidates = []
  for (const entry of entries) {
    if (!entry.isDirectory()) continue
    const manifestPath = join(dir, entry.name, 'runtime.json')
    const manifest = await readFile(manifestPath, 'utf8').then(JSON.parse, () => undefined)
    if (manifest !== undefined) candidates.push({ dir: join(dir, entry.name), manifest })
  }
  if (candidates.length === 0) {
    throw new Error(`probe-runtime: no prepared runtime under ${dir}; run \`pnpm runtime:prepare\` first`)
  }
  return candidates.sort((left, right) => (left.manifest.dshVersion < right.manifest.dshVersion ? 1 : -1))[0]
}

/**
 * Load every runtime-critical module inside one child process.
 * @param {string} node - the Node executable under test.
 * @param {string} closure - the deployed closure directory.
 * @returns {Promise<{versions: object, modules: object[], internals: object}>}
 */
async function inspect(node, closure) {
  const script = `
    const { createRequire } = require('node:module')
    const path = require('node:path')
    const closure = process.argv[1]
    const requireFromClosure = createRequire(path.join(closure, 'node_modules', '@deepseek-ai', 'dsh', 'lib', 'bin.js'))
    const report = { versions: process.versions, modules: [], internals: { module: false, moduleExports: [], exposeInternalsFlag: process.execArgv.includes('--expose-internals') } }
    for (const name of ['node-pty', 'koffi', 'node-addon-require-builtin']) {
      try {
        const loaded = requireFromClosure(name)
        report.modules.push({ name, ok: true, keys: Object.keys(loaded ?? {}).slice(0, 12) })
        if (name === 'node-addon-require-builtin') {
          // Loading the module is NOT the capability: the addon pattern-matches
          // the running Node binary's machine code, so it can import cleanly and
          // still refuse every id (e.g. a distro-repackaged Node). Exercise it.
          report.internals.moduleExports = Object.keys(loaded ?? {}).slice(0, 12)
          try {
            report.internals.module = typeof loaded.requireBuiltin('internal/modules/esm/loader') === 'object'
          } catch (error) {
            report.internals.module = false
            report.internals.moduleError = String(error && error.message ? error.message : error)
          }
          try {
            report.internals.binding = loaded.getBindingInfo === undefined ? undefined : loaded.getBindingInfo()
          } catch (error) {
            report.internals.binding = String(error && error.message ? error.message : error)
          }
        }
      } catch (error) {
        report.modules.push({ name, ok: false, error: String(error && error.message ? error.message : error) })
      }
    }
    process.stdout.write(JSON.stringify(report))
  `
  try {
    const { stdout } = await run(node, ['-e', script, closure], { maxBuffer: 16 * 1024 * 1024 })
    return JSON.parse(stdout)
  } catch (error) {
    const detail = `${error.stdout ?? ''}${error.stderr ?? ''}`.trim().split('\n').slice(-10).join('\n')
    throw new Error(`probe-runtime: could not inspect the closure with ${node}\n${detail}`)
  }
}

/** The internals path upstream's `live` patch-reload profile requires. */
async function checkInternals(node, closure, report) {
  if (report.internals.module) return { ok: true, how: 'node-addon-require-builtin', binding: report.internals.binding }
  const detail = report.internals.moduleError ?? 'requireBuiltin did not return the internal loader'
  // The flag is an equivalent capability but not the preferred one: a Node that
  // needs it is a different runtime than the one this bundle ships, and that
  // distinction has to survive into the caller's decision.
  try {
    await run(node, ['--expose-internals', '-e', 'process.stdout.write("ok")'], { maxBuffer: 1024 * 1024 })
  } catch (error) {
    return { ok: false, how: 'none', detail, exposeInternalsDetail: `${error.stderr ?? error.message}`.trim().split('\n').slice(-3).join('\n') }
  }
  return { ok: true, how: '--expose-internals', fallback: true, detail }
}

/** Upstream's postinstall chmods node-pty's macOS spawn helper; we install without scripts. */
async function checkSpawnHelper(closure) {
  const helper = join(closure, 'node_modules', 'node-pty', 'prebuilds', `${process.platform}-${process.arch}`, 'spawn-helper')
  try {
    await access(helper, constants.X_OK)
    return { path: helper, present: true, executable: true }
  } catch {
    const present = await access(helper, constants.F_OK).then(() => true, () => false)
    return { path: helper, present, executable: false }
  }
}

async function main() {
  const options = parseArgs(process.argv.slice(2))
  const { dir: closure, manifest } = options.runtime === undefined
    ? await newestRuntime()
    : { dir: resolve(options.runtime), manifest: JSON.parse(await readFile(join(resolve(options.runtime), 'runtime.json'), 'utf8')) }

  const requirement = manifest.node?.requirement ?? DEFAULT_REQUIREMENT
  const report = await inspect(options.node, closure)
  const version = report.versions.node
  const internals = await checkInternals(options.node, closure, report)
  const spawnHelper = await checkSpawnHelper(closure)
  const nativeFailures = report.modules.filter((module) => !module.ok)

  const findings = []
  if (!satisfies(version, requirement)) {
    findings.push({ level: 'error', message: `node ${version} does not satisfy ${requirement}` })
  }
  for (const failure of nativeFailures) {
    findings.push({ level: 'error', message: `native module ${failure.name} failed to load: ${failure.error}` })
  }
  if (!internals.ok) {
    findings.push({
      level: options.requireInternals ? 'error' : 'info',
      message: 'internals access is unavailable (only needed by patchReload: live profiles)',
    })
  } else if (internals.fallback) {
    // Reachable through the flag, but this is not the Node the bundle ships.
    findings.push({
      level: options.requireInternals ? 'error' : 'info',
      message: `the native addon cannot serve this Node; internals would come from --expose-internals (${internals.detail})`,
    })
  }
  if (spawnHelper.present && !spawnHelper.executable) {
    findings.push({ level: 'info', message: `spawn-helper is not executable (upstream postinstall normally chmods it): ${spawnHelper.path}` })
  }

  const result = {
    ok: findings.every((finding) => finding.level !== 'error'),
    runtime: { id: manifest.dshVersion, channel: manifest.channel, dir: closure },
    node: { path: options.node, version, requirement },
    modules: report.modules,
    internals,
    spawnHelper,
    findings,
  }

  if (options.json) {
    process.stdout.write(`${JSON.stringify(result, null, 2)}\n`)
  } else {
    const mark = (level) => (level === 'error' ? 'FAIL' : level === 'info' ? 'note' : 'ok  ')
    console.log(`runtime  ${manifest.channel}-${manifest.dshVersion}  ${closure}`)
    console.log(`node     ${version}  (${options.node})  requires ${requirement}`)
    for (const module of report.modules) {
      console.log(`  ${module.ok ? 'ok  ' : 'FAIL'} ${module.name}${module.ok ? '' : ` — ${module.error}`}`)
    }
    console.log(`  ${internals.ok ? 'ok  ' : 'note'} internals: ${internals.how}${internals.detail === undefined ? '' : ` (${internals.detail})`}`)
    console.log(`  ${spawnHelper.executable ? 'ok  ' : 'note'} spawn-helper executable: ${spawnHelper.executable}`)
    for (const finding of findings) console.log(`  ${mark(finding.level)} ${finding.message}`)
    console.log(result.ok ? 'probe-runtime: OK' : 'probe-runtime: FAILED')
  }
  process.exit(result.ok ? 0 : 1)
}

await main()
