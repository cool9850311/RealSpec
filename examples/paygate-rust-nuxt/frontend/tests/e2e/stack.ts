// One scenario's infrastructure: the same eleven containers
// backend/apitest/src/stack.rs starts for the API surface, plus Caddy with the
// prebuilt static front end baked in (`spec.md`, "Isolation").
//
//   postgres · redis · kafka · clickhouse · provider-mock · demo-merchant ·
//   api · relay · ingester · notifier · reconciler · caddy
//
// There is no run-wide FRONTEND container to coordinate the way
// examples/minimart-go-nuxt/frontend needs one — the front end here is static
// files copied into each scenario's own Caddy (build.ts, generateFrontend)
// rather than a long-lived container every scenario would otherwise have to
// share. There IS one run-wide Docker network; see `ensureRunNetwork` below.
//
// ── Ordering, and the one two-phase dependency ───────────────────────────────
//
// `@stack:auth` needs FRONTEND_ORIGIN pinned to the scenario's own origin
// (spec.md, "Isolation"), which is only knowable once Caddy's mapped port
// exists — and this testcontainers version's `restart()` takes no new
// environment, so a container cannot be told its origin after the fact. The
// fix is ordering, not a restart: Caddy depends on nothing (its Caddyfile
// resolves upstreams by network alias, which exists the moment a container
// joins the network, whether or not that container has finished booting yet),
// so it starts in the FIRST phase, alongside the data stores. Only `api` reads
// FRONTEND_ORIGIN/COOKIE_SECURE, so only `api` waits for Caddy's port before it
// starts — everything else that depends on Postgres/Redis/Kafka/ClickHouse
// starts as soon as those are ready, same as backend/apitest/src/stack.rs.
//
// ── One network for the whole run ────────────────────────────────────────────
//
// A scenario used to create its own `Network` (Ryuk reaps it — see
// playwright.config.ts). A run of N scenarios therefore had N networks, and
// Docker's default address pool holds only ~31 subnets — shared with every
// other project on the machine — so a run of any real size exhausted it, and a
// crashed run left its networks behind for the next one to trip over:
// "(HTTP code 400) unexpected - all predefined address pools have been fully
// subnetted". backend/apitest/src/stack.rs hit the same ceiling and fixed it
// the same way this file now does: ONE Docker network for the whole run, not
// one per scenario.
//
// Isolation does not rest on the network. It rests on every container being
// this scenario's own: each alias carries the scenario id, nothing addresses
// another scenario's containers, and a scenario's own PostgreSQL, Redis, Kafka
// and ClickHouse are started and removed with it. examples/minimart-go-nuxt
// shares one bridge for a whole run too, for its own reason (creating one
// while a browser is running aborts the browser's requests).
//
// Rust's `testcontainers-rs` lets `Stack::start` hand every container a plain
// network NAME and let the runner create it on first use — see stack.rs's own
// `RUN_NETWORK`. `testcontainers` (this package) has no such shortcut: a
// `Network` is created by calling `.start()` on an object, which hands back a
// `StartedNetwork` that lives only in the process that created it — and
// Playwright workers are separate OS processes, so a `StartedNetwork` built in
// one cannot be handed to another. `ensureRunNetwork` (global-setup.ts) solves
// this by name instead of by object: it creates the network once, in whichever
// process runs global setup, and writes the name to `env.ts`'s
// `networkStateFile()` — a path outside any single run's own state, so a later
// `npm run test:e2e` invocation, and a nested run started by
// tests/e2e/harness's own self-test, all check the same file before deciding
// whether a network from an earlier invocation is still around to reuse rather
// than create a second one. Every scenario then joins it with
// `GenericContainer.withNetworkMode(name)` — the same "attach by name" the
// Docker CLI's `--network` flag and Rust's `with_network(&str)` both offer —
// rather than `withNetwork(startedNetwork)`, which needs the object this
// process never had. Ryuk stays enabled and reaps the network with the
// session, exactly as it already did for the one every scenario used to make
// for itself.

import fs from 'node:fs'
import path from 'node:path'
import https from 'node:https'
import pg from 'pg'
import { GenericContainer, Network, Wait, type StartedTestContainer } from 'testcontainers'
import {
  APP_IMAGE,
  CADDYFILE,
  CADDY_IMAGE,
  CERTS_DIR,
  CH_DB,
  CH_USER,
  CLICKHOUSE_IMAGE,
  CLICKHOUSE_MIGRATIONS_DIR,
  FIXTURE_MERCHANT_API_KEY,
  FIXTURE_MERCHANT_HASH_IV,
  FIXTURE_MERCHANT_HASH_KEY,
  FRONTEND_OUTPUT_DIR,
  KAFKA_IMAGE,
  KAFKA_TOPIC,
  KAFKA_TOPIC_PARTITIONS,
  PG_DB,
  PG_PASSWORD,
  PG_USER,
  POSTGRES_IMAGE,
  POSTGRES_MIGRATIONS_DIR,
  REDIS_IMAGE,
  SCENARIO_HOST,
  allocateScenarioIndex,
  networkStateFile,
  runStateDir,
} from './env'
import { containerLogs, findExistingNetworks, scenarioLabels } from './docker'

const STARTUP_TIMEOUT_MS = 180_000
const APP_PORT = 8080
const CADDY_PORT = 8443

/**
 * The infrastructure profile a scenario's tags select (spec.md, "Isolation").
 * `@stack:scaled` is a real profile of the API surface but no e2e feature
 * carries it (spec/bdd/e2e/*.feature), so it is deliberately not implemented
 * here — an unknown `@stack:` tag is refused rather than silently run against
 * the default, so a feature that grew that tag would fail loudly instead of
 * running against the wrong shape of stack.
 */
export type StackProfile = 'default' | 'auth'

export const STACK_TAG_PREFIX = '@stack:'

export function profileFromTags(tags: readonly string[]): StackProfile {
  const stackTags = tags.filter((tag) => tag.startsWith(STACK_TAG_PREFIX))
  if (stackTags.length === 0) return 'default'
  if (stackTags.length > 1) {
    throw new Error(`a scenario may carry at most one ${STACK_TAG_PREFIX} tag, found: ${stackTags.join(', ')}`)
  }
  const name = stackTags[0].slice(STACK_TAG_PREFIX.length)
  if (name === 'auth') return 'auth'
  throw new Error(
    `unknown infrastructure profile ${stackTags[0]}; stack.ts knows ${STACK_TAG_PREFIX}auth and the untagged default ` +
      '(not @stack:scaled: no e2e feature uses it)',
  )
}

/** One ingester's `GET /status` (backend/crates/worker/src/admin.rs). */
interface IngesterStatus {
  ready: boolean
  lag: number
}

export interface ScenarioStack {
  readonly index: number
  readonly profile: StackProfile
  /** The host port Caddy's 8443 is published on — this scenario's own address. */
  readonly port: number
  /** `https://127.0.0.1:<port>` — the browser context's base URL. */
  readonly baseURL: string
  /** This scenario's database, connected directly, bypassing the api. */
  readonly db: pg.Client
  /** Applies backend/migrations/{postgres,clickhouse} to this scenario's stores. */
  migrate(): Promise<void>
  /**
   * `in ClickHouse query returns <n> rows:` — runs `sql` against this
   * scenario's ClickHouse and returns the matching rows.
   */
  clickhouseQuery(sql: string): Promise<Record<string, unknown>[]>
  /**
   * `background work has settled` — format.yml's three conditions, polled
   * every 100 ms for up to 30 s. Resolves when all three hold; rejects naming
   * whichever did not, with numbers.
   */
  backgroundWorkSettled(): Promise<void>
  /**
   * `projection "reports" is rebuilt from the event log` — runs a one-shot
   * `paygate-worker rebuild reports` container to completion, exactly as
   * backend/apitest/src/stack.rs does for the API surface.
   */
  rebuildReportsProjection(): Promise<void>
  /** `payment provider received <n> checkout|refund requests` — the mock's own log. */
  paymentProviderReceived(kind: 'checkout' | 'refund'): Promise<number>
  /** `merchant received <n> notifications at "<path>"` — the merchant's own log. */
  merchantReceived(path: string): Promise<number>
  /** Every container's last lines of output, for a failed scenario's artefact. */
  collectLogs(): Promise<Record<string, string>>
  stop(): Promise<void>
}

/**
 * Ensures the one Docker network this whole run's scenarios join exists, and
 * publishes its name to `env.ts`'s `networkStateFile()` — see stack.ts's
 * header comment for why a name, in a file, rather than the `StartedNetwork`
 * object `.start()` hands back.
 *
 * Called from global-setup.ts, once per `playwright test` invocation
 * (including a nested one — child-run.ts), before any scenario can start.
 * Idempotent across invocations: if the name a previous invocation recorded
 * still names a real Docker network — the common case, since the previous
 * invocation may not be done with it, or Ryuk may simply not have gotten to
 * it yet — that network is reused rather than a doomed attempt to create a
 * second one under the same name.
 */
export async function ensureRunNetwork(): Promise<void> {
  const file = networkStateFile()
  const previous = readNetworkStateFile(file)
  if (previous && (await findExistingNetworks([previous])).length > 0) return
  const network = await new Network().start()
  fs.writeFileSync(file, `${JSON.stringify({ name: network.getName() }, null, 2)}\n`)
}

function readNetworkStateFile(file: string): string | undefined {
  if (!fs.existsSync(file)) return undefined
  try {
    return (JSON.parse(fs.readFileSync(file, 'utf8')) as { name?: string }).name
  } catch {
    return undefined
  }
}

/** The name of this run's one Docker network — set by `ensureRunNetwork` in global setup. */
export function runNetworkName(): string {
  const file = networkStateFile()
  const name = readNetworkStateFile(file)
  if (!name) {
    throw new Error(
      `${file} does not name a network; tests/e2e/global-setup.ts's ensureRunNetwork() must run ` +
        'before any scenario starts',
    )
  }
  return name
}

/** Starts one scenario's full stack and returns it ready for the first navigation. */
export async function startScenarioStack(profile: StackProfile): Promise<ScenarioStack> {
  const index = allocateScenarioIndex()
  const id = `s${index}`
  const labels = scenarioLabels(index)
  const started: StartedTestContainer[] = []

  const network = runNetworkName()

  const pgAlias = `pg-${id}`
  const redisAlias = `redis-${id}`
  const kafkaAlias = `kafka-${id}`
  const chAlias = `ch-${id}`
  const mockAlias = `mock-${id}`
  const merchantAlias = `merchant-${id}`
  const apiAlias = `api-${id}`
  const relayAlias = `relay-${id}`
  const ingestAlias = `ingest-${id}`
  const notifyAlias = `notify-${id}`
  const reconcileAlias = `reconcile-${id}`
  const caddyAlias = `caddy-${id}`

  try {
    // ── Phase A: the data stores and Caddy, in parallel ─────────────────────
    // Caddy belongs here, not with the app layer: its Caddyfile resolves
    // upstreams by network alias at REQUEST time, so it needs no app container
    // to be up yet, and starting it early is what makes its mapped port known
    // before `api` — the one container that needs it — starts.
    const postgresPromise = new GenericContainer(POSTGRES_IMAGE)
      .withLabels(labels)
      .withNetworkMode(network)
      .withNetworkAliases(pgAlias)
      .withExposedPorts(5432)
      .withEnvironment({ POSTGRES_USER: PG_USER, POSTGRES_PASSWORD: PG_PASSWORD, POSTGRES_DB: PG_DB })
      // The entrypoint starts a temporary server to initialise the cluster
      // before the real one, so the message appears twice.
      .withWaitStrategy(Wait.forLogMessage(/database system is ready to accept connections/, 2))
      .withStartupTimeout(STARTUP_TIMEOUT_MS)
      .start()

    const redisPromise = new GenericContainer(REDIS_IMAGE)
      .withLabels(labels)
      .withNetworkMode(network)
      .withNetworkAliases(redisAlias)
      .withExposedPorts(6379)
      // No persistence, as the deployment has none (spec.md, "Redis — nothing
      // that matters"). A Redis that saves an RDB file on SIGTERM would carry
      // the rate limiter's buckets across a restart, and the whole point is
      // that it does not.
      .withCommand(['redis-server', '--save', '', '--appendonly', 'no'])
      .withWaitStrategy(Wait.forLogMessage(/Ready to accept connections/))
      .withStartupTimeout(STARTUP_TIMEOUT_MS)
      .start()

    const clickhousePromise = new GenericContainer(CLICKHOUSE_IMAGE)
      .withLabels(labels)
      .withNetworkMode(network)
      .withNetworkAliases(chAlias)
      .withExposedPorts(8123)
      .withUlimits({ nofile: { soft: 262_144, hard: 262_144 } })
      // Without this the image writes users.d/default-user.xml pinning the
      // `default` user to ::1 and 127.0.0.1, so a request from any other
      // container is refused — and refused as AUTHENTICATION_FAILED, which sends
      // you hunting for a password that was never the problem. Skipping the setup
      // leaves `default` reachable with no password, which is what
      // CLICKHOUSE_PASSWORD="" in every container's environment already says.
      .withEnvironment({ CLICKHOUSE_SKIP_USER_SETUP: '1' })
      .withWaitStrategy(Wait.forHttp('/ping', 8123))
      .withStartupTimeout(STARTUP_TIMEOUT_MS)
      .start()

    // The exact env this image needs for a single-node KRaft broker, verified
    // by hand on this machine — matches backend/apitest/src/stack.rs's own.
    const kafkaPromise = new GenericContainer(KAFKA_IMAGE)
      .withLabels(labels)
      .withNetworkMode(network)
      .withNetworkAliases(kafkaAlias)
      .withEnvironment({
        KAFKA_NODE_ID: '1',
        KAFKA_PROCESS_ROLES: 'broker,controller',
        KAFKA_LISTENERS: 'PLAINTEXT://:9092,CONTROLLER://:9093',
        KAFKA_ADVERTISED_LISTENERS: `PLAINTEXT://${kafkaAlias}:9092`,
        KAFKA_CONTROLLER_LISTENER_NAMES: 'CONTROLLER',
        KAFKA_CONTROLLER_QUORUM_VOTERS: `1@${kafkaAlias}:9093`,
        KAFKA_LISTENER_SECURITY_PROTOCOL_MAP: 'CONTROLLER:PLAINTEXT,PLAINTEXT:PLAINTEXT',
        KAFKA_INTER_BROKER_LISTENER_NAME: 'PLAINTEXT',
        KAFKA_OFFSETS_TOPIC_REPLICATION_FACTOR: '1',
        KAFKA_TRANSACTION_STATE_LOG_REPLICATION_FACTOR: '1',
        KAFKA_TRANSACTION_STATE_LOG_MIN_ISR: '1',
        KAFKA_GROUP_INITIAL_REBALANCE_DELAY_MS: '0',
        KAFKA_AUTO_CREATE_TOPICS_ENABLE: 'false',
        CLUSTER_ID: '5L6g3nShT-eMCtK--X86sw',
      })
      .withWaitStrategy(Wait.forLogMessage(/Kafka Server started/))
      .withStartupTimeout(STARTUP_TIMEOUT_MS)
      .start()

    const caddyPromise = new GenericContainer(CADDY_IMAGE)
      .withLabels(labels)
      .withNetworkMode(network)
      .withNetworkAliases(caddyAlias)
      .withExposedPorts(CADDY_PORT)
      .withEnvironment({
        // Caddy's own `{$VAR}` substitution, resolved from the process
        // environment when the Caddyfile loads. One instance of each upstream:
        // no e2e feature carries @stack:scaled (see StackProfile above), so
        // there is never more than one api/mock/merchant to balance across.
        API_UPSTREAMS: `${apiAlias}:${APP_PORT}`,
        PROVIDER_UPSTREAM: `${mockAlias}:${APP_PORT}`,
        MERCHANT_UPSTREAM: `${merchantAlias}:${APP_PORT}`,
      })
      .withCopyFilesToContainer([
        { source: CADDYFILE, target: '/etc/caddy/Caddyfile' },
        // The certificate the Caddyfile's `tls` directive names. Generated once
        // per run by global setup, never committed.
        { source: path.join(CERTS_DIR, 'cert.pem'), target: '/etc/caddy/certs/cert.pem' },
        { source: path.join(CERTS_DIR, 'key.pem'), target: '/etc/caddy/certs/key.pem' },
      ])
      .withCopyDirectoriesToContainer([{ source: FRONTEND_OUTPUT_DIR, target: '/srv' }])
      .withWaitStrategy(Wait.forLogMessage(/serving initial configuration/))
      .withStartupTimeout(STARTUP_TIMEOUT_MS)
      .start()

    const [postgres, redis, clickhouse, kafka, caddy] = await Promise.all([
      postgresPromise,
      redisPromise,
      clickhousePromise,
      kafkaPromise,
      caddyPromise,
    ])
    started.push(postgres, redis, clickhouse, kafka, caddy)

    const port = caddy.getMappedPort(CADDY_PORT)
    const baseURL = `https://${SCENARIO_HOST}:${port}`
    await waitForOrigin(baseURL)

    const pgDsn = `postgres://${PG_USER}:${PG_PASSWORD}@${pgAlias}:5432/${PG_DB}`
    const redisUrl = `redis://${redisAlias}:6379`
    const chUrl = `http://${chAlias}:8123`
    const kafkaBrokers = `${kafkaAlias}:9092`

    // ── Phase B: the two counterparties ──────────────────────────────────────
    const mockPromise = startAppContainer(network, mockAlias, labels, ['paygate-provider-mock'], {
      ...commonEnv(mockAlias),
      PROVIDER_CALLBACK_URLS: `http://${apiAlias}:${APP_PORT}`,
      // "the mock is given the same pair" (spec.md, "Environment variables").
      // These are the platform credentials every feature's Background seeds into
      // `providers`, handed to the mock explicitly rather than left to its
      // defaults: the mock VERIFIES what paygate signs, so the suite's
      // "signed correctly" assertions mean nothing unless both sides were
      // issued the same pair, and a coupling that load-bearing should be visible.
      ECPAY_PLATFORM_ID: '3002607',
      ECPAY_HASH_KEY: 'pwFHCqoQZGmho4w6',
      ECPAY_HASH_IV: 'EkRm7iFT261dpevs',
      NEWEBPAY_PLATFORM_ID: 'MS12345678',
      NEWEBPAY_HASH_KEY: 'Fs5cX1TGqYZ3kLmN8pQrS2tUvW4xY6zA',
      NEWEBPAY_HASH_IV: 'B7cD9eF1gH3iJ5kL',
    }, httpReady('/provider/__control/requests'))

    const merchantPromise = startAppContainer(network, merchantAlias, labels, ['paygate-demo-merchant'], {
      ...commonEnv(merchantAlias),
      GATEWAY_URL: `http://${apiAlias}:${APP_PORT}`,
      DEMO_MERCHANT_API_KEY: FIXTURE_MERCHANT_API_KEY,
      MERCHANT_HASH_KEY: FIXTURE_MERCHANT_HASH_KEY,
      MERCHANT_HASH_IV: FIXTURE_MERCHANT_HASH_IV,
      // /notify-slow answers correctly but just past the notifier's deadline, so
      // it has to be told the same deadline the notifier below is given.
      NOTIFY_TIMEOUT_MS: '500',
    }, httpReady('/demo-merchant/api/__deliveries'))

    const [mock, merchant] = await Promise.all([mockPromise, merchantPromise])
    started.push(mock, merchant)

    // ── Phase C: the api and every worker role ───────────────────────────────
    // Only `api` reads FRONTEND_ORIGIN/COOKIE_SECURE (spec.md, "Environment
    // variables"), and it is the only container in this phase whose env
    // depends on Caddy's port — see the header comment.
    const apiPromise = startAppContainer(network, apiAlias, labels, ['paygate-api'], {
      ...commonEnv(apiAlias),
      DB_DSN: pgDsn,
      DB_POOL_MAX: '20',
      SCHEMA_AUTO_MIGRATE: 'false',
      REDIS_URL: redisUrl,
      REDIS_TIMEOUT_MS: '100',
      CLICKHOUSE_URL: chUrl,
      CLICKHOUSE_DATABASE: CH_DB,
      CLICKHOUSE_USER: CH_USER,
      CLICKHOUSE_PASSWORD: '',
      PROVIDER_TRADE_NO_PREFIX: 'PG',
      PUBLIC_BASE_URL: `http://${apiAlias}:${APP_PORT}`,
      PSP_TIMEOUT_MS: '1000',
      SESSION_TTL_SECONDS: '28800',
      API_KEY_CACHE_TTL_SECONDS: '60',
      IDEMPOTENCY_TTL_HOURS: '24',
      IDEMPOTENCY_LOCK_TTL_MS: '30000',
      PROVIDER_BASE_URL: `http://${mockAlias}:${APP_PORT}`,
      ...apiOriginEnvironment(profile, baseURL),
    }, httpReady('/api/v1/health/live'))

    const relayPromise = startAppContainer(network, relayAlias, labels, ['paygate-worker', 'relay'], {
      ...commonEnv(relayAlias),
      DB_DSN: pgDsn,
      DB_POOL_MAX: '20',
      KAFKA_BROKERS: kafkaBrokers,
      KAFKA_TOPIC,
      KAFKA_TOPIC_PARTITIONS: String(KAFKA_TOPIC_PARTITIONS),
      RELAY_BATCH_SIZE: '100',
      RELAY_POLL_INTERVAL_MS: '50',
    }, httpReady('/healthz'))

    const ingestPromise = startAppContainer(network, ingestAlias, labels, ['paygate-worker', 'ingest'], {
      ...commonEnv(ingestAlias),
      KAFKA_BROKERS: kafkaBrokers,
      KAFKA_TOPIC,
      KAFKA_GROUP_ID: 'paygate-reports',
      INGEST_BATCH_MAX: '500',
      INGEST_BATCH_WAIT_MS: '200',
      CLICKHOUSE_URL: chUrl,
      CLICKHOUSE_DATABASE: CH_DB,
      CLICKHOUSE_USER: CH_USER,
      CLICKHOUSE_PASSWORD: '',
    }, httpReady('/healthz'))

    const notifyPromise = startAppContainer(network, notifyAlias, labels, ['paygate-worker', 'notify'], {
      ...commonEnv(notifyAlias),
      DB_DSN: pgDsn,
      DB_POOL_MAX: '20',
      NOTIFY_MAX_ATTEMPTS: '5',
      NOTIFY_BACKOFF_MS: '50,100,200,400',
      NOTIFY_TIMEOUT_MS: '500',
      NOTIFY_POLL_INTERVAL_MS: '50',
      MERCHANT_BASE_URL: `http://${merchantAlias}:${APP_PORT}`,
    }, httpReady('/healthz'))

    const reconcilePromise = startAppContainer(network, reconcileAlias, labels, ['paygate-worker', 'reconcile'], {
      ...commonEnv(reconcileAlias),
      DB_DSN: pgDsn,
      DB_POOL_MAX: '20',
      RECONCILE_AFTER_MINUTES: '60',
      RECONCILE_RETRY_MINUTES: '60',
      // 0 = do not poll: `the reconciler runs` is API-only (not one of this
      // surface's 15 steps), but the container still runs the same
      // configuration production and the API suite use, so it never settles
      // anything on its own mid-scenario.
      RECONCILE_POLL_INTERVAL_MS: '0',
      RECONCILE_BATCH_SIZE: '50',
      PROVIDER_BASE_URL: `http://${mockAlias}:${APP_PORT}`,
    }, httpReady('/healthz'))

    const [api, relay, ingest, notify, reconcile] = await Promise.all([
      apiPromise,
      relayPromise,
      ingestPromise,
      notifyPromise,
      reconcilePromise,
    ])
    started.push(api, relay, ingest, notify, reconcile)

    const ingestPort = ingest.getMappedPort(APP_PORT)
    await waitIngesterReady(ingestPort)

    const db = new pg.Client({
      host: postgres.getHost(),
      port: postgres.getMappedPort(5432),
      user: PG_USER,
      password: PG_PASSWORD,
      database: PG_DB,
    })
    await db.connect()

    const clickhousePort = clickhouse.getMappedPort(8123)
    const mockPort = mock.getMappedPort(APP_PORT)
    const merchantPort = merchant.getMappedPort(APP_PORT)

    const containers: Record<string, StartedTestContainer> = {
      postgres,
      redis,
      kafka,
      clickhouse,
      'provider-mock': mock,
      'demo-merchant': merchant,
      api,
      relay,
      ingester: ingest,
      notifier: notify,
      reconciler: reconcile,
      caddy,
    }

    recordScenarioStack({ index, port, profile, network, database: postgres.getId(), baseURL })

    let stopped = false
    return {
      index,
      profile,
      port,
      baseURL,
      db,
      async migrate() {
        await migratePostgres(db)
        await migrateClickHouse(clickhousePort)
      },
      async clickhouseQuery(sql: string) {
        return clickhouseQuery(clickhousePort, sql)
      },
      async backgroundWorkSettled() {
        await waitBackgroundWorkSettled(db, [ingestPort])
      },
      async rebuildReportsProjection() {
        await rebuildReportsProjection(network, id, labels, {
          DB_DSN: pgDsn,
          CLICKHOUSE_URL: chUrl,
          CLICKHOUSE_DATABASE: CH_DB,
          CLICKHOUSE_USER: CH_USER,
          CLICKHOUSE_PASSWORD: '',
        })
      },
      async paymentProviderReceived(kind: 'checkout' | 'refund') {
        return countProviderRequests(mockPort, kind)
      },
      async merchantReceived(path: string) {
        return countMerchantDeliveries(merchantPort, path)
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

function apiOriginEnvironment(profile: StackProfile, baseURL: string): Record<string, string> {
  // spec.md, "Isolation":
  //   default — a scenario that is not about authentication should not go red
  //             for a cookie-policy reason.
  //   auth    — the api configured the way production is, so the cookie has to
  //             survive TLS termination at Caddy, an origin check that
  //             includes the port, and the browser's own Secure/SameSite
  //             rules — the only way those are actually tested.
  if (profile === 'auth') {
    return { FRONTEND_ORIGIN: baseURL, COOKIE_SECURE: 'true' }
  }
  return { FRONTEND_ORIGIN: '*', COOKIE_SECURE: 'false' }
}

/** backend/apitest/src/stack.rs's `common_env`. */
function commonEnv(instanceName: string): Record<string, string> {
  return {
    PORT: String(APP_PORT),
    INSTANCE_ID: instanceName,
    LOG_LEVEL: 'info',
    SHUTDOWN_GRACE_SECONDS: '1',
  }
}

function httpReady(path: string) {
  return Wait.forHttp(path, APP_PORT).forStatusCode(200)
}

async function startAppContainer(
  network: string,
  alias: string,
  labels: Record<string, string>,
  cmd: string[],
  env: Record<string, string>,
  wait: ReturnType<typeof Wait.forHttp>,
): Promise<StartedTestContainer> {
  return new GenericContainer(APP_IMAGE)
    .withLabels(labels)
    .withNetworkMode(network)
    .withNetworkAliases(alias)
    .withExposedPorts(APP_PORT)
    .withCommand(cmd)
    .withEnvironment(env)
    .withWaitStrategy(wait)
    .withStartupTimeout(STARTUP_TIMEOUT_MS)
    .start()
}

// ── Migrations ────────────────────────────────────────────────────────────────

function sqlFiles(dir: string): string[] {
  return fs
    .readdirSync(dir)
    .filter((name) => name.endsWith('.sql'))
    .sort()
    .map((name) => path.join(dir, name))
}

async function migratePostgres(db: pg.Client): Promise<void> {
  for (const file of sqlFiles(POSTGRES_MIGRATIONS_DIR)) {
    try {
      await db.query(fs.readFileSync(file, 'utf8'))
    } catch (error) {
      throw new Error(`run migration failed applying ${path.basename(file)}: ${message(error)}`)
    }
  }
}

async function migrateClickHouse(port: number): Promise<void> {
  for (const file of sqlFiles(CLICKHOUSE_MIGRATIONS_DIR)) {
    // Split on `;`, because ClickHouse's HTTP interface takes one statement per
    // request — and drop any piece that is only `--` comment. A migration that
    // ends with an explanatory note leaves exactly such a piece, and ClickHouse
    // answers it with `Code: 62 … Empty query`: a failure caused entirely by
    // prose. backend/apitest's own splitter carries the same rule, for the same
    // file.
    const statements = fs
      .readFileSync(file, 'utf8')
      .split(';')
      .map((s) => s.trim())
      .filter((s) => s.split('\n').some((line) => {
        const t = line.trim()
        return t !== '' && !t.startsWith('--')
      }))
    for (const statement of statements) {
      try {
        await chExec(port, statement)
      } catch (error) {
        throw new Error(`run migration failed applying ${path.basename(file)}: ${message(error)}`)
      }
    }
  }
}

/** One ClickHouse statement with no result expected (DDL). */
async function chExec(port: number, statement: string): Promise<void> {
  const response = await fetch(`http://${SCENARIO_HOST}:${port}/?database=${CH_DB}`, {
    method: 'POST',
    body: statement,
  })
  if (!response.ok) {
    throw new Error(`ClickHouse statement failed (${response.status}): ${await response.text()}\nSQL: ${statement}`)
  }
}

/** `in ClickHouse query returns <n> rows:` — see ScenarioStack.clickhouseQuery. */
async function clickhouseQuery(port: number, sql: string): Promise<Record<string, unknown>[]> {
  const trimmed = sql.trim().replace(/;\s*$/, '')
  const response = await fetch(`http://${SCENARIO_HOST}:${port}/?database=${CH_DB}`, {
    method: 'POST',
    body: `${trimmed}\nFORMAT JSONEachRow`,
  })
  if (!response.ok) {
    throw new Error(`ClickHouse query failed (${response.status}): ${await response.text()}\nSQL: ${sql}`)
  }
  const text = await response.text()
  return text
    .split('\n')
    .map((line) => line.trim())
    .filter((line) => line !== '')
    .map((line) => JSON.parse(line) as Record<string, unknown>)
}

// ── background work has settled ─────────────────────────────────────────────

async function pgCount(db: pg.Client, sql: string): Promise<number> {
  const result = await db.query(sql)
  const row = result.rows[0] as Record<string, unknown> | undefined
  if (!row) return 0
  const value = Object.values(row)[0]
  return Number(value ?? 0)
}

async function ingesterStatus(port: number): Promise<IngesterStatus> {
  const response = await fetch(`http://${SCENARIO_HOST}:${port}/status`)
  if (!response.ok) return { ready: false, lag: Number.POSITIVE_INFINITY }
  const body = (await response.json()) as { ready?: boolean; lag?: number }
  return { ready: body.ready === true, lag: body.lag ?? Number.POSITIVE_INFINITY }
}

async function waitBackgroundWorkSettled(
  db: pg.Client,
  ingestPorts: readonly number[],
  timeoutMs = 30_000,
): Promise<void> {
  const deadline = Date.now() + timeoutMs
  for (;;) {
    const [unpublished, notificationsDue, ingesterStatuses] = await Promise.all([
      pgCount(db, 'SELECT count(*) FROM payment_events WHERE published_at IS NULL'),
      pgCount(db, 'SELECT count(*) FROM notifications WHERE delivered_at IS NULL AND exhausted_at IS NULL'),
      Promise.all(ingestPorts.map((port) => ingesterStatus(port))),
    ])
    const notSettled = ingesterStatuses.filter((s) => !s.ready || s.lag !== 0)
    if (unpublished === 0 && notificationsDue === 0 && notSettled.length === 0) return
    if (Date.now() > deadline) {
      throw new Error(
        `background work has settled: timed out after ${timeoutMs}ms\n` +
          `  unpublished payment_events: ${unpublished}\n` +
          `  notifications still due: ${notificationsDue}\n` +
          `  ingesters not settled: ${
            notSettled.length === 0
              ? 'none'
              : ingesterStatuses.map((s, i) => `#${i} ready=${s.ready} lag=${s.lag}`).join(', ')
          }`,
      )
    }
    await delay(100)
  }
}

// ── projection "reports" is rebuilt from the event log ──────────────────────

async function rebuildReportsProjection(
  network: string,
  scenarioId: string,
  labels: Record<string, string>,
  env: Record<string, string>,
): Promise<void> {
  const container = await new GenericContainer(APP_IMAGE)
    .withLabels(labels)
    .withNetworkMode(network)
    .withNetworkAliases(`rebuild-${scenarioId}`)
    .withCommand(['paygate-worker', 'rebuild', 'reports'])
    .withEnvironment({ ...commonEnv(`rebuild-${scenarioId}`), ...env })
    .withWaitStrategy(Wait.forOneShotStartup())
    .withStartupTimeout(60_000)
    .start()
  const stdout = await streamToString(await container.logs())
  await container.stop({ remove: true }).catch(() => undefined)
  if (!stdout.includes('rebuild complete')) {
    throw new Error(`rebuild container exited without logging "rebuild complete"; stdout:\n${stdout}`)
  }
}

async function streamToString(stream: NodeJS.ReadableStream): Promise<string> {
  const chunks: Buffer[] = []
  for await (const chunk of stream) {
    chunks.push(Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk))
  }
  return Buffer.concat(chunks).toString('utf8')
}

// ── the counterparties' own logs ─────────────────────────────────────────────

/** `payment provider received <n> checkout|refund requests` — the mock's request log. */
async function countProviderRequests(mockPort: number, kind: 'checkout' | 'refund'): Promise<number> {
  const response = await fetch(`http://${SCENARIO_HOST}:${mockPort}/provider/__control/requests`)
  if (!response.ok) {
    throw new Error(`GET /provider/__control/requests failed: ${response.status} ${await response.text()}`)
  }
  // `{ charges: [...], refunds: [...] }` — two lists, not one tagged list, as
  // `spec/openapi/payment-provider.yaml`'s `listProviderRequests` defines it.
  // Both include a request the provider REFUSED for a bad signature: the step
  // counts what arrived, not what was accepted.
  const body = (await response.json()) as { charges?: unknown[]; refunds?: unknown[] }
  const list = kind === 'checkout' ? body.charges : body.refunds
  if (!Array.isArray(list)) {
    throw new Error(
      `GET /provider/__control/requests answered without a "${kind === 'checkout' ? 'charges' : 'refunds'}" array: ${JSON.stringify(body)}`,
    )
  }
  return list.length
}

/** `merchant received <n> notifications at "<path>"` — the merchant's own delivery log. */
async function countMerchantDeliveries(merchantPort: number, notifyPath: string): Promise<number> {
  const response = await fetch(`http://${SCENARIO_HOST}:${merchantPort}/demo-merchant/api/__deliveries`)
  if (!response.ok) {
    throw new Error(`GET /demo-merchant/api/__deliveries failed: ${response.status} ${await response.text()}`)
  }
  const body = (await response.json()) as Record<string, unknown[]>
  return (body[notifyPath] ?? []).length
}

// ── readiness ────────────────────────────────────────────────────────────────

async function waitIngesterReady(port: number, timeoutMs = 120_000): Promise<void> {
  const deadline = Date.now() + timeoutMs
  let last: IngesterStatus | undefined
  for (;;) {
    try {
      last = await ingesterStatus(port)
      if (last.ready) return
    } catch {
      // not listening yet
    }
    if (Date.now() > deadline) {
      throw new Error(
        `ingester did not report ready:true within ${timeoutMs}ms (last: ${last ? JSON.stringify(last) : 'no response'})`,
      )
    }
    await delay(100)
  }
}

/**
 * Polls the scenario's own origin until Caddy answers.
 *
 * `rejectUnauthorized: false` matches the browser context's
 * `ignoreHTTPSErrors`: Caddy's `tls internal` certificate is signed by its own
 * ephemeral CA and this suite deliberately never touches the machine's trust
 * store.
 */
async function waitForOrigin(baseURL: string, timeoutMs = 30_000): Promise<void> {
  const deadline = Date.now() + timeoutMs
  let last = 'no attempt completed'
  for (;;) {
    try {
      const status = await probe(baseURL)
      // `/` is answered by Caddy's own static handler (local/Caddyfile's
      // catch-all `file_server`), never by a reverse-proxied service, so a
      // clean 2xx/3xx here proves the port mapping and TLS handshake work —
      // it says nothing about whether api/mock/merchant are up yet, which is
      // exactly why it is safe to require a real success status rather than
      // tolerating a 5xx that would actually mean the copied bundle is broken.
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

async function teardown(db: pg.Client | undefined, containers: StartedTestContainer[]): Promise<void> {
  const failures: string[] = []
  if (db) {
    await db.end().catch((error: unknown) => failures.push(`close database: ${message(error)}`))
  }
  // Reverse order, so Caddy stops before what it proxies to.
  for (const container of [...containers].reverse()) {
    await container
      .stop({ remove: true, removeVolumes: true })
      .catch((error: unknown) => failures.push(`stop ${container.getName()}: ${message(error)}`))
  }
  // The network is not this scenario's to remove: it is the whole run's (see
  // this file's header comment), started once by global setup and joined by
  // every scenario, so no single scenario's teardown could safely remove it
  // even if `spec.md`, "Isolation" allowed hand-rolled removal here, which it
  // does not — Ryuk reaps it with the session.
  if (failures.length > 0) {
    throw new Error(`scenario teardown left something behind:\n  ${failures.join('\n  ')}`)
  }
}

function message(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

// ── the ledger the harness self-test reads back ─────────────────────────────

export interface ScenarioStackRecord {
  index: number
  port: number
  profile: StackProfile
  /** The run's one shared network (see this file's header comment) — every scenario records the same name. */
  network: string
  /** The id of this scenario's PostgreSQL container (see minimart's stack.ts for why the id, not the alias). */
  database: string
  baseURL: string
}

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
