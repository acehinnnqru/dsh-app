/**
 * Produce a release bundle on this machine.
 *
 * The release path is local by choice (design doc §17.5): the runtime closure
 * resolves native packages for the machine that builds it, so a correct artifact
 * can only come from an Apple Silicon Mac, and hosted macOS runners make that
 * awkward.
 *
 * What lands in `--out` is deliberately limited to things that are *portable and
 * immutable*:
 *
 *   dsh-runtime-<version>-<platform>.tar.gz   the updater's payload
 *   <channel>.json                            the channel document
 *   release.json                              what this bundle contains
 *
 * The `.app` is **not** copied there. A store path is read-only, which is
 * exactly right for something you only run, and copying it out to make it
 * writable is a step only *signing* needs. That step is `packaging/export-app.sh`,
 * run when a Developer ID is actually available (design doc §7.2) — doing it here
 * would mean every local packaging run produced a mutable artifact that looks
 * shippable and is not signed.
 *
 * Usage:
 *   node scripts/package-release.mjs [--out dist] [--channel release]
 *                                    [--base-url <url>] [--build-apps]
 */

import { execFile } from 'node:child_process'
import { mkdir, readFile, rm, writeFile } from 'node:fs/promises'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { promisify } from 'node:util'

const run = promisify(execFile)
const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..')

/** Parse argv; unknown flags fail loudly. */
function parseArgs(argv) {
  const options = { out: 'dist', channel: 'release', baseUrl: undefined, buildApps: false }
  for (let index = 0; index < argv.length; index += 1) {
    const token = argv[index]
    const value = () => {
      const next = argv[index + 1]
      if (next === undefined || next.startsWith('--')) throw new Error(`package-release: ${token} needs a value`)
      index += 1
      return next
    }
    switch (token) {
      case '--out': options.out = value(); break
      case '--channel': options.channel = value(); break
      case '--base-url': options.baseUrl = value(); break
      case '--build-apps': options.buildApps = true; break
      default: throw new Error(`package-release: unknown argument ${JSON.stringify(token)}`)
    }
  }
  return options
}

/** Build one flake output and return its store path. */
async function nixBuild(attribute) {
  const { stdout } = await run('nix', ['build', '--no-link', '--print-out-paths', attribute], {
    cwd: ROOT,
    maxBuffer: 64 * 1024 * 1024,
  })
  const path = stdout.trim().split('\n').pop()
  if (path === undefined || path === '') throw new Error(`nix build ${attribute} printed no path`)
  return path
}

async function main() {
  const options = parseArgs(process.argv.slice(2))
  const out = resolve(ROOT, options.out)
  await rm(out, { recursive: true, force: true })
  await mkdir(out, { recursive: true })

  console.log('package-release: building the runtime closure')
  const closure = await nixBuild('.#dsh-runtime')

  console.log('package-release: packaging the runtime archive')
  const packageArgs = [
    join(ROOT, 'scripts', 'package-runtime.mjs'),
    '--runtime', closure,
    '--out', out,
    '--channel', options.channel,
  ]
  if (options.baseUrl !== undefined) packageArgs.push('--base-url', options.baseUrl)
  await run(process.execPath, packageArgs, { cwd: ROOT })

  // Building the apps is a separate concern from packing the runtime, and it is
  // the slow part, so it is opt-in. The paths are recorded either way so a
  // signing step can find them.
  const apps = {}
  for (const variant of ['bundled', 'slim']) {
    if (!options.buildApps) continue
    console.log(`package-release: building the ${variant} app`)
    apps[variant] = join(await nixBuild(`.#dsh-app-${variant}`), 'Applications', 'dsh-app.app')
  }
  if (!options.buildApps) {
    console.log('package-release: skipping the apps (pass --build-apps, or build them where they are used)')
  }

  const runtimeManifest = JSON.parse(await readFile(join(closure, 'runtime.json'), 'utf8'))
  const rootManifest = JSON.parse(await readFile(join(ROOT, 'package.json'), 'utf8'))
  const summary = {
    shellVersion: rootManifest.version,
    dshVersion: runtimeManifest.dshVersion,
    platform: runtimeManifest.platform,
    nodeVersion: rootManifest.dshRuntime.nodeVersion,
    channel: options.channel,
    runtimeArchive: `dsh-runtime-${runtimeManifest.dshVersion}-${runtimeManifest.platform}.tar.gz`,
    channelDocument: `channel-${options.channel}.json`,
    // Read-only store paths, runnable as they are.
    storePaths: apps,
    signed: false,
  }
  await writeFile(join(out, 'release.json'), `${JSON.stringify(summary, null, 2)}\n`, 'utf8')

  console.log(`package-release: wrote ${out}`)
  console.log(`  ${summary.runtimeArchive}`)
  console.log(`  ${summary.channelDocument}`)
  console.log('  release.json')
  for (const [variant, path] of Object.entries(apps)) {
    console.log(`  app(${variant}) -> ${path}`)
  }
  console.log('package-release: the apps are UNSIGNED; see packaging/export-app.sh to sign them (§7.2)')
}

await main()
