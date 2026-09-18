// What the whole run shares: two freshly built images, one network, and one
// frontend container on it. Every scenario's containers join that same network
// (stack.ts).
//
// The network is created here, before any test runs, and removed after the last
// one, so no bridge appears or disappears while a browser is running. That is
// not tidiness: a bridge is given its subnet's IPv4 gateway address when it is
// created and loses it when it is removed, and Chromium aborts every HTTP/2
// request in flight when an address comes or goes in its namespace. See
// stack.ts and tests/e2e/preflight.ts for the other half of the same problem.
//
// PLAN §4: the front end is a Next.js static export — `next build` with
// `output: 'export'`, whose `out/` directory `serve` hands out as plain files —
// its API base is the relative path `/api`, and the image build's
// `npm run check:bundle` (frontend/scripts) fails if a hostname, or anything that
// needs a Next.js server, reaches `out/`. A bundle that carries no origin cannot
// notice which origin it was loaded from, so ONE container serves every
// scenario, each on its own published port, with no interference to arrange and
// no per-scenario build to pay for.
//
// Everything else — PostgreSQL, the backend, Caddy — is per scenario. See
// stack.ts.

import { execFile } from 'node:child_process'
import { promisify } from 'node:util'
import { GenericContainer, type StartedTestContainer, Wait } from 'testcontainers'
import {
  BACKEND_IMAGE,
  EXAMPLE_DIR,
  FRONTEND_IMAGE,
  REPORTS_DIR,
} from './env'
import { createNetwork, runLabels } from './docker'
import fs from 'node:fs'
import path from 'node:path'

const run = promisify(execFile)

/** The alias every scenario's Caddy resolves the front end by. */
export const FRONTEND_ALIAS = 'frontend'
export const FRONTEND_PORT = 3000

export interface SharedInfrastructure {
  networkName: string
  frontend: StartedTestContainer
}

/**
 * Builds both images exactly the way local/docker-compose.yml does: the context
 * is examples/minimart-java-next and the Dockerfile is <service>/Dockerfile.
 *
 * The backend's build compiles the Maven project inside the image (the build
 * stage runs `mvnw package`), so this is also where a compile error in the
 * service surfaces — in build-backend.log, before any browser starts.
 *
 * Once per run, before any scenario starts, because a per-scenario container
 * must be a start and not a build — that is what makes full isolation cost
 * seconds rather than minutes. It is also the step that makes a green run mean
 * something: an image left over from the last change would test the last
 * change.
 */
export async function buildImages(): Promise<void> {
  fs.mkdirSync(REPORTS_DIR, { recursive: true })
  await Promise.all([
    buildImage('backend/Dockerfile', BACKEND_IMAGE, 'build-backend.log'),
    buildImage('frontend/Dockerfile', FRONTEND_IMAGE, 'build-frontend.log'),
  ])
}

async function buildImage(dockerfile: string, tag: string, logName: string): Promise<void> {
  const logFile = path.join(REPORTS_DIR, logName)
  try {
    const { stdout, stderr } = await run(
      'docker',
      ['build', '-f', path.join(EXAMPLE_DIR, dockerfile), '-t', tag, EXAMPLE_DIR],
      { maxBuffer: 64 * 1024 * 1024 },
    )
    fs.writeFileSync(logFile, stdout + stderr)
  } catch (error) {
    const failure = error as { stdout?: string; stderr?: string; message?: string }
    fs.writeFileSync(logFile, (failure.stdout ?? '') + (failure.stderr ?? '') + (failure.message ?? ''))
    throw new Error(`docker build -f ${dockerfile} -t ${tag} failed; see ${logFile}`)
  }
}

/** Creates the run's one network and starts the shared frontend container on it. */
export async function startSharedInfrastructure(runId: string): Promise<SharedInfrastructure> {
  const networkName = await createNetwork(`realspec-e2e-shared-${runId}`, runId)
  const frontend = await new GenericContainer(FRONTEND_IMAGE)
    .withLabels(runLabels(runId))
    .withNetworkMode(networkName)
    .withNetworkAliases(FRONTEND_ALIAS)
    .withExposedPorts(FRONTEND_PORT)
    // `serve` takes its address from the image's CMD (`--listen 3000`), not from
    // these; they are kept so that an image whose server does read HOST/PORT
    // still binds where Caddy looks for it.
    .withEnvironment({ HOST: '0.0.0.0', PORT: String(FRONTEND_PORT) })
    .withWaitStrategy(Wait.forHttp('/', FRONTEND_PORT))
    .start()
  return { networkName, frontend }
}
