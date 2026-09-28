// Everything the e2e suite knows about *where* it is: paths on disk, image
// tags, the literals every scenario's stack is started with, and the names of
// the environment variables the run passes to itself.
//
// Nothing here starts anything. It is the single place a path, a tag or a
// fixture literal is spelled, so that stack.ts and the harness self-tests
// cannot drift apart on one. Mirrors examples/minimart-go-nuxt/frontend/tests/e2e/env.ts;
// see that file for the reasoning behind each shape of constant.

import { fileURLToPath } from 'node:url'
import path from 'node:path'
import fs from 'node:fs'
import os from 'node:os'

/** frontend/tests/e2e */
export const E2E_DIR = path.dirname(fileURLToPath(import.meta.url))

/** examples/paygate-rust-nuxt/frontend */
export const FRONTEND_DIR = path.resolve(E2E_DIR, '../..')

/**
 * examples/paygate-rust-nuxt ($ROOT) — the build context backend/Dockerfile
 * expects (its own header: "Build context is the example root").
 */
export const EXAMPLE_DIR = path.resolve(FRONTEND_DIR, '..')

/** The repository root, where reports/ lives. */
export const REPO_ROOT = path.resolve(EXAMPLE_DIR, '../..')

export const BACKEND_DIR = path.join(EXAMPLE_DIR, 'backend')
export const POSTGRES_MIGRATIONS_DIR = path.join(BACKEND_DIR, 'migrations/postgres')
export const CLICKHOUSE_MIGRATIONS_DIR = path.join(BACKEND_DIR, 'migrations/clickhouse')
export const I18N_DIR = path.join(FRONTEND_DIR, 'i18n')

/** The static build `npm run generate` produces; copied into every scenario's Caddy. */
export const FRONTEND_OUTPUT_DIR = path.join(FRONTEND_DIR, '.output/public')

/**
 * The real reverse-proxy configuration, used unmodified. Every scenario's Caddy
 * gets this exact file copied in, so a proxy that behaved differently under
 * test would be a proxy the tests do not cover.
 */
export const CADDYFILE = path.join(EXAMPLE_DIR, 'local/Caddyfile')
/** Generated, never committed — see local/certs/generate.sh. */
export const CERTS_DIR = path.join(EXAMPLE_DIR, 'local/certs')
export const CERTS_SCRIPT = path.join(CERTS_DIR, 'generate.sh')

/** Where reports, traces, screenshots and container logs are written. */
export const REPORTS_DIR = path.join(REPO_ROOT, 'reports/e2e')

/**
 * The one app image, built once by global-setup and started many times per
 * scenario, one instance per role (`backend/Dockerfile`: "one image, four
 * binaries. The container picks which one by its command."). Tagged
 * separately from backend/apitest's own `paygate:apitest` so the two suites'
 * builds never collide or race each other's cache.
 */
export const APP_IMAGE_NAME = 'paygate'
export const APP_IMAGE_TAG = 'e2e'
export const APP_IMAGE = `${APP_IMAGE_NAME}:${APP_IMAGE_TAG}`

/** Third-party images, pinned to the tags already confirmed present on this machine. */
export const POSTGRES_IMAGE = 'postgres:17-alpine'
export const REDIS_IMAGE = 'redis:7-alpine'
export const KAFKA_IMAGE = 'apache/kafka-native:3.9.0'
export const CLICKHOUSE_IMAGE = 'clickhouse/clickhouse-server:24.8-alpine'
export const CADDY_IMAGE = 'caddy:2-alpine'

/** The database name/user/password every scenario's PostgreSQL uses — backend/apitest/src/stack.rs. */
export const PG_DB = 'paygate'
export const PG_USER = 'paygate'
export const PG_PASSWORD = 'paygate'

/** ClickHouse's default database, used as-is — backend/apitest/src/stack.rs. */
export const CH_DB = 'default'
export const CH_USER = 'default'

export const KAFKA_TOPIC = 'paygate.payment-events.v1'
export const KAFKA_TOPIC_PARTITIONS = 3

/**
 * The Acme Coffee fixture literals every e2e Background seeds (spec/bdd/e2e/*.feature)
 * and every scenario's demo-merchant container is configured with before any
 * Background SQL runs — copied verbatim from backend/apitest/src/stack.rs so
 * the two harnesses never invent a second convention for the same fixture.
 */
export const FIXTURE_MERCHANT_HASH_KEY = 'acmehashkey0123456789abcdef01234'
export const FIXTURE_MERCHANT_HASH_IV = 'acmehashiv012345'
export const FIXTURE_MERCHANT_API_KEY = 'sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc'

/**
 * The address every scenario is served at. Scenarios are told apart by the
 * port their Caddy is published on, so a literal loopback address costs no
 * DNS.
 */
export const SCENARIO_HOST = '127.0.0.1'

/** Overrides workerCount(). */
export const WORKERS_ENV = 'E2E_WORKERS'

/**
 * How many scenarios run at once. spec.md, "Isolation": "half the machine's
 * CPUs" — one worker is a stack of twelve containers, not a thread.
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
 * allocator and the ledger the harness self-test reads back.
 */
export function runStateDir(): string {
  const dir = path.join(REPORTS_DIR, 'run-state', runId())
  fs.mkdirSync(dir, { recursive: true })
  return dir
}

/**
 * Claims the next free scenario index for this run, across every worker
 * process and any nested run sharing the same run id.
 *
 * `mkdir` is the lock: it is atomic and it fails if the directory exists, so
 * the first process to create `seq/<n>` owns `n`. No file locking, no counter
 * file to read-modify-write, and no chance of two scenarios claiming index 3 —
 * which matters here more than it did for minimart, because every scenario's
 * container names are derived from this index and a collision would be a name
 * collision on the Docker daemon, not just a bookkeeping error.
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

/**
 * Where the one Docker network this suite's scenarios join is published
 * (stack.ts, `ensureRunNetwork`/`runNetworkName`).
 *
 * Deliberately NOT inside `runStateDir()`: that directory is keyed by
 * `runId()`, which is different for a nested run started by
 * tests/e2e/harness's own self-test (child-run.ts mints its own run id so its
 * containers' labels stay distinct from the parent's) and different again for
 * the next `npm run test:e2e` invocation entirely — and the network is meant
 * to be found and reused across exactly those boundaries, the way a second
 * run reuses the image `docker build` already produced. One path, outside any
 * single run's own state, that every invocation checks before deciding
 * whether it needs to create anything.
 */
export function networkStateFile(): string {
  const dir = path.join(REPORTS_DIR, 'run-state')
  fs.mkdirSync(dir, { recursive: true })
  return path.join(dir, 'network.json')
}
