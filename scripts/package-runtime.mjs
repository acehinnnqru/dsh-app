/**
 * Package one prepared runtime closure for a release channel.
 *
 * The channel document the shell reads is written by whatever publishes a
 * release, so the archive layout, the platform tag, and the digest must all
 * agree with `channel.rs` — this script is the single producer of all three.
 * Its output is exactly what `.agents`-style review can check: a tarball, its
 * digest, and the JSON entry that names them.
 *
 * Usage:
 *   node scripts/package-runtime.mjs --runtime <dir> --out <dir> [--channel release]
 *
 * Output:
 *   <out>/dsh-runtime-<version>-<platform>.tar.gz
 *   <out>/<channel>.json            (a channel document with this one release)
 */

import { createHash } from 'node:crypto'
import { execFile } from 'node:child_process'
import { chmod, cp, mkdir, mkdtemp, readFile, readdir, rm, writeFile } from 'node:fs/promises'
import { existsSync } from 'node:fs'
import { dirname, join, resolve, basename } from 'node:path'
import { fileURLToPath } from 'node:url'
import { promisify } from 'node:util'

const run = promisify(execFile)
const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..')

/** The shell contract this shell provides; must match `channel::SHELL_CONTRACT`. */
const SHELL_CONTRACT = 1

/**
 * The platform tag releases are published under.
 *
 * Node's spelling, matching `channel::platform_tag` in the shell. A second
 * convention here would make every published archive invisible to the updater.
 * @returns the `darwin-arm64` style tag.
 */
function platformTag() {
  const os = process.platform === 'darwin' ? 'darwin' : process.platform
  const arch = process.arch === 'arm64' ? 'arm64' : 'x64'
  return `${os}-${arch}`
}

/** Parse argv; unknown flags fail loudly. */
function parseArgs(argv) {
  const options = { runtime: undefined, out: undefined, channel: 'release', baseUrl: undefined }
  for (let index = 0; index < argv.length; index += 1) {
    const token = argv[index]
    const value = () => {
      const next = argv[index + 1]
      if (next === undefined || next.startsWith('--')) throw new Error(`package-runtime: ${token} needs a value`)
      index += 1
      return next
    }
    switch (token) {
      case '--runtime': options.runtime = value(); break
      case '--out': options.out = value(); break
      case '--channel': options.channel = value(); break
      case '--base-url': options.baseUrl = value(); break
      default: throw new Error(`package-runtime: unknown argument ${JSON.stringify(token)}`)
    }
  }
  if (options.runtime === undefined) throw new Error('package-runtime: --runtime is required')
  if (options.out === undefined) throw new Error('package-runtime: --out is required')
  return options
}

/** sha256 of one file. */
async function digestOf(path) {
  return createHash('sha256').update(await readFile(path)).digest('hex')
}

/**
 * Recursively add owner-write permission.
 * @param {string} root - directory to widen, in place.
 */
async function makeWritable(root) {
  await chmod(root, 0o755)
  for (const entry of await readdir(root, { withFileTypes: true })) {
    const path = join(root, entry.name)
    if (entry.isDirectory()) {
      await makeWritable(path)
      continue
    }
    if (entry.isFile()) await chmod(path, 0o644)
  }
}

async function main() {
  const options = parseArgs(process.argv.slice(2))
  const runtime = resolve(options.runtime)
  const manifestPath = join(runtime, 'runtime.json')
  if (!existsSync(manifestPath)) {
    throw new Error(`package-runtime: ${runtime} is not a runtime (no runtime.json)`)
  }
  const manifest = JSON.parse(await readFile(manifestPath, 'utf8'))
  if (typeof manifest.dshVersion !== 'string' || manifest.dshVersion === '') {
    throw new Error('package-runtime: runtime.json has no dshVersion')
  }

  const platform = platformTag()
  if (manifest.platform !== undefined && manifest.platform !== platform) {
    throw new Error(
      `package-runtime: the closure is for ${manifest.platform} but this platform is ${platform}`,
    )
  }

  const out = resolve(options.out)
  await mkdir(out, { recursive: true })
  const name = `dsh-runtime-${manifest.dshVersion}-${platform}.tar.gz`
  const archive = join(out, name)

  // Archive the closure under a single version directory, which is the shape
  // `channel::find_runtime_root` accepts (root or one level below).
  const staging = await mkdtemp(join(out, '.staging-'))
  try {
    const versioned = join(staging, manifest.dshVersion)
    await mkdir(versioned, { recursive: true })
    await cp(runtime, versioned, { recursive: true, force: true })
    // A source path under `/nix/store` is read-only, and `fs.cp` does not apply
    // `mode` to directories — only to the files it writes. So the copy inherits
    // 0o555 directories, and the very next step (staging cleanup after tar, or a
    // rebuild over the same path) fails with EACCES. `tar` records the modes it
    // finds, so widening them here cannot leak into the archive's content.
    await makeWritable(versioned)
    await rm(archive, { force: true })
    await run('tar', ['czf', archive, '-C', staging, manifest.dshVersion])
  } finally {
    await rm(staging, { recursive: true, force: true })
  }

  const sha256 = await digestOf(archive)
  const base = options.baseUrl ?? ''
  const document = {
    channel: options.channel,
    releases: [
      {
        version: manifest.dshVersion,
        platform,
        tarball: `${base}${name}`,
        sha256,
        shellContract: SHELL_CONTRACT,
      },
    ],
  }
  // `channel-` prefix on purpose: a bare `<channel>.json` collides with the
  // release summary `package-release.mjs` writes beside it, and the collision
  // silently overwrote the document the updater needs.
  const documentPath = join(out, `channel-${options.channel}.json`)
  await writeFile(documentPath, `${JSON.stringify(document, null, 2)}\n`, 'utf8')

  console.log(`package-runtime: ${basename(archive)} (${platform})`)
  console.log(`package-runtime: sha256 ${sha256}`)
  console.log(`package-runtime: ${basename(documentPath)}`)
}

await main()
