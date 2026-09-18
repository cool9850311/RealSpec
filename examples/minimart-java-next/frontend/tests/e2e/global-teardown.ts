// Runs once, after every scenario: removes everything this run created, and
// records what it had to remove.
//
// In a healthy run the only things still standing are the shared network and the
// shared frontend container, because every scenario stops its own stack in a
// `finally`. Anything else found here came from a worker that died before its
// `finally` could run — which is exactly the case this sweep exists for. It
// matches on this run's own label, so it cannot touch another run's containers
// or anything else on the machine.

import fs from 'node:fs'
import path from 'node:path'
import { ENV, REPORTS_DIR } from './env'
import { sweepRunResources } from './docker'

export default async function globalTeardown(): Promise<void> {
  const id = process.env[ENV.runId]
  // A nested run adopts what it did not create, and tears down only the
  // per-scenario stacks, which its scenarios have already done themselves. Its
  // own accounting is asserted by the harness self-test that started it.
  if (!id || process.env[ENV.adoptShared] === '1') return

  const removed = await sweepRunResources(id)
  fs.mkdirSync(REPORTS_DIR, { recursive: true })
  fs.writeFileSync(
    path.join(REPORTS_DIR, 'teardown.json'),
    `${JSON.stringify({ runId: id, removed }, null, 2)}\n`,
  )
}
