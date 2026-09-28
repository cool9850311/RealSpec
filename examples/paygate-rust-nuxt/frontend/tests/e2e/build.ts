// The two things this suite is allowed to do before any scenario starts, and
// nowhere else: `docker build`, once (one of the two things `spec.md`,
// "Isolation" lets a test do to Docker directly), and `nuxt generate`, once,
// because the front end is a static SPA one prebuilt artefact serves for every
// scenario (nuxt.config.ts's header comment).
//
// Both are idempotent enough to re-run by hand, but global-setup.ts calls each
// exactly once per `playwright test` invocation, which is what makes a green
// run mean something: an image or a bundle left over from the last change
// would test the last change.

import fs from 'node:fs'
import path from 'node:path'
import { execFile } from 'node:child_process'
import { promisify } from 'node:util'
import {
  APP_IMAGE,
  BACKEND_DIR,
  CERTS_DIR,
  CERTS_SCRIPT,
  EXAMPLE_DIR,
  FRONTEND_DIR,
  FRONTEND_OUTPUT_DIR,
  REPORTS_DIR,
} from './env'

const run = promisify(execFile)

/**
 * `docker build -f backend/Dockerfile -t paygate:e2e $ROOT`.
 *
 * Context is the example root, exactly as backend/Dockerfile's own header
 * comment requires ("Build context is the example root: docker build -f
 * backend/Dockerfile ."), and exactly what backend/apitest/src/stack.rs does
 * for its own `paygate:apitest` tag — a second, unrelated tag so the two
 * suites' builds never collide or race each other's layer cache.
 */
export async function buildAppImage(): Promise<void> {
  fs.mkdirSync(REPORTS_DIR, { recursive: true })
  const logFile = path.join(REPORTS_DIR, 'build-backend.log')
  const dockerfile = path.join(BACKEND_DIR, 'Dockerfile')
  try {
    const { stdout, stderr } = await run(
      'docker',
      ['build', '-f', dockerfile, '-t', APP_IMAGE, EXAMPLE_DIR],
      { maxBuffer: 64 * 1024 * 1024 },
    )
    fs.writeFileSync(logFile, stdout + stderr)
  } catch (error) {
    const failure = error as { stdout?: string; stderr?: string; message?: string }
    fs.writeFileSync(logFile, (failure.stdout ?? '') + (failure.stderr ?? '') + (failure.message ?? ''))
    throw new Error(`docker build -f ${dockerfile} -t ${APP_IMAGE} failed; see ${logFile}`)
  }
}

/**
 * `npm run generate` — a client-only SPA build (nuxt.config.ts: `ssr: false`)
 * whose bundle carries no hostname, so the one output directory is copied,
 * unmodified, into every scenario's own Caddy container (stack.ts).
 *
 * `npm run generate` already runs `check:bundle` (package.json), so a bundle
 * that leaked a hostname fails here rather than inside a container where the
 * failure would be a blank page and a confusing trace.
 */
export async function generateFrontend(): Promise<void> {
  fs.mkdirSync(REPORTS_DIR, { recursive: true })
  const logFile = path.join(REPORTS_DIR, 'build-frontend.log')
  try {
    const { stdout, stderr } = await run('npm', ['run', 'generate'], {
      cwd: FRONTEND_DIR,
      maxBuffer: 64 * 1024 * 1024,
    })
    fs.writeFileSync(logFile, stdout + stderr)
  } catch (error) {
    const failure = error as { stdout?: string; stderr?: string; message?: string }
    fs.writeFileSync(logFile, (failure.stdout ?? '') + (failure.stderr ?? '') + (failure.message ?? ''))
    throw new Error(`npm run generate failed; see ${logFile}`)
  }
  if (!fs.existsSync(FRONTEND_OUTPUT_DIR)) {
    throw new Error(`npm run generate did not produce ${FRONTEND_OUTPUT_DIR}`)
  }
}

/**
 * Writes the TLS certificate Caddy serves, by running the example's own script.
 *
 * Idempotent: an existing certificate that still covers `127.0.0.1` and has not
 * expired is left alone. It is generated rather than committed because a private
 * key does not belong in a repository, and because a committed certificate
 * expires one day into a TLS handshake failure nobody connects back to a file
 * checked in years earlier — the same reasoning, and the same script, as
 * `examples/minimart-go-nuxt`.
 */
export async function generateCertificate(): Promise<void> {
  fs.mkdirSync(REPORTS_DIR, { recursive: true })
  const logFile = path.join(REPORTS_DIR, 'certs.log')
  try {
    const { stdout, stderr } = await run(CERTS_SCRIPT, [], { cwd: EXAMPLE_DIR })
    fs.writeFileSync(logFile, stdout + stderr)
  } catch (error) {
    const failure = error as { stdout?: string; stderr?: string; message?: string }
    fs.writeFileSync(logFile, (failure.stdout ?? '') + (failure.stderr ?? '') + (failure.message ?? ''))
    throw new Error(`${CERTS_SCRIPT} failed; see ${logFile}`)
  }
  for (const file of ['cert.pem', 'key.pem']) {
    const at = path.join(CERTS_DIR, file)
    if (!fs.existsSync(at)) {
      throw new Error(`${CERTS_SCRIPT} finished but did not write ${at}`)
    }
  }
}
