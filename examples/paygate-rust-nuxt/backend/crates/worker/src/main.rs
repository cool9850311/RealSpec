//! `paygate-worker <role>` — the relay, the ingester, the notifier, the
//! reconciler, and the one-shot `rebuild reports` command (`spec.md`,
//! "System overview": the binary/role table). One binary, one
//! `[[bin]]`, dispatching on its own argument list rather than needing four
//! more crates — the same shape `api`, `provider-mock` and `demo-merchant`
//! each are, just with more than one role inside this particular one.

mod admin;
mod ingest;
mod notify;
mod rebuild;
mod reconcile;
mod relay;
mod role;
mod shutdown;

use std::sync::Arc;
use std::time::Duration;

use paygate_infra::clickhouse::ClickHouseClient;
use paygate_infra::config::{
    ClickHouseConfig, CommonConfig, DbConfig, IngestConfig, KafkaConfig, NotifyConfig,
    ReconcileConfig, RelayConfig,
};
use paygate_infra::db::Db;
use paygate_kafkabus::Consumer;

use admin::{AdminState, IngestAdmin, SimpleReady};
use reconcile::ReconcileDeps;
use role::Role;

/// How long an ingester's own `GET /status` waits on Kafka's `committed` and
/// `fetch_watermarks` calls before giving up on this one request — generous
/// enough for a broker under normal load, short enough that a stuck broker
/// does not hang the admin port itself.
const INGEST_STATUS_TIMEOUT: Duration = Duration::from_secs(5);

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let role = role::parse_role(&args).unwrap_or_else(|err| fail_before_tracing(&err));

    let common = CommonConfig::load().unwrap_or_else(|err| fail_before_tracing(&err));
    paygate_infra::log::init(&common.log_level).unwrap_or_else(|err| fail_before_tracing(&err));

    tracing::info!(
        role = %role,
        instance_id = %common.instance_id,
        "paygate-worker starting"
    );

    match role {
        Role::Relay => run_relay(common).await,
        Role::Ingest => run_ingest(common).await,
        Role::Notify => run_notify(common).await,
        Role::Reconcile => run_reconcile(common).await,
        Role::RebuildReports => run_rebuild(common).await,
    }
}

/// Before `LOG_LEVEL` is even known, a bad argument or a bad `CommonConfig`
/// still has to fail loudly and non-zero, naming what was wrong — plain
/// JSON on stderr, the same shape `demo-merchant`'s own `main.rs` uses for
/// the same reason.
fn fail_before_tracing(err: &impl std::fmt::Display) -> ! {
    eprintln!("{{\"level\":\"error\",\"message\":\"{err}\"}}");
    std::process::exit(1);
}

/// Every failure after tracing is installed goes through `tracing::error!`
/// instead, so it is JSON-structured like every other log line this binary
/// writes.
fn fail(err: impl std::fmt::Display) -> ! {
    tracing::error!(error = %err, "paygate-worker cannot start");
    std::process::exit(1);
}

fn build_http_client() -> reqwest::Client {
    // Redirects disabled everywhere this binary calls out, the same rule
    // `spec.md`'s "Telling the merchant" states for the notifier ("the
    // notifier does not follow redirects"); the reconciler's own provider
    // calls follow it for the same reason — a callback or a query answer is
    // a message to a known address, not a link to be chased.
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap_or_else(|err| fail(err))
}

/// A comma-free, single milliseconds value read directly in `main.rs` rather
/// than through `paygate_infra::config`, since that crate is not this one's
/// to edit — see `reconcile::ReconcileDeps::http_timeout`'s own doc comment
/// for why it exists at all.
fn optional_duration_ms(name: &str, default_ms: u64) -> Duration {
    match std::env::var(name) {
        Ok(v) if !v.is_empty() => match v.parse::<u64>() {
            Ok(ms) => Duration::from_millis(ms),
            Err(err) => fail(format!("invalid configuration for {name}: {err}")),
        },
        _ => Duration::from_millis(default_ms),
    }
}

/// Waits for the admin server to finish its own graceful shutdown, bounded by
/// `SHUTDOWN_GRACE_SECONDS` — `axum::serve`'s graceful shutdown has no
/// deadline of its own, so this is what makes `spec.md`'s NFR-OPS-1,
/// "Graceful shutdown", true within the `SHUTDOWN_GRACE_SECONDS` its
/// "Environment variables" names, rather than aspirational.
async fn wait_for_admin(handle: tokio::task::JoinHandle<()>, grace: Duration) {
    if tokio::time::timeout(grace, handle).await.is_err() {
        tracing::warn!("admin server did not shut down within the grace period");
    }
}

async fn run_relay(common: CommonConfig) {
    let db_cfg = DbConfig::load().unwrap_or_else(|err| fail(err));
    let kafka_cfg = KafkaConfig::load().unwrap_or_else(|err| fail(err));
    let relay_cfg = RelayConfig::load().unwrap_or_else(|err| fail(err));

    let db = Db::connect(&db_cfg.dsn, db_cfg.pool_max)
        .await
        .unwrap_or_else(|err| fail(err));

    let mut shutdown_rx = shutdown::signal();
    let ready = Arc::new(SimpleReady::new());
    let server = admin::serve(
        common.port,
        AdminState::Relay(ready.clone()),
        shutdown_rx.clone(),
    )
    .await
    .unwrap_or_else(|err| fail(err));

    if let Some(producer) = relay::ensure_topic_and_producer(&kafka_cfg, &mut shutdown_rx).await {
        ready.mark_ready();
        relay::poll_loop(
            db,
            Arc::new(producer),
            relay_cfg.batch_size,
            relay_cfg.poll_interval,
            shutdown_rx.clone(),
        )
        .await;
    }

    wait_for_admin(server, common.shutdown_grace).await;
}

async fn run_ingest(common: CommonConfig) {
    let kafka_cfg = KafkaConfig::load().unwrap_or_else(|err| fail(err));
    let ingest_cfg = IngestConfig::load().unwrap_or_else(|err| fail(err));
    let ch_cfg = ClickHouseConfig::load().unwrap_or_else(|err| fail(err));

    let consumer = Consumer::new(&kafka_cfg.brokers, &kafka_cfg.group_id, &kafka_cfg.topic)
        .unwrap_or_else(|err| fail(err));
    let consumer = Arc::new(consumer);
    let ch = ClickHouseClient::new(
        &ch_cfg.url,
        &ch_cfg.database,
        &ch_cfg.user,
        &ch_cfg.password,
    );

    let shutdown_rx = shutdown::signal();
    let admin_state = AdminState::Ingest(Arc::new(IngestAdmin {
        consumer: consumer.clone(),
        status_timeout: INGEST_STATUS_TIMEOUT,
        instance_id: common.instance_id.clone(),
    }));
    let server = admin::serve(common.port, admin_state, shutdown_rx.clone())
        .await
        .unwrap_or_else(|err| fail(err));

    let loop_cfg = ingest::IngestLoopConfig {
        batch_max: ingest_cfg.batch_max as usize,
        batch_wait: ingest_cfg.batch_wait,
        instance_id: common.instance_id.clone(),
    };
    ingest::run(consumer, ch, loop_cfg, shutdown_rx.clone()).await;

    wait_for_admin(server, common.shutdown_grace).await;
}

async fn run_notify(common: CommonConfig) {
    let db_cfg = DbConfig::load().unwrap_or_else(|err| fail(err));
    let notify_cfg = NotifyConfig::load().unwrap_or_else(|err| fail(err));

    let db = Db::connect(&db_cfg.dsn, db_cfg.pool_max)
        .await
        .unwrap_or_else(|err| fail(err));
    let http = build_http_client();

    let shutdown_rx = shutdown::signal();
    let ready = Arc::new(SimpleReady::new());
    let server = admin::serve(
        common.port,
        AdminState::Notify(ready.clone()),
        shutdown_rx.clone(),
    )
    .await
    .unwrap_or_else(|err| fail(err));

    // The database is connected, so this role can do its job; nothing else
    // this role depends on has a startup phase of its own.
    ready.mark_ready();
    notify::poll_loop(db, http, notify_cfg, shutdown_rx.clone()).await;

    wait_for_admin(server, common.shutdown_grace).await;
}

async fn run_reconcile(common: CommonConfig) {
    let db_cfg = DbConfig::load().unwrap_or_else(|err| fail(err));
    let reconcile_cfg = ReconcileConfig::load().unwrap_or_else(|err| fail(err));
    let http_timeout = optional_duration_ms("RECONCILE_HTTP_TIMEOUT_MS", 3000);

    let db = Db::connect(&db_cfg.dsn, db_cfg.pool_max)
        .await
        .unwrap_or_else(|err| fail(err));
    let http = build_http_client();

    let deps = ReconcileDeps {
        db,
        http,
        provider_base_url: reconcile_cfg.provider_base_url.clone(),
        after: reconcile_cfg.after,
        retry: reconcile_cfg.retry,
        batch_size: reconcile_cfg.batch_size as i64,
        http_timeout,
    };

    let shutdown_rx = shutdown::signal();
    let ready = Arc::new(SimpleReady::new());
    ready.mark_ready();
    let server = admin::serve(
        common.port,
        AdminState::Reconcile(ready, deps.clone()),
        shutdown_rx.clone(),
    )
    .await
    .unwrap_or_else(|err| fail(err));

    if reconcile_cfg.poll_interval_ms == 0 {
        // "do not poll" (`spec.md`, "Environment variables"): serve
        // `POST /run` and nothing else.
        let mut rx = shutdown_rx.clone();
        let _ = rx.wait_for(|fired| *fired).await;
    } else {
        reconcile::poll_loop(
            deps,
            Duration::from_millis(reconcile_cfg.poll_interval_ms),
            shutdown_rx.clone(),
        )
        .await;
    }

    wait_for_admin(server, common.shutdown_grace).await;
}

async fn run_rebuild(common: CommonConfig) {
    let db_cfg = DbConfig::load().unwrap_or_else(|err| fail(err));
    let ch_cfg = ClickHouseConfig::load().unwrap_or_else(|err| fail(err));
    // Reuses the ingester's own batch size rather than inventing a new
    // environment variable for a one-shot command the suites run with none
    // of the ingester's variables set (`INGEST_BATCH_MAX` defaults to 500
    // either way) — reported as a chosen default in the final report.
    let ingest_cfg = IngestConfig::load().unwrap_or_else(|err| fail(err));

    let db = Db::connect(&db_cfg.dsn, db_cfg.pool_max)
        .await
        .unwrap_or_else(|err| fail(err));
    let ch = ClickHouseClient::new(
        &ch_cfg.url,
        &ch_cfg.database,
        &ch_cfg.user,
        &ch_cfg.password,
    );

    if let Err(err) = rebuild::run(&db, &ch, ingest_cfg.batch_max as i64, &common.instance_id).await
    {
        fail(err);
    }
}
