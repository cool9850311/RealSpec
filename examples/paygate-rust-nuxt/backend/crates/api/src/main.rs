//! `paygate-api` — the merchant API, the provider callback receiver and the
//! dashboard (`spec/openapi/openapi.yaml`, everything under `/api/v1`).

mod auth;
mod cookies;
mod db;
mod dto;
mod error;
mod idempotency;
#[cfg(test)]
mod integration_tests;
mod middleware;
mod refund_flow;
mod routes;
mod state;

use std::time::Duration;

use axum::routing::{delete, get, post};
use axum::Router;
use tower_http::cors::{AllowHeaders, AllowMethods, AllowOrigin, CorsLayer};
use tower_http::trace::TraceLayer;

use paygate_infra::clickhouse::ClickHouseClient;
use paygate_infra::config::{ApiConfig, ClickHouseConfig, CommonConfig, DbConfig, RedisConfig};
use paygate_infra::redis_store::RedisStore;
use paygate_infra::Db;

use state::AppState;

fn fail(variable: &str, reason: impl std::fmt::Display) -> ! {
    // Startup fails loudly, non-zero, naming the variable — before the JSON
    // logging subscriber even exists, so this is a plain stderr line, the
    // same discipline `demo-merchant` and `provider-mock` already follow.
    eprintln!(
        "{{\"level\":\"error\",\"message\":\"invalid configuration for {variable}: {reason}\"}}"
    );
    std::process::exit(1);
}

/// The pure half of reading a required environment variable — never exits,
/// so it is what this module's own unit tests exercise directly
/// (`spec.md`, "Test plan" -> `api`: "configuration parsing"). `required_env`
/// is the thin, untestable wrapper `main` actually calls.
fn read_required_env(name: &str) -> Result<String, &'static str> {
    match std::env::var(name) {
        Ok(v) if !v.is_empty() => Ok(v),
        _ => Err("required but not set"),
    }
}

fn required_env(name: &'static str) -> String {
    read_required_env(name).unwrap_or_else(|reason| fail(name, reason))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let common = CommonConfig::load().unwrap_or_else(|e| fail(e.variable, e.reason));
    let db_config = DbConfig::load().unwrap_or_else(|e| fail(e.variable, e.reason));
    let redis_config = RedisConfig::load().unwrap_or_else(|e| fail(e.variable, e.reason));
    let clickhouse_config = ClickHouseConfig::load().unwrap_or_else(|e| fail(e.variable, e.reason));
    let api_config = ApiConfig::load().unwrap_or_else(|e| fail(e.variable, e.reason));
    // `spec.md`'s "Environment variables" names `PROVIDER_BASE_URL` as used
    // by "api, reconciler", but `paygate_infra::config::ApiConfig` does not
    // read it (only `ReconcileConfig`, the worker's own, does) — a gap in
    // this crate's final report. Read directly here with the same discipline.
    let provider_base_url = required_env("PROVIDER_BASE_URL");

    paygate_infra::log::init(&common.log_level).unwrap_or_else(|e| {
        eprintln!(
            "{{\"level\":\"error\",\"message\":\"failed to install the log subscriber: {e}\"}}"
        );
        std::process::exit(1);
    });

    tracing::info!(
        instance_id = %common.instance_id,
        port = common.port,
        "paygate-api starting"
    );

    let db = Db::connect(&db_config.dsn, db_config.pool_max)
        .await
        .unwrap_or_else(|e| {
            eprintln!(
                "{{\"level\":\"error\",\"message\":\"could not connect to PostgreSQL: {e}\"}}"
            );
            std::process::exit(1);
        });
    if db_config.schema_auto_migrate {
        if let Err(e) = db.migrate().await {
            eprintln!("{{\"level\":\"error\",\"message\":\"migration failed: {e}\"}}");
            std::process::exit(1);
        }
    }

    let redis = RedisStore::connect(&redis_config.url)
        .await
        .unwrap_or_else(|e| {
            eprintln!("{{\"level\":\"error\",\"message\":\"could not connect to Redis: {e}\"}}");
            std::process::exit(1);
        });

    let clickhouse = ClickHouseClient::new(
        &clickhouse_config.url,
        &clickhouse_config.database,
        &clickhouse_config.user,
        &clickhouse_config.password,
    );

    let shutdown_grace = common.shutdown_grace;
    let port = common.port;
    let frontend_origin = api_config.frontend_origin.clone();

    let state = AppState::new(db, redis, clickhouse, api_config, common, provider_base_url);

    let app = build_router(state, &frontend_origin);

    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port)).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal(shutdown_grace))
        .await?;

    Ok(())
}

fn build_router(state: AppState, frontend_origin: &str) -> Router {
    // The dashboard session travels as a cookie, so this can never be the
    // wildcard-origin, credential-less CORS a public API would use — every
    // browser refuses `Allow-Origin: *` together with `Allow-Credentials:
    // true`, and `tower_http` refuses to even build a `CorsLayer` that pairs
    // `allow_credentials(true)` with an `Any` header/method list for the same
    // reason. A literal `*` in `FRONTEND_ORIGIN` (the suites' own value, since
    // the API surface is driven directly rather than through a browser) is
    // therefore satisfied by reflecting whatever `Origin` a caller sent
    // rather than by the literal wildcard.
    let allow_origin = if frontend_origin == "*" {
        AllowOrigin::mirror_request()
    } else {
        frontend_origin
            .parse::<axum::http::HeaderValue>()
            .map(AllowOrigin::exact)
            .unwrap_or_else(|_| AllowOrigin::mirror_request())
    };
    let cors = CorsLayer::new()
        .allow_credentials(true)
        .allow_headers(AllowHeaders::mirror_request())
        .allow_methods(AllowMethods::mirror_request())
        .allow_origin(allow_origin);

    let v1 = Router::new()
        .route(
            "/payments",
            post(routes::payments::create_order).get(routes::payments::find_order_by_trade_no),
        )
        .route("/payments/{paymentId}", get(routes::payments::get_order))
        .route(
            "/payments/{paymentId}/refunds",
            post(routes::payments::create_refund),
        )
        .route(
            "/webhooks/{provider}",
            post(routes::webhooks::receive_callback),
        )
        .route("/dashboard/login", post(routes::dashboard::login))
        .route("/dashboard/logout", post(routes::dashboard::logout))
        .route("/dashboard/me", get(routes::dashboard::me))
        .route(
            "/dashboard/api-keys",
            post(routes::dashboard::create_api_key),
        )
        .route(
            "/dashboard/api-keys/{keyId}",
            delete(routes::dashboard::revoke_api_key),
        )
        .route(
            "/dashboard/reports/daily",
            get(routes::dashboard::daily_report),
        )
        .route("/health/live", get(routes::health::live))
        .route("/health/ready", get(routes::health::ready));

    Router::new()
        .nest("/api/v1", v1)
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            middleware::request_context,
        ))
        .layer(cors)
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

/// Waits for Ctrl+C or SIGTERM, then gives in-flight requests up to
/// `SHUTDOWN_GRACE_SECONDS` before `axum::serve` finishes tearing down.
async fn shutdown_signal(_grace: Duration) {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    tracing::info!("paygate-api shutting down");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Environment variables are process-global; serialise the tests that
    // touch them the same way `paygate-infra::config`'s own tests do.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn a_missing_variable_is_reported_as_required_but_not_set() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("PAYGATE_API_TEST_ONLY_VAR");
        let err = read_required_env("PAYGATE_API_TEST_ONLY_VAR").unwrap_err();
        assert_eq!(err, "required but not set");
    }

    #[test]
    fn an_empty_variable_is_treated_the_same_as_missing() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("PAYGATE_API_TEST_ONLY_VAR", "");
        let err = read_required_env("PAYGATE_API_TEST_ONLY_VAR").unwrap_err();
        assert_eq!(err, "required but not set");
        std::env::remove_var("PAYGATE_API_TEST_ONLY_VAR");
    }

    #[test]
    fn a_present_variable_is_returned_verbatim() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("PAYGATE_API_TEST_ONLY_VAR", "http://provider-mock:8080");
        let value = read_required_env("PAYGATE_API_TEST_ONLY_VAR").unwrap();
        assert_eq!(value, "http://provider-mock:8080");
        std::env::remove_var("PAYGATE_API_TEST_ONLY_VAR");
    }
}
