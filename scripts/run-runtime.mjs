/**
 * Launch one prepared runtime exactly the way the Tauri shell will: spawn the
 * launcher, read the readiness line from stdout, probe the served index, then
 * shut the child down and report how it exited.
 *
 * This is the executable form of the M1/M2 acceptance check, so it must fail
 * on any deviation rather than degrade quietly.
 *
 * Usage:
 *   node scripts/run-runtime.mjs [--runtime <dir>] [--node <exe>] [--profile web]
 *                                [--home <dir>] [--timeout 60] [--no-probe]
 *                                [--no-shutdown] [-- <extra dsh args>]
 */

import { mkdtemp, readFile, readdir, rm } from 'node:fs/promises'
import { spawn } from 'node:child_process'
import { existsSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..')

/**
 * Pull the authenticated URL out of the readiness line.
 * Tolerant on purpose: upstream appends a ` (LAN: …)` suffix when it detects a
 * LAN address, and the line is a log statement rather than a committed API.
 * @param {string} line - one stdout line.
 * @returns {string | undefined} the absolute URL, when this line announces readiness.
 */
export function parseReadyLine(line) {
  const match = /dsh web:\s+(https?:\/\/\S+)/u.exec(line)
  return match?.[1]
}

/** Parse argv; unknown flags fail loudly. */
function parseArgs(argv) {
  const options = { runtime: undefined, node: process.execPath, profile: 'web', home: undefined, timeout: 60, probe: true, shutdown: true, nodeArgs: [], extra: [] }
  for (let index = 0; index < argv.length; index += 1) {
    const token = argv[index]
    if (token === '--') {
      options.extra = argv.slice(index + 1)
      break
    }
    const value = () => {
      const next = argv[index + 1]
      if (next === undefined || next.startsWith('--')) throw new Error(`run-runtime: ${token} needs a value`)
      index += 1
      return next
    }
    switch (token) {
      case '--runtime': options.runtime = value(); break
      case '--node': options.node = value(); break
      case '--profile': options.profile = value(); break
      case '--home': options.home = value(); break
      case '--timeout': options.timeout = Number(value()); break
      case '--node-arg': {
        const next = argv[index + 1]
        if (next === undefined) throw new Error('run-runtime: --node-arg needs a value')
        options.nodeArgs.push(next)
        index += 1
        break
      }
      case '--no-probe': options.probe = false; break
      case '--no-shutdown': options.shutdown = false; break
      default: throw new Error(`run-runtime: unknown argument ${JSON.stringify(token)}`)
    }
  }
  return options
}

/** The newest prepared runtime. */
async function newestRuntime() {
  const dir = join(ROOT, '.dsh-build', 'runtimes')
  const entries = await readdir(dir, { withFileTypes: true }).catch(() => [])
  const candidates = []
  for (const entry of entries) {
    if (!entry.isDirectory()) continue
    const manifest = await readFile(join(dir, entry.name, 'runtime.json'), 'utf8').then(JSON.parse, () => undefined)
    if (manifest !== undefined) candidates.push({ dir: join(dir, entry.name), manifest })
  }
  if (candidates.length === 0) throw new Error(`run-runtime: no prepared runtime under ${dir}; run \`pnpm runtime:prepare\` first`)
  return candidates.sort((left, right) => (left.manifest.dshVersion < right.manifest.dshVersion ? 1 : -1))[0]
}

/**
 * The environment the shell will hand the child: never inherit NODE_OPTIONS or
 * package-manager state (the Electron shell strips the same names).
 */
function childEnvironment(home) {
  const env = {}
  for (const [name, value] of Object.entries(process.env)) {
    if (name === 'NODE_OPTIONS') continue
    if (/^DSH_/u.test(name)) continue
    if (/^(?:npm|pnpm|corepack)_/iu.test(name)) continue
    if (value === undefined) continue
    env[name] = value
  }
  env.DSH_HOME = home
  return env
}

/**
 * Follow the launch-token exchange without a cookie jar:
 * the index with `?token=` must redirect to the clean path.
 * @param {string} url - the authenticated URL printed by the runtime.
 * @returns {Promise<object>} status facts for the acceptance report.
 */
async function probeIndex(url) {
  const authenticated = await fetch(url, { redirect: 'manual' })
  const location = authenticated.headers.get('location')
  const clean = new URL(url)
  clean.search = ''
  const unauthenticated = await fetch(clean, { redirect: 'manual' })
  return {
    tokenStatus: authenticated.status,
    tokenLocation: location,
    anonymousStatus: unauthenticated.status,
  }
}

async function main() {
  const options = parseArgs(process.argv.slice(2))
  const { dir: closure, manifest } = options.runtime === undefined
    ? await newestRuntime()
    : { dir: resolve(options.runtime), manifest: JSON.parse(await readFile(join(resolve(options.runtime), 'runtime.json'), 'utf8')) }

  const entry = join(closure, manifest.entry.path)
  if (!existsSync(entry)) throw new Error(`run-runtime: manifest entry is missing: ${entry}`)

  const owned = options.home === undefined
  const home = options.home ?? await mkdtemp(join(tmpdir(), 'dsh-app-home-'))
  const args = [...options.nodeArgs, entry, '--profile', options.profile, '--no-open', '--port', '0', ...options.extra]

  console.log(`run-runtime: node ${options.node}`)
  console.log(`run-runtime: entry ${manifest.entry.path}`)
  console.log(`run-runtime: DSH_HOME ${home}`)
  console.log(`run-runtime: argv ${args.join(' ')}`)

  const child = spawn(options.node, args, {
    cwd: closure,
    env: childEnvironment(home),
    stdio: ['pipe', 'pipe', 'pipe'],
  })

  const stderr = []
  child.stderr.setEncoding('utf8')
  child.stderr.on('data', (chunk) => {
    stderr.push(chunk)
    process.stderr.write(chunk)
  })

  const exited = new Promise((settle) => {
    child.once('exit', (code, signal) => settle({ code, signal }))
    child.once('error', (error) => settle({ code: null, signal: null, error }))
  })

  let ready
  const readyPromise = new Promise((settle) => { ready = settle })
  let stdoutBuffer = ''
  child.stdout.setEncoding('utf8')
  child.stdout.on('data', (chunk) => {
    process.stdout.write(chunk)
    stdoutBuffer += chunk
    let index
    while ((index = stdoutBuffer.indexOf('\n')) >= 0) {
      const line = stdoutBuffer.slice(0, index)
      stdoutBuffer = stdoutBuffer.slice(index + 1)
      const url = parseReadyLine(line)
      if (url !== undefined) ready(url)
    }
  })

  // Rejects rather than resolves: a resolved Error would be assigned to `url`
  // and the real failure would surface later as a TypeError, or not at all.
  const timeout = new Promise((_resolve, reject) => {
    const timer = setTimeout(() => reject(new Error(`run-runtime: no readiness line within ${options.timeout}s`)), options.timeout * 1000)
    timer.unref()
  })

  // A child that dies before readiness is the failure this script exists to
  // report, so it must be reported as itself. The timer above is unref'ed and
  // keeps no handle alive, so waiting for it is not merely slower: the process
  // would end on an "unsettled top-level await" warning with the child's own
  // stderr never printed.
  const died = exited.then((outcome) => {
    throw new Error(`run-runtime: the runtime exited before announcing readiness (code=${outcome.code} signal=${outcome.signal})`)
  })
  died.catch(() => {})

  let url
  try {
    url = await Promise.race([readyPromise, died, timeout])
  } catch (error) {
    child.kill('SIGTERM')
    const detail = stderr.join('').trim().split('\n').slice(-20).join('\n')
    console.error(`\nrun-runtime: FAILED — ${error.message}`)
    if (detail !== '') console.error(`--- child stderr ---\n${detail}`)
    await rm(home, { recursive: true, force: true }).catch(() => {})
    process.exit(1)
  }
  console.log(`\nrun-runtime: ready at ${url.replace(/token=[^&]+/u, 'token=<redacted>')}`)

  let probe
  if (options.probe) {
    probe = await probeIndex(url)
    console.log(`run-runtime: index?token= → ${probe.tokenStatus} location=${probe.tokenLocation ?? '(none)'}`)
    console.log(`run-runtime: index anonymous → ${probe.anonymousStatus}`)
  }

  let outcome = { code: null, signal: null }
  if (options.shutdown) {
    child.kill('SIGTERM')
    outcome = await Promise.race([
      exited,
      new Promise((settle) => setTimeout(() => settle({ code: null, signal: 'SIGKILL-timeout' }), 10_000)),
    ])
    if (outcome.signal === 'SIGKILL-timeout') {
      console.error('run-runtime: child ignored SIGTERM for 10s; sending SIGKILL')
      child.kill('SIGKILL')
      outcome = await exited
    }
    console.log(`run-runtime: child exited code=${outcome.code} signal=${outcome.signal}`)
  } else {
    console.log('run-runtime: leaving the child running (--no-shutdown)')
  }

  const failures = []
  if (options.probe) {
    if (probe.tokenStatus !== 303 && probe.tokenStatus !== 302) failures.push(`token exchange returned ${probe.tokenStatus}, expected 302/303`)
    if (probe.anonymousStatus !== 401) failures.push(`anonymous index returned ${probe.anonymousStatus}, expected 401`)
  }
  if (options.shutdown && outcome.code !== 0) failures.push(`clean shutdown expected exit code 0, got code=${outcome.code} signal=${outcome.signal}`)

  if (owned) await rm(home, { recursive: true, force: true }).catch(() => {})
  if (failures.length > 0) {
    for (const failure of failures) console.error(`run-runtime: FAILED — ${failure}`)
    process.exit(1)
  }
  console.log('run-runtime: OK')
}

await main()
