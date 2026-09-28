//! The per-scenario stack: every container a scenario needs, started, waited
//! for and stopped through `testcontainers` (`spec.md`, "Isolation":
//! containers are Testcontainers' to manage, and nothing else manages them).
//! Nothing here calls `docker` directly except [`ensure_image_built`] and
//! [`ensure_run_network`], the two things that section allows a test to do
//! to Docker directly, once, for the whole suite.

use std::{collections::HashMap, future::Future, time::Duration};

use futures::future::join_all;
use testcontainers::{
    core::{wait::HttpWaitStrategy, IntoContainerPort, WaitFor},
    runners::AsyncRunner,
    ContainerAsync, GenericImage, Image, ImageExt,
};
use testcontainers_modules::{clickhouse::ClickHouse, postgres::Postgres, redis::Redis};
use tokio::{io::AsyncWriteExt, sync::OnceCell};

/// The image every app container (api, worker, provider-mock, demo-merchant)
/// runs, built once for the whole suite by [`ensure_image_built`].
pub const APP_IMAGE_NAME: &str = "paygate";
pub const APP_IMAGE_TAG: &str = "apitest";

/// The Acme Coffee fixture every feature's Background seeds with the exact
/// same literals (hash key/iv, raw API key) — see e.g.
/// `spec/bdd/api/handoff.feature`. `demo-merchant` is one long-lived
/// container per scenario, configured before any Background SQL runs, so its
/// `MERCHANT_HASH_KEY`/`MERCHANT_HASH_IV`/`DEMO_MERCHANT_API_KEY` have to be
/// literals a scenario's own seed data will also produce — these are those
/// literals.
pub const FIXTURE_MERCHANT_HASH_KEY: &str = "acmehashkey0123456789abcdef01234";
pub const FIXTURE_MERCHANT_HASH_IV: &str = "acmehashiv012345";
pub const FIXTURE_MERCHANT_API_KEY: &str = "sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc";

/// A ClickHouse that keeps to a share of the machine rather than to a share of
/// what it can see. 1.2 GB is comfortably more than any report in this suite
/// needs and small enough that several stacks coexist.
const CLICKHOUSE_MEMORY_CAP: &str = concat!(
    "<clickhouse>\n",
    "    <max_server_memory_usage>1200000000</max_server_memory_usage>\n",
    "</clickhouse>\n"
);

/// The one Docker network every scenario of a run joins, created by
/// [`ensure_run_network`] before any container. See `Stack::start`.
const RUN_NETWORK: &str = "paygate-apitest";

const STARTUP_TIMEOUT: Duration = Duration::from_secs(120);

/// A whole stack has this long to come up, end to end.
///
/// Every individual wait in this file is already bounded, and that turned out not
/// to be enough: the Docker daemon itself stops answering when too many stacks are
/// asked for at once, and a create-or-start call that never returns is not covered
/// by a readiness timeout. A run wedged that way is worse than a failing one — it
/// reports nothing and finishes never — so this has to be enforced somewhere.
///
/// It is enforced *inside* [`Stack::start`], one container at a time (see
/// `bounded`), not by racing the whole of `start` against it from the
/// outside. Wrapping the outer call in `tokio::time::timeout` was tried
/// first and is exactly the wrong shape: on expiry it cancels the whole
/// `start` future, and everything that future had already built — every
/// container already up — is dropped with it. Every one of those drops then has
/// to do the removal `start` would otherwise have done explicitly, all at once
/// and from inside a cancelled future; a real run hit this twice and left two
/// whole stacks, 22 containers, running, back when `Drop` was disarmed
/// entirely. Timing
/// out each container's own future against this same deadline instead means
/// a hang costs at most that one container: every other future in flight
/// either has already resolved — its container fully owned, gathered by the
/// same partial-failure cleanup a pull failure or a bad wait condition would
/// trigger — or times out the same way. Nothing is ever dropped while it
/// might still be a live container nobody has a handle to.
pub const STACK_TOTAL_TIMEOUT: Duration = Duration::from_secs(360);
const RESTART_READY_TIMEOUT: Duration = Duration::from_secs(60);

static IMAGE_BUILT: OnceCell<()> = OnceCell::const_new();

/// Builds `backend/Dockerfile` into `paygate:apitest`, once, for the whole
/// suite — the one direct `docker` invocation this harness makes.
pub async fn ensure_image_built() {
    IMAGE_BUILT
        .get_or_init(|| async {
            build_image().await.unwrap_or_else(|e| {
                panic!("building {APP_IMAGE_NAME}:{APP_IMAGE_TAG} failed: {e}")
            });
        })
        .await;
}

static RUN_NETWORK_READY: OnceCell<()> = OnceCell::const_new();

/// Creates the run's shared Docker network, once, before any container — the
/// second and last direct `docker` invocation this harness makes, and it is
/// here to keep the library in charge of everything else.
///
/// `Network::new` returns `None` for a network that already exists ("Networks
/// already exists and created outside the testcontainers", its own comment), so
/// a network created here is one the library never owns and never removes.
/// Left to create it itself, the library removes it as soon as the last
/// container referencing it goes — which, between two scenarios, is every time:
/// the network would be destroyed and re-created once per scenario, each cycle
/// another address-pool allocation, and "all predefined address pools have been
/// fully subnetted" is a failure this suite has already had.
///
/// That ownership is the only thing standing between this harness and the
/// library's default `TESTCONTAINERS_COMMAND=remove`, which it needs so that a
/// container whose startup is cancelled out from under it is still removed
/// (`tests/api.rs`, at the top of `main`). Creating it is idempotent: `docker
/// network create` on a name that exists fails, and that failure is the answer
/// to the question this function asks, so it is not an error.
pub async fn ensure_run_network() {
    RUN_NETWORK_READY
        .get_or_init(|| async {
            let output = tokio::process::Command::new("docker")
                .arg("network")
                .arg("create")
                .arg(RUN_NETWORK)
                .output()
                .await
                .unwrap_or_else(|e| {
                    panic!("could not run `docker network create {RUN_NETWORK}`: {e}")
                });
            if output.status.success() {
                eprintln!("[apitest] docker network create {RUN_NETWORK}");
                return;
            }
            // Already there: this run reuses it, exactly as the next run will.
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(
                stderr.contains("already exists"),
                "creating the run network {RUN_NETWORK} failed for a reason other than it \
                 already existing: {stderr}"
            );
        })
        .await;
}

async fn build_image() -> anyhow::Result<()> {
    let root = crate::registry::spec_root();
    let dockerfile = root.join("backend/Dockerfile");
    eprintln!(
        "[apitest] docker build -f {} -t {APP_IMAGE_NAME}:{APP_IMAGE_TAG} {}",
        dockerfile.display(),
        root.display()
    );
    let status = tokio::process::Command::new("docker")
        .arg("build")
        .arg("-f")
        .arg(&dockerfile)
        .arg("-t")
        .arg(format!("{APP_IMAGE_NAME}:{APP_IMAGE_TAG}"))
        .arg(&root)
        .status()
        .await?;
    anyhow::ensure!(status.success(), "docker build exited with {status}");
    Ok(())
}

/// One running instance of a service the harness talks to over its mapped
/// host port (an api replica, a worker's admin port, the mock, the
/// merchant).
pub struct ServiceInstance {
    pub name: String,
    pub container: ContainerAsync<GenericImage>,
    pub host_port: u16,
}

impl ServiceInstance {
    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.host_port)
    }
}

/// A horizontally-scaled role: 1 instance in the default profile, more under
/// `@stack:scaled`.
#[derive(Default)]
pub struct ServiceGroup {
    pub instances: Vec<ServiceInstance>,
}

impl ServiceGroup {
    pub fn len(&self) -> usize {
        self.instances.len()
    }

    pub fn is_empty(&self) -> bool {
        self.instances.is_empty()
    }

    pub fn base_url(&self, idx: usize) -> String {
        self.instances[idx].base_url()
    }
}

/// One scenario's full stack.
pub struct Stack {
    pub scenario_id: String,
    pub network: String,
    pub scaled: bool,

    pub postgres: ContainerAsync<Postgres>,
    pub postgres_host_port: u16,
    pub pg: tokio_postgres::Client,

    pub redis: ContainerAsync<Redis>,
    pub redis_host_port: u16,
    pub redis_name: String,

    pub kafka: ContainerAsync<GenericImage>,
    pub kafka_name: String,

    pub clickhouse: ContainerAsync<ClickHouse>,
    pub clickhouse_host_port: u16,
    pub clickhouse_name: String,

    pub provider_mock: ContainerAsync<GenericImage>,
    pub provider_mock_host_port: u16,
    pub provider_mock_name: String,

    pub demo_merchant: ContainerAsync<GenericImage>,
    pub demo_merchant_host_port: u16,
    pub demo_merchant_name: String,

    pub api: ServiceGroup,
    pub relay: ServiceGroup,
    pub ingester: ServiceGroup,
    pub notifier: ServiceGroup,
    pub reconciler: ServiceGroup,

    /// Control-plane HTTP client (mock, merchant, admin ports): no cookie
    /// jar, no redirects, same as the api-under-test client.
    pub http: reqwest::Client,
}

impl Stack {
    /// Tears the scenario's stack down, awaiting every removal.
    ///
    /// This exists because letting `Stack` fall out of scope **deadlocks the
    /// whole run**, and the failure is invisible: `ContainerAsync`'s `Drop` has
    /// to reach an async Docker call from a synchronous context, so it does
    /// `block_in_place` + `block_on`, and doing that from inside cucumber's
    /// `after` hook — which is already running on the runtime — parks a worker
    /// thread that never comes back. The suite then reports nothing and finishes
    /// never, which is exactly the shape of failure a test harness must not have.
    /// A `sample` of the hung process shows it plainly:
    /// `drop_in_place<Stack>` → `ServiceInstance` → `block_in_place` → parked.
    ///
    /// So every container is removed explicitly, with the library's own async
    /// `rm`, and awaited — this is not a hand-rolled lifecycle, it is the same
    /// library call `Drop` would have made, made from somewhere it can be
    /// awaited. A successful `rm` also marks the container dropped, so its own
    /// `Drop` has nothing left to do and the deadlock above cannot be reached
    /// from here at all.
    ///
    /// `Drop` is still armed (`TESTCONTAINERS_COMMAND=remove`, `tests/api.rs`)
    /// and it matters for exactly two things this method cannot reach: a
    /// container the library cancelled mid-startup, whose handle never arrived
    /// here, and a container whose `rm` above failed — `rm` leaves it marked
    /// undropped, so `Drop` tries once more. There is no Ryuk in
    /// testcontainers-rs 0.25 to be a backstop for either.
    ///
    /// Removal order is the reverse of startup, and a failure to remove one
    /// container does not stop the others: leaving eleven behind because the
    /// first was already gone would turn one flake into a run-wide resource leak.
    ///
    /// The actual removal is [`StackContainers::remove_all`] — the same code
    /// [`Stack::start`] runs on itself when it fails partway, so there is one
    /// place that knows how to tear a stack's containers down, not two that
    /// can drift apart.
    pub async fn shutdown(self) {
        // The database client first: its connection task ends when PostgreSQL
        // goes, and dropping the client is what lets that task finish.
        drop(self.pg);

        StackContainers {
            postgres: self.postgres,
            redis: self.redis,
            redis_name: self.redis_name,
            kafka: self.kafka,
            kafka_name: self.kafka_name,
            clickhouse: self.clickhouse,
            clickhouse_name: self.clickhouse_name,
            provider_mock: self.provider_mock,
            provider_mock_name: self.provider_mock_name,
            demo_merchant: self.demo_merchant,
            demo_merchant_name: self.demo_merchant_name,
            api: self.api,
            relay: self.relay,
            ingester: self.ingester,
            notifier: self.notifier,
            reconciler: self.reconciler,
        }
        .remove_all()
        .await;
    }
}

/// Removes one container, reporting rather than propagating a failure.
async fn remove<I: testcontainers::Image>(container: ContainerAsync<I>, name: &str) {
    if let Err(e) = container.rm().await {
        eprintln!("[apitest] could not remove {name}: {e}");
    }
}

/// Every container a fully-started [`Stack`] holds, minus the fields
/// ([`tokio_postgres::Client`], `reqwest::Client`, ids/names that are not
/// containers) that either are not containers or do not need removing.
///
/// This exists so the removal logic is written once and used from two
/// places: [`Stack::shutdown`], tearing down a stack that lived long enough
/// to run scenario steps, and the tail of [`Stack::start`], tearing down a
/// stack that started every container fine but then failed on something
/// after (connecting to PostgreSQL, building the HTTP client, an ingester
/// never reporting ready). Both leave nothing behind; only the doc comment
/// on the caller differs.
struct StackContainers {
    postgres: ContainerAsync<Postgres>,
    redis: ContainerAsync<Redis>,
    redis_name: String,
    kafka: ContainerAsync<GenericImage>,
    kafka_name: String,
    clickhouse: ContainerAsync<ClickHouse>,
    clickhouse_name: String,
    provider_mock: ContainerAsync<GenericImage>,
    provider_mock_name: String,
    demo_merchant: ContainerAsync<GenericImage>,
    demo_merchant_name: String,
    api: ServiceGroup,
    relay: ServiceGroup,
    ingester: ServiceGroup,
    notifier: ServiceGroup,
    reconciler: ServiceGroup,
}

impl StackContainers {
    /// Removes every container, reverse of start order, awaited; one
    /// container failing to remove does not stop the rest. Identical to what
    /// `Stack::shutdown` did inline before this was pulled out so
    /// `Stack::start` could call it too.
    async fn remove_all(self) {
        for group in [
            self.reconciler,
            self.notifier,
            self.ingester,
            self.relay,
            self.api,
        ] {
            for instance in group.instances {
                remove(instance.container, &instance.name).await;
            }
        }

        remove(self.demo_merchant, &self.demo_merchant_name).await;
        remove(self.provider_mock, &self.provider_mock_name).await;
        remove(self.kafka, &self.kafka_name).await;
        remove(self.clickhouse, &self.clickhouse_name).await;
        remove(self.redis, &self.redis_name).await;
        remove(self.postgres, "postgres").await;
    }
}

/// Splits a phase's per-instance results into what came up and what did
/// not, so a partial failure in that phase still knows exactly what it has
/// to remove — the successes of the failing phase itself, not just the
/// phases before it.
fn partition_started(
    results: Vec<anyhow::Result<ServiceInstance>>,
) -> (Vec<ServiceInstance>, Vec<anyhow::Error>) {
    let mut ok = Vec::new();
    let mut errs = Vec::new();
    for r in results {
        match r {
            Ok(v) => ok.push(v),
            Err(e) => errs.push(e),
        }
    }
    (ok, errs)
}

/// Races one container's own future against `Stack::start`'s total budget
/// (`deadline`, a single absolute instant shared by every future in the
/// stack, not a fresh duration per call) and turns "still not done at the
/// deadline" into an ordinary `Err` naming `what`, instead of letting an
/// outer timeout cancel something bigger.
///
/// This is what makes [`STACK_TOTAL_TIMEOUT`] safe to enforce at all: every
/// future passed through here settles — one way or the other — no later
/// than `deadline`, so a `tokio::join!` of several of them never needs to be
/// raced (and potentially cancelled) as a whole. Whatever this drops on
/// expiry was never turned into a value anyone owns, so there is nothing to
/// leak; whatever the sibling futures produced before `deadline` stays
/// exactly as owned as it always was.
async fn bounded<T>(
    deadline: tokio::time::Instant,
    what: String,
    fut: impl Future<Output = anyhow::Result<T>>,
) -> anyhow::Result<T> {
    match tokio::time::timeout_at(deadline, fut).await {
        Ok(result) => result,
        Err(_) => Err(anyhow::anyhow!(
            "{what} did not finish before the stack's {STACK_TOTAL_TIMEOUT:?} total start \
             budget ran out. Every individual wait in this file is already bounded, so this \
             means the Docker daemon itself was not answering — usually too many stacks \
             running at once for the memory it has. Try a lower PAYGATE_TEST_CONCURRENCY."
        )),
    }
}

/// Builds the error `Stack::start` returns after removing whatever of the
/// scenario's stack had already come up. Folds every failure of the phase
/// (there can be more than one — `tokio::join!` does not stop at the
/// first) into one error, with the first as the reported cause and the
/// rest summarized alongside it, and says plainly that cleanup already
/// happened so nothing here is left running for a caller to wonder about.
fn cleanup_error(errors: Vec<anyhow::Error>) -> anyhow::Error {
    let mut errors = errors.into_iter();
    let first = errors
        .next()
        .expect("cleanup_error is only called with at least one error");
    let rest: Vec<String> = errors.map(|e| format!("{e:#}")).collect();
    let note = if rest.is_empty() {
        "starting the scenario's stack failed; every container already \
         started for it was removed before returning this error"
            .to_string()
    } else {
        format!(
            "starting the scenario's stack failed ({} more container(s) also \
             failed to start: {}); every container already started for it was \
             removed before returning this error",
            rest.len(),
            rest.join("; ")
        )
    };
    first.context(note)
}

impl std::fmt::Debug for Stack {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Stack")
            .field("scenario_id", &self.scenario_id)
            .field("scaled", &self.scaled)
            .finish_non_exhaustive()
    }
}

fn short_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..10].to_string()
}

fn http_ready(path: &str) -> WaitFor {
    WaitFor::http(
        HttpWaitStrategy::new(path)
            .with_port(8080.tcp())
            .with_expected_status_code(200u16),
    )
}

fn common_env(instance_name: &str) -> HashMap<String, String> {
    HashMap::from([
        ("PORT".to_string(), "8080".to_string()),
        ("INSTANCE_ID".to_string(), instance_name.to_string()),
        ("LOG_LEVEL".to_string(), "info".to_string()),
        ("SHUTDOWN_GRACE_SECONDS".to_string(), "1".to_string()),
    ])
}

async fn start_app_container(
    network: &str,
    name: &str,
    cmd: Vec<String>,
    env: HashMap<String, String>,
    ready: WaitFor,
) -> anyhow::Result<ContainerAsync<GenericImage>> {
    let image = GenericImage::new(APP_IMAGE_NAME, APP_IMAGE_TAG)
        .with_exposed_port(8080.tcp())
        .with_wait_for(ready);
    let mut req = image
        .with_cmd(cmd)
        .with_container_name(name)
        .with_network(network)
        .with_startup_timeout(STARTUP_TIMEOUT);
    for (k, v) in env {
        req = req.with_env_var(k, v);
    }
    Ok(req.start().await?)
}

async fn start_service_instance(
    network: &str,
    name: &str,
    cmd: Vec<String>,
    env: HashMap<String, String>,
    ready: WaitFor,
) -> anyhow::Result<ServiceInstance> {
    let container = start_app_container(network, name, cmd, env, ready).await?;
    let host_port = container.get_host_port_ipv4(8080).await?;
    Ok(ServiceInstance {
        name: name.to_string(),
        container,
        host_port,
    })
}

/// The database name/user/password every scenario's PostgreSQL uses.
const PG_DB: &str = "paygate";
const PG_USER: &str = "paygate";
const PG_PASSWORD: &str = "paygate";

/// ClickHouse's default database, used as-is rather than creating a new one:
/// the migrations create tables inside whichever database `CLICKHOUSE_URL`
/// points at.
const CH_DB: &str = "default";
const CH_USER: &str = "default";

const KAFKA_TOPIC: &str = "paygate.payment-events.v1";

impl Stack {
    /// Starts a full stack for one scenario. `scaled` selects the
    /// `@stack:scaled` profile (api ×3, relay/ingester/notifier/reconciler
    /// ×2, 3 partitions either way).
    ///
    /// Invariant this whole function is built around: it either returns a
    /// running `Stack`, or it has already removed every container it
    /// started and returns `Err`. There is no third outcome where it fails
    /// having left something running — callers (see `tests/api.rs`'s
    /// `before` hook) rely on that to decide whether there is anything to
    /// pass to `Stack::shutdown` at all. Do not wrap this call in an outer
    /// `tokio::time::timeout` or anything else that can cancel it: a
    /// cancelled future is dropped, not returned, and cannot honor this
    /// invariant (see `STACK_TOTAL_TIMEOUT`, `bounded`).
    pub async fn start(scaled: bool) -> anyhow::Result<Self> {
        ensure_image_built().await;

        // One absolute deadline for the whole function — see
        // `STACK_TOTAL_TIMEOUT` and `bounded` for why this is raced per
        // container rather than around the call to `start` itself.
        let deadline = tokio::time::Instant::now() + STACK_TOTAL_TIMEOUT;

        let scenario_id = short_id();
        // ONE network for the whole run, not one per scenario.
        //
        // This is forced by how testcontainers-rs cleans up. `Network` removes
        // itself in `Drop`, and to reach an async Docker call from a synchronous
        // `Drop` it goes through `core::async_drop`, which either does
        // `block_in_place` + `block_on` (multi-threaded runtime) or blocks on a
        // channel while a drop-worker thread does the work (current-thread
        // runtime). Both deadlock here, and this suite hit both in turn: removing
        // a scenario's last container drops the final `Arc<Network>`, and the
        // drop-worker then needs a process-global `tokio::Mutex` that a task on
        // the blocked runtime is holding.
        //
        // `Network::drop` never runs at all now, because the library never owns
        // this network: `ensure_run_network` creates it before any container and
        // `Network::new` returns `None` for a network that already exists. One
        // shared network for a whole run rather than 174, no `Arc<Network>` to
        // drop, and nothing for the drop-worker to deadlock on — while `Drop`
        // stays armed for the containers, which is what stops a cancelled
        // startup leaking one.
        //
        // Isolation does not rest on the network. It rests on every container
        // being this scenario's own: each name carries the scenario id, nothing
        // addresses another scenario's containers, and a scenario's own
        // PostgreSQL, Redis, Kafka and ClickHouse are started and removed with
        // it. `examples/minimart-go-nuxt` shares one bridge for a whole run too,
        // for its own reason (creating one while a browser is running aborts the
        // browser's requests).
        let network = RUN_NETWORK.to_string();

        let n_api = if scaled { 3 } else { 1 };
        let n_worker = if scaled { 2 } else { 1 };

        let pg_name = format!("pg-{scenario_id}");
        let redis_name = format!("redis-{scenario_id}");
        let kafka_name = format!("kafka-{scenario_id}");
        let ch_name = format!("ch-{scenario_id}");
        let mock_name = format!("mock-{scenario_id}");
        let merchant_name = format!("merchant-{scenario_id}");
        let api_names: Vec<String> = (1..=n_api)
            .map(|i| format!("api-{scenario_id}-{i}"))
            .collect();
        let relay_names: Vec<String> = (1..=n_worker)
            .map(|i| format!("relay-{scenario_id}-{i}"))
            .collect();
        let ingest_names: Vec<String> = (1..=n_worker)
            .map(|i| format!("ingest-{scenario_id}-{i}"))
            .collect();
        let notify_names: Vec<String> = (1..=n_worker)
            .map(|i| format!("notify-{scenario_id}-{i}"))
            .collect();
        let reconcile_names: Vec<String> = (1..=n_worker)
            .map(|i| format!("reconcile-{scenario_id}-{i}"))
            .collect();

        // ── Phase A: the data stores, in parallel ───────────────────────────
        let postgres_fut = {
            let network = network.clone();
            let pg_name = pg_name.clone();
            async move {
                let container = Postgres::default()
                    .with_db_name(PG_DB)
                    .with_user(PG_USER)
                    .with_password(PG_PASSWORD)
                    // Pinned like every other image in this stack, and after the
                    // module's own builders because `with_tag` is `ImageExt`'s and
                    // returns a `ContainerRequest`. Without it the module default
                    // (11-alpine) is used, so the suite would run against a major
                    // version nobody chose and which is not the one
                    // local/docker-compose.yml runs.
                    .with_tag("17-alpine")
                    .with_container_name(&pg_name)
                    .with_network(&network)
                    .with_startup_timeout(STARTUP_TIMEOUT)
                    .start()
                    .await?;
                let port = container.get_host_port_ipv4(5432).await?;
                Ok::<_, anyhow::Error>((container, port))
            }
        };
        let redis_fut = {
            let network = network.clone();
            let redis_name = redis_name.clone();
            async move {
                let container = Redis::default()
                    .with_tag("7-alpine")
                    // No persistence, as the deployment has none (spec.md,
                    // "Redis — nothing that matters"). This is not decoration:
                    // the module's default command saves an RDB file on SIGTERM,
                    // so `service "redis" is stopped` followed by `started`
                    // would silently bring the rate limiter's buckets BACK — and
                    // `resilience.feature` asserts the opposite, that a restart
                    // is a burst because the buckets come back full. Without
                    // this the scenario passes while proving nothing.
                    .with_cmd(vec!["redis-server", "--save", "", "--appendonly", "no"])
                    .with_container_name(&redis_name)
                    .with_network(&network)
                    .with_startup_timeout(STARTUP_TIMEOUT)
                    .start()
                    .await?;
                let port = container.get_host_port_ipv4(6379).await?;
                Ok::<_, anyhow::Error>((container, port))
            }
        };
        let clickhouse_fut = {
            let network = network.clone();
            let ch_name = ch_name.clone();
            async move {
                let container = ClickHouse::default()
                    .with_tag("24.8-alpine")
                    // Without this the image writes
                    // /etc/clickhouse-server/users.d/default-user.xml pinning the
                    // `default` user to ::1 and 127.0.0.1, so every request from
                    // outside the container is refused — and refused as
                    // AUTHENTICATION_FAILED ("password is incorrect, or there is
                    // no user with such name"), which sends you looking for a
                    // password that was never the problem. Skipping the setup
                    // leaves `default` reachable with no password, which is what
                    // CLICKHOUSE_PASSWORD="" in every container's environment
                    // already says (spec.md, "Environment variables").
                    .with_env_var("CLICKHOUSE_SKIP_USER_SETUP", "1")
                    // Cap what ClickHouse will reserve. Left alone it sizes
                    // itself from the RAM it can SEE — 90% of the whole Docker
                    // VM — and two scenarios' worth of that, plus a Kafka broker
                    // each, is more than the machine has. The symptom is not a
                    // ClickHouse error: it is some other container in the stack
                    // dying during startup, which surfaced here as Kafka's log
                    // stream ending before it ever said "Kafka Server started".
                    //
                    // A `--max_server_memory_usage` argument is rejected by this
                    // image ("Unknown option specified"), so it goes in as a
                    // config drop-in, which is how the server is meant to be
                    // configured and is verifiable: `system.server_settings`
                    // reports the value back.
                    .with_copy_to(
                        "/etc/clickhouse-server/config.d/memory.xml",
                        CLICKHOUSE_MEMORY_CAP.as_bytes().to_vec(),
                    )
                    .with_container_name(&ch_name)
                    .with_network(&network)
                    .with_ulimit("nofile", 262144, Some(262144))
                    .with_startup_timeout(STARTUP_TIMEOUT)
                    .start()
                    .await?;
                let port = container.get_host_port_ipv4(8123).await?;
                Ok::<_, anyhow::Error>((container, port))
            }
        };
        let kafka_fut = {
            let network = network.clone();
            let kafka_name = kafka_name.clone();
            async move {
                let env: HashMap<String, String> = [
                    ("KAFKA_NODE_ID", "1"),
                    ("KAFKA_PROCESS_ROLES", "broker,controller"),
                    ("KAFKA_LISTENERS", "PLAINTEXT://:9092,CONTROLLER://:9093"),
                    ("KAFKA_CONTROLLER_LISTENER_NAMES", "CONTROLLER"),
                    (
                        "KAFKA_LISTENER_SECURITY_PROTOCOL_MAP",
                        "CONTROLLER:PLAINTEXT,PLAINTEXT:PLAINTEXT",
                    ),
                    ("KAFKA_INTER_BROKER_LISTENER_NAME", "PLAINTEXT"),
                    ("KAFKA_OFFSETS_TOPIC_REPLICATION_FACTOR", "1"),
                    ("KAFKA_TRANSACTION_STATE_LOG_REPLICATION_FACTOR", "1"),
                    ("KAFKA_TRANSACTION_STATE_LOG_MIN_ISR", "1"),
                    ("KAFKA_GROUP_INITIAL_REBALANCE_DELAY_MS", "0"),
                    ("KAFKA_AUTO_CREATE_TOPICS_ENABLE", "false"),
                    ("CLUSTER_ID", "5L6g3nShT-eMCtK--X86sw"),
                ]
                .into_iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
                let mut req = GenericImage::new("apache/kafka-native", "3.9.0")
                    .with_wait_for(WaitFor::message_on_stdout("Kafka Server started"))
                    .with_container_name(&kafka_name)
                    .with_network(&network)
                    .with_env_var(
                        "KAFKA_ADVERTISED_LISTENERS",
                        format!("PLAINTEXT://{kafka_name}:9092"),
                    )
                    .with_env_var(
                        "KAFKA_CONTROLLER_QUORUM_VOTERS",
                        format!("1@{kafka_name}:9093"),
                    )
                    .with_startup_timeout(STARTUP_TIMEOUT);
                for (k, v) in env {
                    req = req.with_env_var(k, v);
                }
                let container = req.start().await?;
                Ok::<_, anyhow::Error>(container)
            }
        };

        // `join!`, not `try_join!`: every store gets a chance to finish
        // starting even if a sibling fails, so a failure here can remove
        // whatever else came up instead of leaking it (see `spec.md`,
        // "Isolation", and this module's doc comment on `Stack::shutdown`).
        // Each is also
        // individually raced against the shared `deadline` through
        // `bounded`, so a daemon that never answers one of these becomes an
        // ordinary error here rather than something that has to cancel — and
        // possibly lose — the other three.
        let (postgres_res, redis_res, clickhouse_res, kafka_res) = tokio::join!(
            bounded(deadline, "postgres".to_string(), postgres_fut),
            bounded(deadline, "redis".to_string(), redis_fut),
            bounded(deadline, "clickhouse".to_string(), clickhouse_fut),
            bounded(deadline, "kafka".to_string(), kafka_fut),
        );

        let mut phase_a_errors: Vec<anyhow::Error> = Vec::new();
        let postgres_ok = match postgres_res {
            Ok(v) => Some(v),
            Err(e) => {
                phase_a_errors.push(e);
                None
            }
        };
        let redis_ok = match redis_res {
            Ok(v) => Some(v),
            Err(e) => {
                phase_a_errors.push(e);
                None
            }
        };
        let clickhouse_ok = match clickhouse_res {
            Ok(v) => Some(v),
            Err(e) => {
                phase_a_errors.push(e);
                None
            }
        };
        let kafka_ok = match kafka_res {
            Ok(v) => Some(v),
            Err(e) => {
                phase_a_errors.push(e);
                None
            }
        };

        if !phase_a_errors.is_empty() {
            // Reverse of start order; whichever of these did come up gets
            // removed before the failure is reported.
            if let Some(c) = kafka_ok {
                remove(c, &kafka_name).await;
            }
            if let Some((c, _)) = clickhouse_ok {
                remove(c, &ch_name).await;
            }
            if let Some((c, _)) = redis_ok {
                remove(c, &redis_name).await;
            }
            if let Some((c, _)) = postgres_ok {
                remove(c, "postgres").await;
            }
            return Err(cleanup_error(phase_a_errors));
        }

        let (postgres, postgres_host_port) = postgres_ok.expect("checked above");
        let (redis, redis_host_port) = redis_ok.expect("checked above");
        let (clickhouse, clickhouse_host_port) = clickhouse_ok.expect("checked above");
        let kafka = kafka_ok.expect("checked above");

        // ── Phase B: the two counterparties, in parallel with the app layer ─
        let mock_env = common_env(&mock_name);
        let mock_fut = start_service_instance(
            &network,
            &mock_name,
            vec!["paygate-provider-mock".to_string()],
            {
                let mut e = mock_env;
                e.insert(
                    "PROVIDER_CALLBACK_URLS".to_string(),
                    api_names
                        .iter()
                        .map(|n| format!("http://{n}:8080"))
                        .collect::<Vec<_>>()
                        .join(","),
                );
                // "the mock is told the same pair" (spec.md, "Environment
                // variables"). Set explicitly rather than left to the mock's
                // defaults, because the coupling to what every feature's
                // Background seeds into `providers` is the whole reason the
                // suite's "paygate signed it correctly" assertions mean
                // anything — and a coupling that matters should be visible.
                e.insert("ECPAY_PLATFORM_ID".to_string(), "3002607".to_string());
                e.insert("ECPAY_HASH_KEY".to_string(), "pwFHCqoQZGmho4w6".to_string());
                e.insert("ECPAY_HASH_IV".to_string(), "EkRm7iFT261dpevs".to_string());
                e.insert("NEWEBPAY_PLATFORM_ID".to_string(), "MS12345678".to_string());
                e.insert(
                    "NEWEBPAY_HASH_KEY".to_string(),
                    "Fs5cX1TGqYZ3kLmN8pQrS2tUvW4xY6zA".to_string(),
                );
                e.insert(
                    "NEWEBPAY_HASH_IV".to_string(),
                    "B7cD9eF1gH3iJ5kL".to_string(),
                );
                e
            },
            http_ready("/provider/__control/requests"),
        );

        let merchant_env = {
            let mut e = common_env(&merchant_name);
            e.insert(
                "GATEWAY_URL".to_string(),
                format!("http://{}:8080", api_names[0]),
            );
            e.insert(
                "DEMO_MERCHANT_API_KEY".to_string(),
                FIXTURE_MERCHANT_API_KEY.to_string(),
            );
            e.insert(
                "MERCHANT_HASH_KEY".to_string(),
                FIXTURE_MERCHANT_HASH_KEY.to_string(),
            );
            e.insert(
                "MERCHANT_HASH_IV".to_string(),
                FIXTURE_MERCHANT_HASH_IV.to_string(),
            );
            // `/notify-slow` answers correctly but just too late, so it has to
            // know the deadline it is meant to miss. Same value as the
            // notifier's below: 500 ms rather than the production 2000, because
            // a merchant that is retried five times at two seconds each would
            // spend most of `background work has settled`'s 30-second budget
            // waiting (spec.md, NFR-OLAP-1).
            e.insert("NOTIFY_TIMEOUT_MS".to_string(), "500".to_string());
            e
        };
        let merchant_fut = start_service_instance(
            &network,
            &merchant_name,
            vec!["paygate-demo-merchant".to_string()],
            merchant_env,
            http_ready("/demo-merchant/api/__deliveries"),
        );

        let (mock_res, merchant_res) = tokio::join!(
            bounded(deadline, "provider-mock".to_string(), mock_fut),
            bounded(deadline, "demo-merchant".to_string(), merchant_fut),
        );

        let mut phase_b_errors: Vec<anyhow::Error> = Vec::new();
        let mock_ok = match mock_res {
            Ok(v) => Some(v),
            Err(e) => {
                phase_b_errors.push(e);
                None
            }
        };
        let merchant_ok = match merchant_res {
            Ok(v) => Some(v),
            Err(e) => {
                phase_b_errors.push(e);
                None
            }
        };

        if !phase_b_errors.is_empty() {
            // Whichever of the mock/merchant came up, then all of phase A —
            // which fully succeeded, or phase B would never have started.
            if let Some(inst) = merchant_ok {
                remove(inst.container, &inst.name).await;
            }
            if let Some(inst) = mock_ok {
                remove(inst.container, &inst.name).await;
            }
            remove(kafka, &kafka_name).await;
            remove(clickhouse, &ch_name).await;
            remove(redis, &redis_name).await;
            remove(postgres, "postgres").await;
            return Err(cleanup_error(phase_b_errors));
        }

        let mock_instance = mock_ok.expect("checked above");
        let merchant_instance = merchant_ok.expect("checked above");
        let ServiceInstance {
            container: provider_mock,
            host_port: provider_mock_host_port,
            ..
        } = mock_instance;
        let ServiceInstance {
            container: demo_merchant,
            host_port: demo_merchant_host_port,
            ..
        } = merchant_instance;

        // ── Phase C: api replicas and every worker role, in parallel ───────
        let pg_dsn = format!("postgres://{PG_USER}:{PG_PASSWORD}@{pg_name}:5432/{PG_DB}");
        let redis_url = format!("redis://{redis_name}:6379");
        let ch_url = format!("http://{ch_name}:8123");
        let provider_base = format!("http://{mock_name}:8080");
        let merchant_base = format!("http://{merchant_name}:8080");
        let kafka_brokers = format!("{kafka_name}:9092");

        let mut api_futs = Vec::new();
        for name in &api_names {
            let mut env = common_env(name);
            env.insert("DB_DSN".to_string(), pg_dsn.clone());
            env.insert("DB_POOL_MAX".to_string(), "20".to_string());
            env.insert("SCHEMA_AUTO_MIGRATE".to_string(), "false".to_string());
            env.insert("REDIS_URL".to_string(), redis_url.clone());
            env.insert("REDIS_TIMEOUT_MS".to_string(), "100".to_string());
            env.insert("CLICKHOUSE_URL".to_string(), ch_url.clone());
            env.insert("CLICKHOUSE_DATABASE".to_string(), CH_DB.to_string());
            env.insert("CLICKHOUSE_USER".to_string(), CH_USER.to_string());
            env.insert("CLICKHOUSE_PASSWORD".to_string(), String::new());
            env.insert("PROVIDER_TRADE_NO_PREFIX".to_string(), "PG".to_string());
            env.insert("PUBLIC_BASE_URL".to_string(), format!("http://{name}:8080"));
            env.insert("PSP_TIMEOUT_MS".to_string(), "1000".to_string());
            env.insert("SESSION_TTL_SECONDS".to_string(), "28800".to_string());
            env.insert("API_KEY_CACHE_TTL_SECONDS".to_string(), "60".to_string());
            env.insert("IDEMPOTENCY_TTL_HOURS".to_string(), "24".to_string());
            env.insert("IDEMPOTENCY_LOCK_TTL_MS".to_string(), "30000".to_string());
            env.insert("COOKIE_SECURE".to_string(), "false".to_string());
            env.insert("FRONTEND_ORIGIN".to_string(), "*".to_string());
            env.insert("PROVIDER_BASE_URL".to_string(), provider_base.clone());
            api_futs.push(bounded(
                deadline,
                name.clone(),
                start_service_instance(
                    &network,
                    name,
                    vec!["paygate-api".to_string()],
                    env,
                    http_ready("/api/v1/health/live"),
                ),
            ));
        }

        let mut relay_futs = Vec::new();
        for name in &relay_names {
            let mut env = common_env(name);
            env.insert("DB_DSN".to_string(), pg_dsn.clone());
            env.insert("DB_POOL_MAX".to_string(), "20".to_string());
            env.insert("KAFKA_BROKERS".to_string(), kafka_brokers.clone());
            env.insert("KAFKA_TOPIC".to_string(), KAFKA_TOPIC.to_string());
            env.insert("KAFKA_TOPIC_PARTITIONS".to_string(), "3".to_string());
            env.insert("RELAY_BATCH_SIZE".to_string(), "100".to_string());
            env.insert("RELAY_POLL_INTERVAL_MS".to_string(), "50".to_string());
            relay_futs.push(bounded(
                deadline,
                name.clone(),
                start_service_instance(
                    &network,
                    name,
                    vec!["paygate-worker".to_string(), "relay".to_string()],
                    env,
                    http_ready("/healthz"),
                ),
            ));
        }

        let mut ingest_futs = Vec::new();
        for name in &ingest_names {
            let mut env = common_env(name);
            env.insert("KAFKA_BROKERS".to_string(), kafka_brokers.clone());
            env.insert("KAFKA_TOPIC".to_string(), KAFKA_TOPIC.to_string());
            env.insert("KAFKA_GROUP_ID".to_string(), "paygate-reports".to_string());
            env.insert("INGEST_BATCH_MAX".to_string(), "500".to_string());
            env.insert("INGEST_BATCH_WAIT_MS".to_string(), "200".to_string());
            env.insert("CLICKHOUSE_URL".to_string(), ch_url.clone());
            env.insert("CLICKHOUSE_DATABASE".to_string(), CH_DB.to_string());
            env.insert("CLICKHOUSE_USER".to_string(), CH_USER.to_string());
            env.insert("CLICKHOUSE_PASSWORD".to_string(), String::new());
            ingest_futs.push(bounded(
                deadline,
                name.clone(),
                start_service_instance(
                    &network,
                    name,
                    vec!["paygate-worker".to_string(), "ingest".to_string()],
                    env,
                    http_ready("/healthz"),
                ),
            ));
        }

        let mut notify_futs = Vec::new();
        for name in &notify_names {
            let mut env = common_env(name);
            env.insert("DB_DSN".to_string(), pg_dsn.clone());
            env.insert("DB_POOL_MAX".to_string(), "20".to_string());
            env.insert("NOTIFY_MAX_ATTEMPTS".to_string(), "5".to_string());
            env.insert(
                "NOTIFY_BACKOFF_MS".to_string(),
                "50,100,200,400".to_string(),
            );
            env.insert("NOTIFY_TIMEOUT_MS".to_string(), "500".to_string());
            env.insert("NOTIFY_POLL_INTERVAL_MS".to_string(), "50".to_string());
            env.insert("MERCHANT_BASE_URL".to_string(), merchant_base.clone());
            notify_futs.push(bounded(
                deadline,
                name.clone(),
                start_service_instance(
                    &network,
                    name,
                    vec!["paygate-worker".to_string(), "notify".to_string()],
                    env,
                    http_ready("/healthz"),
                ),
            ));
        }

        let mut reconcile_futs = Vec::new();
        for name in &reconcile_names {
            let mut env = common_env(name);
            // `DB_DSN`/`DB_POOL_MAX` are not named for the reconciler in
            // spec.md's "Environment variables" table, but the reconciler
            // reads and writes `payment_attempts`/`payments`/`provider_queries`
            // directly, so it cannot work without a database handle. Reported
            // as a probable spec gap; supplied here so the reconciler role has
            // what it needs to do its documented job.
            env.insert("DB_DSN".to_string(), pg_dsn.clone());
            env.insert("DB_POOL_MAX".to_string(), "20".to_string());
            env.insert("RECONCILE_AFTER_MINUTES".to_string(), "60".to_string());
            env.insert("RECONCILE_RETRY_MINUTES".to_string(), "60".to_string());
            env.insert("RECONCILE_POLL_INTERVAL_MS".to_string(), "0".to_string());
            env.insert("RECONCILE_BATCH_SIZE".to_string(), "50".to_string());
            env.insert("PROVIDER_BASE_URL".to_string(), provider_base.clone());
            reconcile_futs.push(bounded(
                deadline,
                name.clone(),
                start_service_instance(
                    &network,
                    name,
                    vec!["paygate-worker".to_string(), "reconcile".to_string()],
                    env,
                    http_ready("/healthz"),
                ),
            ));
        }

        // `join_all`, not `try_join_all`: the latter drops every future the
        // moment one resolves to `Err`, which is exactly how a slow api
        // replica used to leak nine fast ones. Letting all five roles finish
        // means every instance that did come up is still ours to remove.
        // Every future pushed above is also individually `bounded` against
        // `deadline`, so "finish" here is guaranteed rather than hoped for.
        let (api_results, relay_results, ingest_results, notify_results, reconcile_results) = tokio::join!(
            join_all(api_futs),
            join_all(relay_futs),
            join_all(ingest_futs),
            join_all(notify_futs),
            join_all(reconcile_futs),
        );

        let mut phase_c_errors: Vec<anyhow::Error> = Vec::new();
        let (api_ok, errs) = partition_started(api_results);
        phase_c_errors.extend(errs);
        let (relay_ok, errs) = partition_started(relay_results);
        phase_c_errors.extend(errs);
        let (ingest_ok, errs) = partition_started(ingest_results);
        phase_c_errors.extend(errs);
        let (notify_ok, errs) = partition_started(notify_results);
        phase_c_errors.extend(errs);
        let (reconcile_ok, errs) = partition_started(reconcile_results);
        phase_c_errors.extend(errs);

        if !phase_c_errors.is_empty() {
            // Every replica/worker that did come up, across all five roles
            // (reverse of role-start order), then phase B's mock/merchant
            // and phase A's data stores — all of which fully succeeded, or
            // phase C would never have started.
            for group in [reconcile_ok, notify_ok, ingest_ok, relay_ok, api_ok] {
                for inst in group {
                    remove(inst.container, &inst.name).await;
                }
            }
            remove(demo_merchant, &merchant_name).await;
            remove(provider_mock, &mock_name).await;
            remove(kafka, &kafka_name).await;
            remove(clickhouse, &ch_name).await;
            remove(redis, &redis_name).await;
            remove(postgres, "postgres").await;
            return Err(cleanup_error(phase_c_errors));
        }

        let api = ServiceGroup { instances: api_ok };
        let relay = ServiceGroup {
            instances: relay_ok,
        };
        let ingester = ServiceGroup {
            instances: ingest_ok,
        };
        let notifier = ServiceGroup {
            instances: notify_ok,
        };
        let reconciler = ServiceGroup {
            instances: reconcile_ok,
        };

        // Every container is up now. From here on a failure is no longer a
        // partial-phase problem — it is "the whole stack came up and then
        // something else broke" — so the rest of this function tracks every
        // container in one `StackContainers` and removes all of it, the same
        // way `Stack::shutdown` would, before returning any `Err`.
        let containers = StackContainers {
            postgres,
            redis,
            redis_name,
            kafka,
            kafka_name,
            clickhouse,
            clickhouse_name: ch_name,
            provider_mock,
            provider_mock_name: mock_name,
            demo_merchant,
            demo_merchant_name: merchant_name,
            api,
            relay,
            ingester,
            notifier,
            reconciler,
        };

        let http = match reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .cookie_store(false)
            .build()
        {
            Ok(http) => http,
            Err(e) => {
                containers.remove_all().await;
                return Err(cleanup_error(vec![e.into()]));
            }
        };

        // The stack is not ready until every ingester holds a partition
        // assignment (`crates/worker/src/admin.rs`: `GET /status` reports
        // `ready: true` only once `Consumer::status` says so) — a rule worth
        // holding for the unscaled profile too, since a single ingester
        // racing its own consumer-group join is exactly the kind of flake
        // this whole design exists to rule out.
        for inst in &containers.ingester.instances {
            let what = format!("{} readiness", inst.name);
            if let Err(e) = bounded(deadline, what, wait_ingester_ready(&http, inst)).await {
                containers.remove_all().await;
                return Err(cleanup_error(vec![e]));
            }
        }

        let pg = match bounded(
            deadline,
            "connecting to postgres".to_string(),
            connect_pg(postgres_host_port),
        )
        .await
        {
            Ok(pg) => pg,
            Err(e) => {
                containers.remove_all().await;
                return Err(cleanup_error(vec![e]));
            }
        };

        let StackContainers {
            postgres,
            redis,
            redis_name,
            kafka,
            kafka_name,
            clickhouse,
            clickhouse_name,
            provider_mock,
            provider_mock_name,
            demo_merchant,
            demo_merchant_name,
            api,
            relay,
            ingester,
            notifier,
            reconciler,
        } = containers;

        Ok(Stack {
            scenario_id,
            network,
            scaled,
            postgres,
            postgres_host_port,
            pg,
            redis,
            redis_host_port,
            redis_name,
            kafka,
            kafka_name,
            clickhouse,
            clickhouse_host_port,
            clickhouse_name,
            provider_mock,
            provider_mock_host_port,
            provider_mock_name,
            demo_merchant,
            demo_merchant_host_port,
            demo_merchant_name,
            api,
            relay,
            ingester,
            notifier,
            reconciler,
            http,
        })
    }

    pub fn clickhouse_base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.clickhouse_host_port)
    }

    pub fn provider_mock_base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.provider_mock_host_port)
    }

    pub fn demo_merchant_base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.demo_merchant_host_port)
    }

    /// Runs a one-shot `paygate-worker rebuild reports` container to
    /// completion (`WaitFor::Exit`), on the same network and pointed at the
    /// same stores as everything else, then removes it.
    pub async fn rebuild_reports(&self) -> anyhow::Result<()> {
        let pg_dsn = format!(
            "postgres://{PG_USER}:{PG_PASSWORD}@{}:5432/{PG_DB}",
            self.postgres_container_name()
        );
        let ch_url = format!("http://{}:8123", self.clickhouse_name);
        let name = format!("rebuild-{}", self.scenario_id);
        let mut env = common_env(&name);
        env.insert("DB_DSN".to_string(), pg_dsn);
        env.insert("CLICKHOUSE_URL".to_string(), ch_url);
        env.insert("CLICKHOUSE_DATABASE".to_string(), CH_DB.to_string());
        env.insert("CLICKHOUSE_USER".to_string(), CH_USER.to_string());
        env.insert("CLICKHOUSE_PASSWORD".to_string(), String::new());

        let mut req = GenericImage::new(APP_IMAGE_NAME, APP_IMAGE_TAG)
            .with_wait_for(WaitFor::Exit(
                testcontainers::core::wait::ExitWaitStrategy::new().with_exit_code(0),
            ))
            .with_cmd(vec![
                "paygate-worker".to_string(),
                "rebuild".to_string(),
                "reports".to_string(),
            ])
            .with_container_name(&name)
            .with_network(&self.network)
            .with_startup_timeout(Duration::from_secs(60));
        for (k, v) in env {
            req = req.with_env_var(k, v);
        }
        let container = req.start().await?;
        let stdout = container.stdout_to_vec().await.unwrap_or_default();
        let _ = container.rm().await;
        anyhow::ensure!(
            String::from_utf8_lossy(&stdout).contains("rebuild complete"),
            "rebuild container exited without logging \"rebuild complete\"; stdout:\n{}",
            String::from_utf8_lossy(&stdout)
        );
        Ok(())
    }

    fn postgres_container_name(&self) -> String {
        format!("pg-{}", self.scenario_id)
    }

    /// Runs one reconciliation pass on every reconciler instance at once
    /// (SKIP LOCKED means only one of them ever claims a given row — see
    /// `scaling.feature`, "Two reconcilers racing").
    /// Every reconciler instance's `POST /run` URL. Owned, so that an actor in
    /// a race (`these things happen at one instant:`) can run a pass without
    /// holding a borrow of the stack across the gate.
    pub fn reconciler_run_urls(&self) -> Vec<String> {
        self.reconciler
            .instances
            .iter()
            .map(|inst| format!("{}/run", inst.base_url()))
            .collect()
    }

    pub async fn run_reconciler(&self) -> anyhow::Result<()> {
        let mut calls = Vec::new();
        for url in self.reconciler_run_urls() {
            calls.push(self.http.post(url).send());
        }
        let results = futures::future::join_all(calls).await;
        let mut any_ok = false;
        let mut errors = Vec::new();
        for r in results {
            match r {
                Ok(resp) if resp.status().is_success() => any_ok = true,
                Ok(resp) => errors.push(format!("reconciler /run answered {}", resp.status())),
                Err(e) => errors.push(format!("reconciler /run failed: {e}")),
            }
        }
        anyhow::ensure!(
            any_ok,
            "no reconciler instance completed a pass: {}",
            errors.join("; ")
        );
        Ok(())
    }

    /// Stops or starts every container of `service`, per `service_state`.
    pub async fn set_service_state(
        &mut self,
        service: &str,
        want_running: bool,
    ) -> anyhow::Result<()> {
        match service {
            "redis" => {
                set_container_state(&self.redis, want_running).await?;
                if want_running {
                    self.redis_host_port = self.redis.get_host_port_ipv4(6379).await?;
                    wait_tcp_ready(self.redis_host_port).await?;
                }
            }
            "clickhouse" => {
                set_container_state(&self.clickhouse, want_running).await?;
                if want_running {
                    self.clickhouse_host_port = self.clickhouse.get_host_port_ipv4(8123).await?;
                    wait_http_ready(&self.http, &format!("{}/", self.clickhouse_base_url()))
                        .await?;
                }
            }
            "api" => {
                set_group_state(
                    &self.http,
                    &mut self.api,
                    want_running,
                    "/api/v1/health/live",
                )
                .await?;
            }
            "relay" => {
                set_group_state(&self.http, &mut self.relay, want_running, "/healthz").await?;
            }
            "ingester" => {
                set_group_state(&self.http, &mut self.ingester, want_running, "/healthz").await?;
                if want_running {
                    for inst in &self.ingester.instances {
                        wait_ingester_ready(&self.http, inst).await?;
                    }
                }
            }
            "notifier" => {
                set_group_state(&self.http, &mut self.notifier, want_running, "/healthz").await?;
            }
            other => anyhow::bail!("service_state: unknown service {other:?}"),
        }
        Ok(())
    }
}

async fn set_container_state<I: Image>(
    container: &ContainerAsync<I>,
    want_running: bool,
) -> anyhow::Result<()> {
    let running = container.is_running().await?;
    anyhow::ensure!(
        running != want_running,
        "service is already {}",
        if want_running { "started" } else { "stopped" }
    );
    if want_running {
        container.start().await?;
    } else {
        container.stop().await?;
    }
    Ok(())
}

async fn set_group_state(
    http: &reqwest::Client,
    group: &mut ServiceGroup,
    want_running: bool,
    ready_path: &str,
) -> anyhow::Result<()> {
    for inst in &group.instances {
        set_container_state(&inst.container, want_running).await?;
    }
    if want_running {
        for inst in &mut group.instances {
            inst.host_port = inst.container.get_host_port_ipv4(8080).await?;
            let url = format!("{}{}", inst.base_url(), ready_path);
            wait_http_ready(http, &url).await?;
        }
    }
    Ok(())
}

async fn wait_http_ready(http: &reqwest::Client, url: &str) -> anyhow::Result<()> {
    let deadline = tokio::time::Instant::now() + RESTART_READY_TIMEOUT;
    loop {
        if let Ok(resp) = http.get(url).send().await {
            if resp.status().is_success() {
                return Ok(());
            }
        }
        if tokio::time::Instant::now() >= deadline {
            anyhow::bail!("{url} did not become ready within {RESTART_READY_TIMEOUT:?}");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn wait_ingester_ready(http: &reqwest::Client, inst: &ServiceInstance) -> anyhow::Result<()> {
    let url = format!("{}/status", inst.base_url());
    let deadline = tokio::time::Instant::now() + RESTART_READY_TIMEOUT;
    loop {
        if let Ok(resp) = http.get(&url).send().await {
            if let Ok(json) = resp.json::<serde_json::Value>().await {
                if json.get("ready").and_then(|v| v.as_bool()) == Some(true) {
                    return Ok(());
                }
            }
        }
        if tokio::time::Instant::now() >= deadline {
            anyhow::bail!(
                "{} did not report ready:true within {RESTART_READY_TIMEOUT:?}",
                inst.name
            );
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// A bare TCP connect, used as Redis's own readiness probe after a restart —
/// good enough because the harness never speaks RESP to Redis itself
/// (format.yml: "No Redis assertion").
async fn wait_tcp_ready(port: u16) -> anyhow::Result<()> {
    let deadline = tokio::time::Instant::now() + RESTART_READY_TIMEOUT;
    loop {
        if let Ok(mut stream) = tokio::net::TcpStream::connect(("127.0.0.1", port)).await {
            let _ = stream.write_all(b"PING\r\n").await;
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            anyhow::bail!(
                "127.0.0.1:{port} did not accept a connection within {RESTART_READY_TIMEOUT:?}"
            );
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn connect_pg(host_port: u16) -> anyhow::Result<tokio_postgres::Client> {
    let dsn = format!(
        "host=127.0.0.1 port={host_port} user={PG_USER} password={PG_PASSWORD} dbname={PG_DB}"
    );
    let (client, connection) = tokio_postgres::connect(&dsn, tokio_postgres::NoTls).await?;
    tokio::spawn(async move {
        // The connection ends when the scenario's PostgreSQL container is stopped,
        // which is every scenario's last act — so a error here is the expected
        // shape of a normal teardown, not a fault, and printing it for all 168
        // scenarios buries the one line that matters in a failing run.
        let _ = connection.await;
    });
    Ok(client)
}
