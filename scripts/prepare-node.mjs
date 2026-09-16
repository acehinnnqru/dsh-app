/**
 * Fetch the official Node.js runtime the shell bundles.
 *
 * The archive comes from nodejs.org and is verified against that release's own
 * `SHASUMS256.txt`, so the pinned version plus the upstream manifest is the
 * whole trust chain — no hash is copied into this repository where it could rot
 * unnoticed.
 *
 * Why the official build specifically: the harness needs `require-builtin`
 * access for `patchReload: live` profiles, and `node-addon-require-builtin`
 * works by matching the running binary's machine code. A distribution-repackaged
 * Node (Nix's `nodejs-slim`, for example) can load the addon and still refuse
 * every id, so only an official archive is a supported runtime.
 *
 * Output: .dsh-build/node/<version>/bin/node
 *
 * Usage:
 *   node scripts/prepare-node.mjs [--version 24.17.0] [--force]
 */

import { createHash } from 'node:crypto'
import { execFile } from 'node:child_process'
import { mkdir, mkdtemp, rename, rm, writeFile } from 'node:fs/promises'
import { existsSync } from 'node:fs'
import { join, resolve, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'
import { promisify } from 'node:util'

const run = promisify(execFile)
const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const NODE_ROOT = join(ROOT, '.dsh-build', 'node')

/** The Node release the shell bundles; the Electron shell pins the same major. */
const DEFAULT_VERSION = '24.17.0'

/** Parse argv; unknown flags fail loudly. */
function parseArgs(argv) {
  const options = { version: DEFAULT_VERSION, force: false }
  for (let index = 0; index < argv.length; index += 1) {
    const token = argv[index]
    const value = () => {
      const next = argv[index + 1]
      if (next === undefined || next.startsWith('--')) throw new Error(`prepare-node: ${token} needs a value`)
      index += 1
      return next
    }
    switch (token) {
      case '--version': options.version = value(); break
      case '--force': options.force = true; break
      default: throw new Error(`prepare-node: unknown argument ${JSON.stringify(token)}`)
    }
  }
  return options
}

/** The official archive name for this machine. */
function archiveName(version) {
  if (process.platform !== 'darwin') {
    throw new Error(`prepare-node: ${process.platform} is not a supported target yet (macOS only)`)
  }
  return `node-v${version}-${process.platform}-${process.arch}.tar.gz`
}

/** Download a URL into memory. */
async function download(url) {
  const response = await fetch(url)
  if (!response.ok) throw new Error(`prepare-node: GET ${url} -> ${response.status} ${response.statusText}`)
  return Buffer.from(await response.arrayBuffer())
}

/** The expected sha256 for one archive, from the release's own manifest. */
async function expectedDigest(version, archive) {
  const manifestUrl = `https://nodejs.org/dist/v${version}/SHASUMS256.txt`
  const manifest = (await download(manifestUrl)).toString('utf8')
  for (const line of manifest.split('\n')) {
    const [digest, name] = line.trim().split(/\s+/u)
    if (name === archive) return digest
  }
  throw new Error(`prepare-node: ${archive} is absent from ${manifestUrl}`)
}

async function main() {
  const options = parseArgs(process.argv.slice(2))
  const archive = archiveName(options.version)
  const target = join(NODE_ROOT, options.version)
  const nodeBinary = join(target, 'bin', 'node')

  if (existsSync(nodeBinary) && !options.force) {
    const { stdout } = await run(nodeBinary, ['--version'])
    console.log(`prepare-node: reusing ${nodeBinary} (${stdout.trim()})`)
    console.log('prepare-node: pass --force to re-download')
    return
  }

  await mkdir(NODE_ROOT, { recursive: true })
  const staging = await mkdtemp(join(NODE_ROOT, `.staging-${options.version}-`))
  console.log(`prepare-node: fetching ${archive}`)

  try {
    const url = `https://nodejs.org/dist/v${options.version}/${archive}`
    const expected = await expectedDigest(options.version, archive)
    const tarball = await download(url)
    const actual = createHash('sha256').update(tarball).digest('hex')
    if (actual !== expected) {
      throw new Error(`prepare-node: ${archive} digest mismatch\n  expected ${expected}\n  actual   ${actual}`)
    }
    console.log(`prepare-node: sha256 ${actual} (verified against SHASUMS256.txt)`)
    await writeFile(join(staging, archive), tarball)

    console.log('prepare-node: extracting runtime/ (docs, include, headers excluded)')
    await run('tar', [
      'xzf', join(staging, archive),
      '-C', staging,
      '--strip-components=1',
      '--exclude', '*/include',
      '--exclude', '*/share',
      '--exclude', '*/CHANGELOG.md',
      '--exclude', '*/README.md',
    ])

    const extracted = join(staging, 'bin', 'node')
    if (!existsSync(extracted)) {
      throw new Error(`prepare-node: ${archive} did not contain bin/node`)
    }
    const { stdout } = await run(extracted, ['--version'])
    console.log(`prepare-node: archive reports ${stdout.trim()}`)
    if (!stdout.trim().endsWith(`v${options.version}`)) {
      throw new Error(`prepare-node: archive runs ${stdout.trim()}, expected v${options.version}`)
    }

    if (existsSync(target)) await rm(target, { recursive: true, force: true })
    await rename(staging, target)
    console.log(`prepare-node: published ${target}`)
  } finally {
    await rm(staging, { recursive: true, force: true }).catch(() => {})
  }
}

await main()
