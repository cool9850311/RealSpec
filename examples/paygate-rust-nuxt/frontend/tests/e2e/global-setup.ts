// Runs once, before any scenario: checks the one thing about the machine that
// cannot be worked around, ensures the run's one shared Docker network exists,
// builds the app image, and generates the static front end. `docker build` is
// one of the two things `spec.md`, "Isolation" lets a test do to Docker
// directly, and `nuxt generate` follows the same once-per-run reasoning, both
// done here rather than once per scenario.
//
// Unlike examples/minimart-go-nuxt/frontend, there is no run-wide frontend
// container to start — every scenario copies the generated `.output/public`
// into its own Caddy (stack.ts, build.ts) — but there IS one run-wide Docker
// network, for the reason stack.ts's header comment gives. `ensureRunNetwork`
// runs unconditionally, before the early return below: a nested run started by
// tests/e2e/harness gets its own run id (so its containers' labels stay
// distinct from the parent's — docker.ts, scenarioLabels), and that run id
// picks out a different `runStateDir()`, but the network is looked up by a
// path outside any single run's state (env.ts, `networkStateFile`), so both
// the parent and a nested run resolve to the very same one.

import fs from 'node:fs'
import { ENV, REPORTS_DIR } from './env'
import { buildAppImage, generateCertificate, generateFrontend } from './build'
import { ensureRunNetwork } from './stack'
import { assertBrowserSurvivesContainerChurn } from './preflight'

export default async function globalSetup(): Promise<void> {
  // First, because every later line costs time: a machine that will abort the
  // browser's requests mid-navigation should say so before it builds anything.
  assertBrowserSurvivesContainerChurn()

  fs.mkdirSync(REPORTS_DIR, { recursive: true })

  // Every invocation needs the shared network resolved before any scenario
  // can start — including a nested run, which builds nothing else itself (see
  // the early return below) but still has to know what to join.
  await ensureRunNetwork()

  // A nested run started by tests/e2e/harness reuses the parent's build: it
  // builds nothing itself, only inherits the run id so its own containers are
  // labelled distinctly from the parent's (docker.ts, scenarioLabels).
  if (process.env[ENV.runId]) return

  const id = `${Date.now().toString(36)}${process.pid.toString(36)}`
  process.env[ENV.runId] = id

  // All three are independent, and all three are prerequisites of the first
  // scenario: the image every container runs, the static bundle each Caddy
  // serves, and the certificate each Caddy serves it over.
  await Promise.all([buildAppImage(), generateFrontend(), generateCertificate()])

  fs.writeFileSync(
    `${REPORTS_DIR}/run.json`,
    `${JSON.stringify({ runId: id }, null, 2)}\n`,
  )
}
