//! `paygate-provider-mock` — the stand-in for ECPay and NewebPay
//! (spec/openapi/payment-provider.yaml). One process, both shapes, served
//! under `/provider`. It is a real counterparty: it verifies every signature
//! paygate hands it and refuses what does not check out, so "paygate signs
//! correctly" is a cryptographic fact this process asserts rather than a
//! matching pair of bugs (`spec.md`, "The payment provider mock").

mod callbacks;
mod cards;
mod config;
mod html;
mod routes;
mod state;
mod util;
mod wire;

use std::sync::Arc;

use axum::extract::Request;
use axum::http::HeaderValue;
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use tower_http::trace::TraceLayer;
use tracing::Instrument;
use tracing_subscriber::EnvFilter;

use config::Config;
use state::AppState;

/// Stamps every request with an id, in both directions: logged in the span
/// every handler's `tracing` calls land in, and echoed back as
/// `X-Request-Id` (`spec.md`, "Non-functional requirements": NFR-OBS-1,
/// "every response carries a request id", and NFR-OBS-3, "structured logs
/// without secrets").
async fn request_id(req: Request, next: Next) -> Response {
    let id = uuid::Uuid::new_v4().to_string();
    let span = tracing::info_span!(
        "request",
        request_id = %id,
        method = %req.method(),
        path = %req.uri().path(),
    );
    async move {
        let mut res = next.run(req).await;
        if let Ok(value) = HeaderValue::from_str(&id) {
            res.headers_mut().insert("x-request-id", value);
        }
        res
    }
    .instrument(span)
    .await
}

fn init_tracing() {
    let level = std::env::var("LOG_LEVEL").unwrap_or_else(|_| "info".to_string());
    let filter = EnvFilter::try_new(&level).unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .json()
        .flatten_event(true)
        .with_env_filter(filter)
        .init();
}

fn build_router(state: Arc<AppState>) -> Router {
    let provider_routes = Router::new()
        .route(
            "/ecpay/Cashier/AioCheckOut/V5",
            post(routes::cashier::ecpay_cashier),
        )
        .route(
            "/newebpay/MPG/mpg_gateway",
            post(routes::cashier::newebpay_cashier),
        )
        .route("/ecpay/Cashier/Card", post(routes::cashier::submit_card))
        .route("/newebpay/Cashier/Card", post(routes::cashier::submit_card))
        .route(
            "/issuer/3ds/{provider_trade_no}",
            get(routes::issuer::issuer_page),
        )
        .route(
            "/issuer/3ds/{provider_trade_no}/authenticate",
            post(routes::issuer::authenticate),
        )
        .route(
            "/ecpay/CreditDetail/DoAction",
            post(routes::refund::ecpay_refund),
        )
        .route(
            "/newebpay/API/CreditCard/Close",
            post(routes::refund::newebpay_refund),
        )
        .route(
            "/ecpay/Cashier/QueryTradeInfo/V5",
            post(routes::query::ecpay_query),
        )
        // Not in payment-provider.yaml (only ECPay's query path is
        // documented there), but `providers.query_url` for NewebPay in every
        // BDD fixture names exactly this path, and `paygate-provider`'s
        // NewebPay adapter builds a query request to send somewhere — see
        // this crate's build report.
        .route(
            "/newebpay/API/QueryTradeInfo",
            post(routes::query::newebpay_query),
        )
        .route(
            "/__control/callbacks",
            get(routes::control::list_callbacks).post(routes::control::release_callbacks),
        )
        .route("/__control/query", post(routes::control::arm_query))
        .route("/__control/queries", get(routes::control::list_queries))
        .route("/__control/requests", get(routes::control::list_requests))
        .with_state(state);

    Router::new()
        .nest("/provider", provider_routes)
        .layer(middleware::from_fn(request_id))
        .layer(TraceLayer::new_for_http())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();

    let config = Config::from_env()?;
    let port = config.port;
    let state = Arc::new(AppState::new(config));

    let app = build_router(state);

    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port)).await?;
    tracing::info!(port, "paygate-provider-mock listening");
    axum::serve(listener, app).await?;
    Ok(())
}
