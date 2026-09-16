// One scenario's infrastructure: its own PostgreSQL, backend and Caddy, all on
// the run's single shared network (created once in shared.ts).
//
//   shared network — one per run
//     frontend                                        prebuilt, static, shared
//     postgres-<run>-<n>  backend-<run>-<n>  caddy-<run>-<n>      per scenario
//
//   browser context, baseURL = https://127.0.0.1:<mapped port>
//
// Isolation rests on separate containers, not separate networks: each scenario
// has its own database, its own backend and its own origin — the port is what
// carries the cookie jar. What is given up is network-level separation: any
// scenario's containers can address any other's by alias, so aliases carry the
// run id and the scenario index to keep them apart.
//
// Creating a bridge is what is being avoided, and the reason is measured
// (spec.md, "Isolation"): a new bridge is given the subnet's IPv4 gateway
// address, and Chromium — in the same network namespace on a Linux runner —
// reads any address appearing or disappearing as the network having changed and
// aborts every HTTP/2 request in flight. Unlike the IPv6 link-local address a
// veth gets, which tests/e2e/preflight.ts requires the host to suppress, this
// one cannot be turned off: Docker needs it. So the run creates its one network
// before the browser exists and removes it after the browser is gone, and no
// bridge appears or disappears in between.

import https from 'node:https'
import fs from 'node:fs'
import path from 'node:path'
import pg from 'pg'
import { GenericContainer, type StartedTestContainer, Wait } from 'testcontainers'
import {
  CADDYFILE,
  CADDY_IMAGE,
  CERTS_DIR,
  DB_NAME,
  DB_PASSWORD,
  DB_USER,
  ENV,
  JWT_SECRET,
  MIGRATIONS_DIR,
  POSTGRES_IMAGE,
  BACKEND_IMAGE,
  SCENARIO_HOST,
  allocateScenarioIndex,
  runId,
  runStateDir,
} from './env'
import { containerLogs, runLabels } from './docker'
import { FRONTEND_ALIAS, FRONTEND_PORT } from './shared'

/**
 * The infrastructure profile a scenario's tags select (spec.md, "Isolation").
 *
 * minimart has no separate auth service, so the tag selects CONFIGURATION
 * rather than extra containers. The mechanism being proved portable is
 * "tag → infrastructure profile", not the particular containers: the original project's
 * `@stack:auth` adds a service and a database, and the same dispatch reads it.
 */
export type StackProfile = 'default' | 'auth'

export const STACK_TAG_PREFIX = '@stack:'

/**
 * Reads the profile out of a scenario's tags.
 *
 * An unknown `@stack:` tag is an error rather than a silent fall back to the
 * default: a feature that asks for infrastructure this harness does not have
 * must not quietly run against something else.
 */
export function profileFromTags(tags: readonly string[]): StackProfile {
  const stackTags = tags.filter((tag) => tag.startsWith(STACK_TAG_PREFIX))
  if (stackTags.length === 0) return 'default'
  if (stackTags.length > 1) {
    throw new Error(`a scenario may carry at most one ${STACK_TAG_PREFIX} tag, found: ${stackTags.join(', ')}`)
  }
  const name = stackTags[0].slice(STACK_TAG_PREFIX.length)
  if (name === 'auth') return 'auth'
  throw new Error(
    `unknown infrastructure profile ${stackTags[0]}; stack.ts knows ${STACK_TAG_PREFIX}auth and the untagged default`,
  )
}

export interface ScenarioStack {
  /** 1-based index of this scenario within the run. */
  readonly index: number
  readonly profile: StackProfile
  /** The host port Caddy's 443 is published on — this scenario's own address. */
  readonly port: number
  /** `https://127.0.0.1:<port>` — the browser context's base URL. */
  readonly baseURL: string
  /** This scenario's database, connected directly, bypassing the backend. */
  readonly db: pg.Client
  /** Applies backend/migrations to this scenario's database. */
  migrate(): Promise<void>
  /** Every container's last lines of output, for a failed scenario's artefact. */
  collectLogs(): Promise<Record<string, string>>
  stop(): Promise<void>
}

const STARTUP_TIMEOUT_MS = 120_000

/** Starts one scenario's stack and returns it ready for the first navigation. */
export async function startScenarioStack(profile: StackProfile): Promise<ScenarioStack> {
  const id = runId()
  const labels = runLabels(id)
  const index = allocateScenarioIndex()
  const network = requireSharedNetwork()

  // Aliases are unique per scenario because the network is not.
  const tag = `${id}-${index}`
  const postgresAlias = `postgres-${tag}`
  const backendAlias = `backend-${tag}`
  const caddyAlias = `caddy-${tag}`
  const started: StartedTestContainer[] = []

  try {
    // PostgreSQL with durability off and its data directory on tmpfs. This
    // database lives for one scenario and is thrown away; fsync buys nothing
    // and costs the startup time that makes per-scenario isolation affordable.
    // Measured at 791 ms to "ready to accept connections".
    const postgresPromise = new GenericContainer(POSTGRES_IMAGE)
      .withLabels(labels)
      .withNetworkMode(network)
      .withNetworkAliases(postgresAlias)
      .withExposedPorts(5432)
      .withEnvironment({
        POSTGRES_USER: DB_USER,
        POSTGRES_PASSWORD: DB_PASSWORD,
        POSTGRES_DB: DB_NAME,
      })
      .withCommand([
        'postgres',
        '-c', 'fsync=off',
        '-c', 'full_page_writes=off',
        '-c', 'synchronous_commit=off',
      ])
      .withTmpFs({ '/var/lib/postgresql/data': 'rw' })
      // The entrypoint starts a temporary server to initialise the cluster
      // before the real one, so the message appears twice.
      .withWaitStrategy(Wait.forLogMessage(/database system is ready to accept connections/, 2))
      .withStartupTimeout(STARTUP_TIMEOUT_MS)
      .start()

    // Caddy starts before the backend because its published port is part of
    // this scenario's origin, and the `auth` profile pins that origin — port
    // included — in the backend's FRONTEND_ORIGIN. The upstreams are resolved
    // per request, so a proxy whose backend is not up yet is not a problem.
    const caddyPromise = new GenericContainer(CADDY_IMAGE)
      .withLabels(labels)
      .withNetworkMode(network)
      .withNetworkAliases(caddyAlias)
      .withExposedPorts(443)
      // The Caddyfile's upstreams are placeholders; the backend's is this
      // scenario's own alias, the front end's is the one shared container.
      .withEnvironment({
        BACKEND_UPSTREAM: `${backendAlias}:8080`,
        FRONTEND_UPSTREAM: `${FRONTEND_ALIAS}:${FRONTEND_PORT}`,
      })
      .withBindMounts([
        { source: CADDYFILE, target: '/etc/caddy/Caddyfile', mode: 'ro' },
        { source: CERTS_DIR, target: '/etc/caddy/certs', mode: 'ro' },
      ])
      .withWaitStrategy(Wait.forLogMessage(/serving initial configuration/))
      .withStartupTimeout(STARTUP_TIMEOUT_MS)
      .start()

    const [postgres, caddy] = await Promise.all([postgresPromise, caddyPromise])
    started.push(postgres, caddy)

    const port = caddy.getMappedPort(443)
    const baseURL = `https://${SCENARIO_HOST}:${port}`

    // SCHEMA_AUTO_MIGRATE is false because `run migration` is a step of the
    // feature: the schema is created by the Background, in the open, not as a
    // side effect of a container booting.
    const backend = await new GenericContainer(BACKEND_IMAGE)
      .withLabels(labels)
      .withNetworkMode(network)
      .withNetworkAliases(backendAlias)
      .withExposedPorts(8080)
      .withEnvironment({
        PORT: '8080',
        DB_DSN: `postgres://${DB_USER}:${DB_PASSWORD}@${postgresAlias}:5432/${DB_NAME}?sslmode=disable`,
        JWT_SECRET,
        SCHEMA_AUTO_MIGRATE: 'false',
        ...backendProfileEnvironment(profile, baseURL),
      })
      // Any status proves the HTTP server is listening, which is all that is
      // being waited for: the schema does not exist yet, so a route that reads
      // the database is entitled to fail at this point.
      .withWaitStrategy(Wait.forHttp('/api/v1/products', 8080).forStatusCodeMatching(() => true))
      .withStartupTimeout(STARTUP_TIMEOUT_MS)
      .start()
    started.push(backend)

    const db = new pg.Client({
      host: postgres.getHost(),
      port: postgres.getMappedPort(5432),
      user: DB_USER,
      password: DB_PASSWORD,
      database: DB_NAME,
    })
    await db.connect()

    // One condition-based wait on the exact URL the browser will use: it proves
    // the published port is reachable, that TLS and SNI agree on a non-standard
    // port, and that the proxy can reach the shared frontend. Doing it here
    // rather than letting the first `visit` discover it turns a class of flake
    // into a startup failure with a legible cause.
    await waitForOrigin(baseURL)

    const containers: Record<string, StartedTestContainer> = { postgres, backend, caddy }

    recordScenarioStack({ index, port, profile, network, database: postgres.getId(), baseURL })

    let stopped = false
    return {
      index,
      profile,
      port,
      baseURL,
      db,
      async migrate() {
        for (const file of migrationFiles()) {
          const sql = fs.readFileSync(file, 'utf8')
          try {
            await db.query(sql)
          } catch (error) {
            throw new Error(`run migration failed applying ${path.basename(file)}: ${message(error)}`)
          }
        }
      },
      async collectLogs() {
        const entries = await Promise.all(
          Object.entries(containers).map(
            async ([name, container]) => [name, await containerLogs(container.getId())] as const,
          ),
        )
        return Object.fromEntries(entries)
      },
      async stop() {
        if (stopped) return
        stopped = true
        await teardown(db, started)
      },
    }
  } catch (error) {
    await teardown(undefined, started)
    throw error
  }
}

function backendProfileEnvironment(profile: StackProfile, baseURL: string): Record<string, string> {
  // spec.md, "Isolation":
  //   default — a scenario that is not about authentication should not go red
  //             for a cookie-policy reason.
  //   auth    — the backend configured the way production is. The cookie then
  //             has to survive TLS termination at Caddy, an origin check that
  //             includes the port, and the browser's own Secure / SameSite
  //             rules, which is the only way those are actually tested.
  if (profile === 'auth') {
    return { FRONTEND_ORIGIN: baseURL, COOKIE_SECURE: 'true' }
  }
  return { FRONTEND_ORIGIN: '*', COOKIE_SECURE: 'false' }
}

function migrationFiles(): string[] {
  return fs
    .readdirSync(MIGRATIONS_DIR)
    .filter((name) => name.endsWith('.up.sql'))
    .sort()
    .map((name) => path.join(MIGRATIONS_DIR, name))
}

async function teardown(db: pg.Client | undefined, containers: StartedTestContainer[]): Promise<void> {
  const failures: string[] = []
  if (db) {
    await db.end().catch((error: unknown) => failures.push(`close database: ${message(error)}`))
  }
  // Reverse order, so the proxy stops before what it proxies to.
  for (const container of [...containers].reverse()) {
    await container
      .stop({ remove: true, removeVolumes: true })
      .catch((error: unknown) => failures.push(`stop ${container.getName()}: ${message(error)}`))
  }
  if (failures.length > 0) {
    throw new Error(`scenario teardown left something behind:\n  ${failures.join('\n  ')}`)
  }
}

/**
 * Polls the scenario's own origin until the proxy answers.
 *
 * `rejectUnauthorized: false` matches the browser context's `ignoreHTTPSErrors`:
 * the certificate is a local one and the machine's trust store is deliberately
 * not modified by this suite.
 */
async function waitForOrigin(baseURL: string, timeoutMs = 30_000): Promise<void> {
  const deadline = Date.now() + timeoutMs
  let last = 'no attempt completed'
  for (;;) {
    try {
      const status = await probe(baseURL)
      if (status >= 200 && status < 400) return
      last = `HTTP ${status}`
    } catch (error) {
      last = message(error)
    }
    if (Date.now() > deadline) {
      throw new Error(`${baseURL} did not become reachable within ${timeoutMs}ms; last attempt: ${last}`)
    }
    await delay(100)
  }
}

function probe(url: string): Promise<number> {
  return new Promise((resolve, reject) => {
    const request = https.get(url, { rejectUnauthorized: false, timeout: 5000 }, (response) => {
      response.resume()
      resolve(response.statusCode ?? 0)
    })
    request.on('timeout', () => request.destroy(new Error('probe timed out')))
    request.on('error', reject)
  })
}

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms))
}

function requireSharedNetwork(): string {
  const name = process.env[ENV.sharedNetwork]
  if (!name) {
    throw new Error(
      `${ENV.sharedNetwork} is not set: the run's network and shared frontend container have not been started. ` +
        'They are created by tests/e2e/global-setup.ts, which playwright.config.ts registers as globalSetup.',
    )
  }
  return name
}

/** One line per scenario, for the harness self-test that checks isolation. */
export interface ScenarioStackRecord {
  index: number
  port: number
  profile: StackProfile
  /** The run's one shared network — the same for every scenario. */
  network: string
  /**
   * The id of this scenario's PostgreSQL container.
   *
   * The container rather than its alias: an alias is a name this harness chose,
   * and a name proves nothing about what was reached. Two container ids cannot
   * be equal unless the two scenarios really did share one database.
   */
  database: string
  baseURL: string
}

/**
 * Appends this scenario's address to the run's ledger.
 *
 * tests/e2e/harness reads it back to assert that a parallel run really did give
 * every scenario its own port and its own database on the one shared network —
 * what isolation rests on, and what no scenario can observe about itself.
 */
function recordScenarioStack(record: ScenarioStackRecord): void {
  fs.appendFileSync(path.join(runStateDir(), 'stacks.jsonl'), `${JSON.stringify(record)}\n`)
}

/** Reads back every scenario stack recorded for a run. */
export function readScenarioStacks(stateDir: string): ScenarioStackRecord[] {
  const file = path.join(stateDir, 'stacks.jsonl')
  if (!fs.existsSync(file)) return []
  return fs
    .readFileSync(file, 'utf8')
    .split('\n')
    .filter((line) => line.trim() !== '')
    .map((line) => JSON.parse(line) as ScenarioStackRecord)
}

function message(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}
