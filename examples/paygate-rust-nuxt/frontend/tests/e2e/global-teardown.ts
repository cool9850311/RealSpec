// Runs once, after every scenario.
//
// There is deliberately very little here. Every scenario's stack.ts stops its
// own containers in a `finally` (fixtures.ts), and `spec.md`, "Isolation" gives
// the rest — the one network the whole run shared (stack.ts's header comment),
// and anything a worker left behind because it died before its `finally`
// ran — to Ryuk, which stays enabled for this suite (playwright.config.ts).
// Unlike examples/minimart-go-nuxt/frontend, which disables Ryuk and sweeps
// its own leftovers here, this file removes nothing: doing so would be
// exactly the hand-rolled lifecycle management that section forbids, and for
// the shared network specifically it could remove something a still-running
// nested run (tests/e2e/harness) or the very next invocation is about to
// reuse (env.ts, `networkStateFile`). tests/e2e/harness/harness.spec.ts is
// where "did Ryuk actually reap everything" is asserted for containers — by
// reading the Docker daemon, never by cleaning it up itself.

import fs from 'node:fs'
import path from 'node:path'
import { ENV, REPORTS_DIR } from './env'

export default function globalTeardown(): void {
  const id = process.env[ENV.runId]
  if (!id) return
  fs.mkdirSync(REPORTS_DIR, { recursive: true })
  fs.writeFileSync(path.join(REPORTS_DIR, 'teardown.json'), `${JSON.stringify({ runId: id }, null, 2)}\n`)
}
