/**
 * Record every module a dsh runtime resolves, so a closure can be trimmed from
 * evidence instead of from a guess.
 *
 * Loaded with `node --import <this file>`; see `scripts/trace-runtime.mjs`.
 * The destination comes from `MODULE_TRACE_LOG`, which is deliberately not a
 * `DSH_*` name: the shell hands the child a sanitized environment that drops
 * every `DSH_*` and package-manager variable, and this hook has to survive
 * exactly that path to be worth anything.
 *
 * One JSON object per resolved module, appended when the resolution happens. A
 * buffer flushed on exit would lose the trace whenever the runtime is stopped
 * with a signal, and every trace run ends that way.
 *
 * `registerHooks` (Node >= 22.15) rather than `module.register`: synchronous
 * hooks in the same thread see both `import` and `require`, need no second
 * loader file, and add no async boundary to module loading.
 *
 * @see scripts/trace-runtime.mjs
 */

import { appendFileSync } from 'node:fs'
import { registerHooks } from 'node:module'

const destination = process.env.MODULE_TRACE_LOG

if (destination !== undefined && destination !== '') {
  const seen = new Set()
  registerHooks({
    resolve(specifier, context, nextResolve) {
      const resolved = nextResolve(specifier, context)
      const url = resolved?.url
      if (typeof url === 'string' && !url.startsWith('node:') && !seen.has(url)) {
        seen.add(url)
        try {
          appendFileSync(destination, `${JSON.stringify({ url, specifier, parent: context.parentURL ?? null })}\n`)
        } catch {
          // A trace that cannot be written must not change what is being traced.
        }
      }
      return resolved
    },
  })
}
