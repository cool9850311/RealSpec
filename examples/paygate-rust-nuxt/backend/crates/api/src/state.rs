//! Shared, per-replica state. `paygate-api` is stateless across requests
//! (spec.md, "Scaling": "api is stateless; any replica serves any request"):
//! every field here is a handle to something external (PostgreSQL, Redis,
//! ClickHouse, the provider over HTTP) or a piece of configuration loaded
//! once at startup, never anything a request would need the SAME replica to
//! see again.

use std::sync::Arc;
use std::time::Duration;

use paygate_infra::clickhouse::ClickHouseClient;
use paygate_infra::config::{ApiConfig, CommonConfig};
use paygate_infra::redis_store::RedisStore;
use paygate_infra::Db;

#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    pub redis: Arc<RedisStore>,
    pub clickhouse: Arc<ClickHouseClient>,
    /// Used for exactly one thing this crate does over the network itself:
    /// calling a provider's refund endpoint (`spec.md`, "Refunds": "the one
    /// call paygate makes to a provider and waits for"). `redirect(Policy::none())`
    /// for the same reason the notifier's client refuses them (`spec.md`,
    /// "Telling the merchant": "the notifier does not follow redirects" — "a
    /// callback is a message to a known address, not a link to be chased"):
    /// a POST that gets redirected is a POST to somewhere nobody signed for.
    pub http: reqwest::Client,
    pub config: Arc<ApiConfig>,
    pub common: Arc<CommonConfig>,
    /// `PROVIDER_BASE_URL` — `spec.md`'s "Environment variables" names this
    /// as used by "api, reconciler", but `paygate_infra::config::ApiConfig`
    /// (unlike `ReconcileConfig`) does not read it. Reported as a gap in this
    /// agent's final report; read directly here, with the same "fail loudly,
    /// name the variable" startup discipline every other required variable
    /// gets (`crate::extra_config`).
    pub provider_base_url: String,
}

/// Every Redis call this crate makes directly is bounded by this. The
/// circuit breaker (`paygate_infra::redis_store::RedisStore`) stops
/// RETRYING once it has decided Redis is down, but a single call made
/// while that decision is still being reached — the connection manager
/// reconnecting, a command queued behind one that never got an answer — has
/// no bound of its own, and `resilience.feature`'s "Without Redis, four
/// refunds racing with one key still produce one refund" caught exactly
/// that: a handler that never gets `StoreUnavailable` back can never run the
/// degradation it already knows how to do (fall back to PostgreSQL, fail a
/// rate limit open, treat a session as absent) — it just never answers at
/// all. `routes::health::ready` already bounds its own probe this way (with
/// a much tighter, poll-appropriate 200ms — a false "down" there just costs
/// one extra poll); this is the same idea for every other Redis call the
/// crate makes, where a false negative fails a real request instead. Three
/// seconds is deliberately generous rather than tight: measured against a
/// real, restarted Redis, `redis::aio::ConnectionManager` can take a little
/// over a second to reconnect (`resilience.feature`'s own "Redis coming back
/// empty" scenarios issue a request the instant Redis is reported healthy
/// again), so this only has to be shorter than a request's own patience —
/// the apitest suite's own HTTP client waits 30s — not as tight as a
/// liveness probe.
pub const REDIS_OP_TIMEOUT: Duration = Duration::from_millis(3_000);

/// Run a Redis call bounded by [`REDIS_OP_TIMEOUT`], collapsing a timeout
/// into the same `StoreUnavailable` its caller already has a declared
/// degradation for — so nothing downstream has to know or care whether
/// Redis answered "no" or never answered at all.
pub async fn redis_bounded<T>(
    fut: impl std::future::Future<Output = paygate_infra::redis_store::StoreResult<T>>,
) -> paygate_infra::redis_store::StoreResult<T> {
    tokio::time::timeout(REDIS_OP_TIMEOUT, fut)
        .await
        .unwrap_or(Err(paygate_infra::redis_store::StoreUnavailable))
}

impl AppState {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        db: Db,
        redis: RedisStore,
        clickhouse: ClickHouseClient,
        config: ApiConfig,
        common: CommonConfig,
        provider_base_url: String,
    ) -> Self {
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            db,
            redis: Arc::new(redis),
            clickhouse: Arc::new(clickhouse),
            http,
            config: Arc::new(config),
            common: Arc::new(common),
            provider_base_url,
        }
    }
}
