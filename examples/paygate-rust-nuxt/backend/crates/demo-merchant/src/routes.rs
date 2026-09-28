//! The router: every path is exactly one `demo-merchant.yaml` declares,
//! mounted under `/demo-merchant/api` — the reverse proxy forwards that
//! prefix without stripping it (`local/Caddyfile`), so this binary must
//! serve it too.

use axum::routing::{get, post};
use axum::Router;
use std::sync::Arc;

use crate::handlers::{deliveries, notify, orders};
use crate::state::AppState;

pub fn build_router(state: Arc<AppState>) -> Router {
    let api = Router::new()
        .route("/orders", post(orders::create_order))
        .route(
            "/orders/{merchantTradeNo}/form",
            get(orders::get_payment_form),
        )
        .route("/orders/{merchantTradeNo}", get(orders::get_order))
        .route("/notify", post(notify::notify))
        .route("/notify-strict", post(notify::notify_strict))
        .route("/notify-flaky", post(notify::notify_flaky))
        .route("/notify-loose", post(notify::notify_loose))
        .route("/notify-slow", post(notify::notify_slow))
        .route("/notify-reject", post(notify::notify_reject))
        .route("/notify-mumble", post(notify::notify_mumble))
        // The addition documented in `demo-merchant.yaml`: the merchant's own
        // delivery log, so the registry's `merchant received <n>
        // notifications at "<path>"` step has something to read.
        .route("/__deliveries", get(deliveries::get_deliveries));

    Router::new()
        .nest("/demo-merchant/api", api)
        .with_state(state)
}
