// Everything the e2e suite knows about *where* it is: paths on disk, image
// tags, the names of the environment variables the run passes to itself, and
// the per-run directory the workers use to coordinate.
//
// Nothing here starts anything. It is the single place a path or a tag is
// spelled, so that stack.ts, shared.ts and the harness self-tests cannot drift
// apart on one.

import { fileURLToPath } from 'node:url'
import path from 'node:path'
import fs from 'node:fs'
import os from 'node:os'

/** frontend/tests/e2e */
export const E2E_DIR = path.dirname(fileURLToPath(import.meta.url))

/** examples/minimart-java-next/frontend */
export const FRONTEND_DIR = path.resolve(E2E_DIR, '../..')

/**
 * examples/minimart-java-next — the build context both Dockerfiles expect, the
 * same one local/docker-compose.yml uses (`context: ..`).
 */
export const EXAMPLE_DIR = path.resolve(FRONTEND_DIR, '..')

/** The repository root, where reports/ lives. */
export const REPO_ROOT = path.resolve(EXAMPLE_DIR, '../..')

export const BACKEND_DIR = path.join(EXAMPLE_DIR, 'backend')
/**
 * Flyway's location, `classpath:db/migration`, as it sits in the source tree.
 * The service applies these same files through Flyway when SCHEMA_AUTO_MIGRATE
 * is on; `run migration` applies them here, in Flyway's version order (see
 * migrationFiles() in stack.ts), so both paths create the same schema.
 */
export const MIGRATIONS_DIR = path.join(BACKEND_DIR, 'src/main/resources/db/migration')
export const I18N_DIR = path.join(FRONTEND_DIR, 'i18n')

/**
 * The real reverse-proxy configuration, used unmodified. Every scenario starts
 * its own Caddy from this file, which is the file `docker compose up` mounts —
 * a proxy that behaved differently under test would be a proxy the tests do not
 * cover.
 */
export const CADDYFILE = path.join(EXAMPLE_DIR, 'local/Caddyfile')
export const CERTS_DIR = path.join(EXAMPLE_DIR, 'local/certs')

/**
 * Where reports, traces, screenshots and container logs are written.
 *
 * Not reports/e2e: that is minimart-go-nuxt's, and both suites write run.json,
 * teardown.json, the build logs and the HTML report under fixed names. Sharing
 * the directory would let whichever ran last overwrite the other's evidence.
 */
export const REPORTS_DIR = path.join(REPO_ROOT, 'reports/e2e-java-next')

/** Images built once per run by global-setup and started many times per run. */
export const BACKEND_IMAGE = 'minimart-java-backend:e2e'
export const FRONTEND_IMAGE = 'minimart-next-frontend:e2e'

/** Third-party images, pinned to the tags local/docker-compose.yml uses. */
export const POSTGRES_IMAGE = 'postgres:16-alpine'
export const CADDY_IMAGE = 'caddy:2-alpine'

export const DB_NAME = 'minimart'
export const DB_USER = 'minimart'
export const DB_PASSWORD = 'minimart_secret'

/**
 * This suite's signing key. It is deliberately not the deployment's: a test
 * that passes only because it shares production's secret is not a test.
 *
 * At least 32 bytes (this one is 38), unlike minimart-go-nuxt's. The service
 * signs and verifies HS256 through Nimbus, which refuses an HMAC key shorter
 * than the hash output, and AppConfig checks the length at boot so that a short
 * key fails the container's start with the variable named rather than the first
 * login with a stack trace (spec.md, "Deliberate differences from
 * minimart-go-nuxt"). A shorter value
 * here would leave every scenario waiting on a backend that never listens.
 */
export const JWT_SECRET = 'minimart-e2e-secret-0123456789abcdef01'

/**
 * The label every container and network this suite creates carries, so that
 * teardown can find its own leftovers and nothing else on the machine.
 *
 * The same key minimart-go-nuxt's suite uses, on purpose. What a sweep matches
 * is `key=<run id>`, and a run id is minted per invocation from the clock and
 * the process id (global-setup.ts), so two suites running side by side already
 * cannot touch each other's containers. One key keeps a single
 * `docker ps --filter label=realspec.e2e.run` answering "is any e2e run leaving
 * things behind", whichever example it came from.
 */
export const RUN_LABEL = 'realspec.e2e.run'

/**
 * The address every scenario is served at. Scenarios are told apart by the port
 * their proxy is published on, so a literal loopback address costs no DNS.
 *
 * There is no fixture-password constant here. The features seed a bcrypt hash
 * and type the password that produced it into the login form, so the value is
 * visible in the test that uses it rather than configurable out from under it.
 */
export const SCENARIO_HOST = '127.0.0.1'

/** Overrides workerCount(). The API runner's CUCUMBER_CONCURRENCY, for browsers. */
export const WORKERS_ENV = 'E2E_WORKERS'

/**
 * How many scenarios run at once. One worker is a stack of containers, not a
 * thread, so: half the CPUs, never below one. A fixed 4 oversubscribes the
 * 2-vCPU runner a private repository gets.
 */
export function workerCount(cpus: number = os.availableParallelism()): number {
  const override = Number(process.env[WORKERS_ENV])
  if (Number.isInteger(override) && override >= 1) return override
  return Math.max(1, Math.floor(cpus / 2))
}

/**
 * Environment variables the run sets for itself. global-setup writes them;
 * worker processes and any nested run started by the harness self-tests read
 * them.
 */
export const ENV = {
  /** Identifies one `playwright test` invocation and everything it created. */
  runId: 'E2E_RUN_ID',
  /** The run's one Docker network: the shared frontend and every scenario. */
  sharedNetwork: 'E2E_SHARED_NETWORK',
  /** Set by a nested run: adopt the shared infrastructure, do not own it. */
  adoptShared: 'E2E_ADOPT_SHARED',
  /** Set by a nested run: the single project to run (see playwright.config.ts). */
  childProject: 'E2E_CHILD_PROJECT',
} as const

/** The id of the current run, whether this process created it or inherited it. */
export function runId(): string {
  const existing = process.env[ENV.runId]
  if (existing) return existing
  throw new Error(
    `${ENV.runId} is not set. It is assigned by tests/e2e/global-setup.ts; ` +
      'this module must not be imported outside a Playwright run.',
  )
}

/**
 * The directory workers of this run use to coordinate: the scenario-index
 * allocator and the ledger of ports the harness self-test reads back.
 */
export function runStateDir(): string {
  const dir = path.join(REPORTS_DIR, 'run-state', runId())
  fs.mkdirSync(dir, { recursive: true })
  return dir
}

/**
 * Claims the next free scenario index for this run, across every worker process
 * and any nested run sharing the same run id.
 *
 * `mkdir` is the lock: it is atomic and it fails if the directory exists, so the
 * first process to create `seq/<n>` owns `n`. No file locking, no counter file
 * to read-modify-write, and no chance of two scenarios claiming index 3.
 */
export function allocateScenarioIndex(): number {
  const seqDir = path.join(runStateDir(), 'seq')
  fs.mkdirSync(seqDir, { recursive: true })
  for (let index = 1; index < 100_000; index++) {
    try {
      fs.mkdirSync(path.join(seqDir, String(index)))
      return index
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== 'EEXIST') throw error
    }
  }
  throw new Error('scenario index allocator exhausted')
}
