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
 * Usage:
 *   node scripts/prepare-runtime.mjs [--version <v>] [--channel release] [--force] [--keep-staging]
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
const BUILD = join(ROOT, '.dsh-build')
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
  const options = { version: undefined, channel: 'release', force: false, keepStaging: false }
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

async function main() {
  const options = parseArgs(process.argv.slice(2))
  const pin = await readPin()
  const version = options.version ?? pin.version
  const platform = platformKey()
  const id = `${options.channel}-${version}`
  const output = join(RUNTIMES, id)

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
  const deploy = join(staging, 'closure')
  console.log(`prepare-runtime: staging ${relative(ROOT, staging)}`)

  try {
    const spec = `${pin.package}@${version}`
    await mkdir(project, { recursive: true })
    await writeFile(join(staging, 'pnpm-workspace.yaml'), "packages:\n  - 'runtime'\n", 'utf8')
    await writeFile(join(project, 'package.json'), `${JSON.stringify({
      name: projectName,
      private: true,
      version: '0.0.0',
      description: `${pin.package} production closure for the dsh-app desktop shell`,
    }, null, 2)}\n`, 'utf8')

    console.log(`prepare-runtime: adding ${spec} (exact, scripts disabled)`)
    await exec('pnpm', ['--filter', projectName, 'add', '--save-exact', '--ignore-scripts', spec], { cwd: staging })

    console.log('prepare-runtime: deploying production closure (hoisted, links materialized)')
    await exec('pnpm', [
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

    await materializeLinks(deploy)
    await removeBinDirs(deploy)
    const { entry, dshVersion } = await assertEntrypoint(deploy)

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
