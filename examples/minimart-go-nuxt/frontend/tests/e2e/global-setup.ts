// Runs once, before any scenario: checks the one thing about the machine that
// cannot be worked around, builds the two images, and starts the one thing
// every scenario shares.
//
// Environment variables set here reach the worker processes, which Playwright
// spawns afterwards. That is how a worker learns the run id and the name of the
// shared network without a file to read or a port to negotiate.

import fs from 'node:fs'
import path from 'node:path'
import { execFileSync } from 'node:child_process'
import { CERTS_DIR, ENV, REPORTS_DIR } from './env'
import { buildImages, startSharedInfrastructure } from './shared'
import { assertBrowserSurvivesContainerChurn } from './preflight'

// The certificate every scenario's Caddy serves is generated, never committed:
// a private key does not belong in a repository, and mkcert names its CA after
// the account and machine that made it. local/certs/.gitignore keeps the output
// out, and this makes a clean clone work on the first `npm run test:e2e`
// instead of failing inside a container with a missing-file error that points
// nowhere useful.
//
// The script is idempotent, so this costs one `openssl x509` when the
// certificate is already there.
function ensureCertificate(): void {
  try {
    execFileSync(path.join(CERTS_DIR, 'generate.sh'), [], { stdio: 'inherit' })
  } catch (error) {
    throw new Error(
      `could not generate the TLS certificate in ${CERTS_DIR}: ${error instanceof Error ? error.message : String(error)}\n` +
        'Run local/certs/generate.sh by hand to see why; it needs openssl, and prefers mkcert when present.',
    )
  }
}

export default async function globalSetup(): Promise<void> {
  // First, because every later line costs time: a machine that will abort the
  // browser's requests mid-navigation should say so before it builds an image.
  assertBrowserSurvivesContainerChurn()

  fs.mkdirSync(REPORTS_DIR, { recursive: true })
  ensureCertificate()

  // A nested run started by tests/e2e/harness adopts the shared infrastructure
  // of the run that started it: same network, same frontend container, its own
  // run id so that its own containers can be accounted for separately. It
  // builds nothing and owns nothing, and global-teardown leaves it all alone.
  if (process.env[ENV.adoptShared] === '1') {
    requireEnv(ENV.runId)
    requireEnv(ENV.sharedNetwork)
    return
  }

  const id = `${Date.now().toString(36)}${process.pid.toString(36)}`
  process.env[ENV.runId] = id

  await buildImages()
  const shared = await startSharedInfrastructure(id)
  process.env[ENV.sharedNetwork] = shared.networkName

  fs.writeFileSync(
    path.join(REPORTS_DIR, 'run.json'),
    `${JSON.stringify(
      { runId: id, sharedNetwork: shared.networkName, frontendContainer: shared.frontend.getId() },
      null,
      2,
    )}\n`,
  )
}

function requireEnv(name: string): void {
  if (!process.env[name]) {
    throw new Error(`${name} must be set when ${ENV.adoptShared}=1`)
  }
}
