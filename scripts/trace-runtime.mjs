/**
 * Trace one runtime boot and report which packages it actually loaded.
 *
 * The closure is a `pnpm deploy --prod` of `@deepseek-ai/dsh`, so it carries
 * every plugin package the harness ships. Which of them load is decided at run
 * time — profile bundles are resolved by name (`dsh-app-boot`'s template map) —
 * which makes a static import graph useless as a pruning oracle. Measured on
 * 0.1.6-alpha.2: a BFS over every `import`/`require` in the closure reached 54
 * of 471 packages, and `@deepseek-ai/dsh-web-frontend` is a declared dependency
 * that is never imported. This script produces what a pruning list needs
 * instead: the modules the boot path really resolves.
 *
 * The child is spawned through `scripts/run-runtime.mjs`, so it gets the same
 * argv and the same sanitized environment the Tauri shell gives it; this script
 * only adds `--import <hook>` to the Node invocation and reads the trace after
 * the run.
 *
 * Usage:
 *   node scripts/trace-runtime.mjs [--runtime <dir>] [--node <exe>]
 *                                  [--profile web] [--out <file>] [--json]
 *
 * `--profile web` is the faithful default: the app's own default preset boots
 * exactly those bundles (see `scripts/lib/profile-template.mjs`).
 *
 * Writes `.dsh-build/module-trace.jsonl` (one line per resolved module) and
 * `.dsh-build/module-trace-report.json`, and prints the human summary.
 *
 * Caveat that must survive into any decision made from this report: a package
 * that never loaded here may still load on a path the boot does not exercise
 * (a lazily imported feature), while a package that did load must stay.
 */

import { mkdir, readFile, readdir, rm, stat, writeFile } from 'node:fs/promises'
import { spawn } from 'node:child_process'
import { fileURLToPath } from 'node:url'
import { dirname, join, resolve, sep } from 'node:path'

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const HOOK = join(ROOT, 'scripts', 'lib', 'module-trace.mjs')
const DEFAULT_TRACE = join(ROOT, '.dsh-build', 'module-trace.jsonl')
const DEFAULT_REPORT = join(ROOT, '.dsh-build', 'module-trace-report.json')

/** Parse argv; unknown flags fail loudly. */
function parseArgs(argv) {
  const options = { runtime: undefined, node: process.execPath, profile: 'web', out: DEFAULT_TRACE, json: false }
  for (let index = 0; index < argv.length; index += 1) {
    const token = argv[index]
    const value = () => {
      const next = argv[index + 1]
      if (next === undefined || next.startsWith('--')) throw new Error(`trace-runtime: ${token} needs a value`)
      index += 1
      return next
    }
    switch (token) {
      case '--runtime': options.runtime = value(); break
      case '--node': options.node = value(); break
      case '--profile': options.profile = value(); break
      case '--out': options.out = resolve(value()); break
      case '--json': options.json = true; break
      default: throw new Error(`trace-runtime: unknown argument ${JSON.stringify(token)}`)
    }
  }
  return options
}

/** The newest prepared runtime, matching the other runtime scripts. */
async function newestRuntime() {
  const dir = join(ROOT, '.dsh-build', 'runtimes')
  const entries = await readdir(dir, { withFileTypes: true }).catch(() => [])
  const candidates = []
  for (const entry of entries) {
    if (!entry.isDirectory()) continue
    const manifest = await readFile(join(dir, entry.name, 'runtime.json'), 'utf8').then(JSON.parse, () => undefined)
    if (manifest !== undefined) candidates.push({ dir: join(dir, entry.name), manifest })
  }
  if (candidates.length === 0) throw new Error(`trace-runtime: no prepared runtime under ${dir}; run \`pnpm runtime:prepare\` first`)
  return candidates.sort((left, right) => (left.manifest.dshVersion < right.manifest.dshVersion ? 1 : -1))[0]
}

/** Every package directory under one closure, with the bytes it occupies. */
async function inventory(closure) {
  const nm = join(closure, 'node_modules')
  const names = []
  for (const entry of await readdir(nm, { withFileTypes: true })) {
    if (!entry.isDirectory()) continue
    if (entry.name.startsWith('@')) {
      for (const scoped of await readdir(join(nm, entry.name), { withFileTypes: true })) {
        if (scoped.isDirectory()) names.push(`${entry.name}/${scoped.name}`)
      }
    } else {
      names.push(entry.name)
    }
  }
  const sized = []
  for (const name of names) sized.push({ name, bytes: await size(join(nm, name)) })
  return sized.sort((left, right) => right.bytes - left.bytes)
}

/** Recursive file bytes; symlinks are followed once, like the closure hash does. */
async function size(dir) {
  let total = 0
  for (const entry of await readdir(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name)
    if (entry.isDirectory()) total += await size(path)
    else if (entry.isFile()) total += (await stat(path)).size
    else if (entry.isSymbolicLink()) total += await stat(path).then((info) => (info.isFile() ? info.size : 0), () => 0)
  }
  return total
}

/** The package a resolved module belongs to, or undefined outside the closure. */
function packageOf(url, closure) {
  let path
  try {
    path = fileURLToPath(url)
  } catch {
    return undefined
  }
  // The last marker wins: a nested copy inside another package's own
  // node_modules is the module that actually loaded.
  const marker = `${join(closure, 'node_modules')}${sep}`
  const at = path.lastIndexOf(marker)
  if (at < 0) return undefined
  const parts = path.slice(at + marker.length).split(sep)
  if (parts[0].startsWith('@')) return parts.length >= 2 ? `${parts[0]}/${parts[1]}` : undefined
  return parts[0]
}

async function main() {
  const options = parseArgs(process.argv.slice(2))
  const closure = options.runtime === undefined ? (await newestRuntime()).dir : resolve(options.runtime)
  const manifest = JSON.parse(await readFile(join(closure, 'runtime.json'), 'utf8'))

  await mkdir(dirname(options.out), { recursive: true })
  await rm(options.out, { force: true })

  console.log(`trace-runtime: runtime ${manifest.channel}-${manifest.dshVersion}`)
  console.log(`trace-runtime: closure ${closure}`)
  console.log(`trace-runtime: profile ${options.profile}`)

  const args = [
    join(ROOT, 'scripts', 'run-runtime.mjs'),
    '--runtime', closure,
    '--node', options.node,
    '--profile', options.profile,
    // Node flags, so they must precede the entry point run-runtime inserts.
    '--node-arg', '--import', '--node-arg', HOOK,
  ]
  const child = spawn(process.execPath, args, {
    cwd: ROOT,
    env: { ...process.env, MODULE_TRACE_LOG: options.out },
    stdio: ['ignore', 'inherit', 'inherit'],
  })
  const outcome = await new Promise((settle) => child.once('exit', (code, signal) => settle({ code, signal })))
  if (outcome.code !== 0) {
    // Not fatal: a partial trace still says which packages the failing boot
    // needed, and that is often the more interesting half.
    console.error(`trace-runtime: the boot did not exit cleanly (code=${outcome.code} signal=${outcome.signal}); reporting the partial trace`)
  }

  const lines = (await readFile(options.out, 'utf8').catch(() => '')).split('\n').filter((line) => line !== '')
  const loaded = new Set()
  for (const line of lines) {
    try {
      const name = packageOf(JSON.parse(line).url, closure)
      if (name !== undefined) loaded.add(name)
    } catch {
      // One unreadable line must not discard the rest of the trace.
    }
  }

  const sizes = await inventory(closure)
  const byName = new Map(sizes.map((entry) => [entry.name, entry.bytes]))
  const kept = sizes.filter((entry) => loaded.has(entry.name))
  const never = sizes.filter((entry) => !loaded.has(entry.name))
  const untraced = [...loaded].filter((name) => !byName.has(name))
  const mb = (bytes) => (bytes / 1024 / 1024).toFixed(1)
  const total = sizes.reduce((sum, entry) => sum + entry.bytes, 0)

  const report = {
    runtime: { id: `${manifest.channel}-${manifest.dshVersion}`, closure },
    profile: options.profile,
    modulesResolved: lines.length,
    loaded: { count: kept.length, bytes: kept.reduce((sum, entry) => sum + entry.bytes, 0), packages: kept.map((entry) => entry.name) },
    neverLoaded: { count: never.length, bytes: never.reduce((sum, entry) => sum + entry.bytes, 0), packages: never.map(({ name, bytes }) => ({ name, bytes })) },
    resolvableFromOutside: untraced,
  }
  await writeFile(DEFAULT_REPORT, `${JSON.stringify(report, null, 2)}\n`, 'utf8')

  if (options.json) {
    process.stdout.write(`${JSON.stringify(report, null, 2)}\n`)
    return
  }
  console.log('')
  console.log(`trace-runtime: ${lines.length} module resolutions, ${loaded.size} packages`)
  console.log(`trace-runtime: loaded      ${String(kept.length).padStart(4)} packages  ${mb(report.loaded.bytes).padStart(7)} MB`)
  console.log(`trace-runtime: never loaded ${String(never.length).padStart(4)} packages  ${mb(report.neverLoaded.bytes).padStart(7)} MB of ${mb(total)} MB`)
  if (untraced.length > 0) console.log(`trace-runtime: ${untraced.length} resolved packages are not in the closure inventory (stale trace?): ${untraced.slice(0, 5).join(', ')}`)
  console.log('')
  console.log('never loaded during this boot, largest first (candidates, not verdicts):')
  for (const entry of never.slice(0, 30)) console.log(`  ${mb(entry.bytes).padStart(7)} MB  ${entry.name}`)
  console.log('')
  console.log(`trace-runtime: report ${DEFAULT_REPORT}`)
  console.log('trace-runtime: a package that never loaded here may still load on a lazy path; a package that loaded must stay')
}

await main()
