//! The `/__control` surface. Not part of any real provider — the mechanism
//! `spec/bdd/api/*.feature` uses to release a queued callback without a
//! browser, arm the next `QueryTradeInfo` answer, and read back what the
//! mock actually saw (`payment-provider.yaml`, `Control` tag).

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use crate::callbacks::{self, Variant};
use crate::state::{AppState, ArmedQuery};

pub async fn list_callbacks(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let pending = {
        let order = state.pending_order.lock().expect("pending_order lock");
        let attempts = state.attempts.lock().expect("attempts lock");
        order
            .iter()
            .filter_map(|trade_no| attempts.get(trade_no))
            .map(|attempt| callbacks::record_for(attempt, Variant::Honest))
            .collect::<Vec<_>>()
    };
    let delivered = state
        .delivered_log
        .lock()
        .expect("delivered_log lock")
        .clone();
    Json(json!({ "pending": pending, "delivered": delivered }))
}

#[derive(Debug, Deserialize)]
pub struct ReleaseRequest {
    times: u32,
    #[serde(rename = "as", default)]
    r#as: Option<String>,
    rtn_code: Option<String>,
}

pub async fn release_callbacks(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ReleaseRequest>,
) -> Response {
    let anything_pending = !state
        .pending_order
        .lock()
        .expect("pending_order lock")
        .is_empty();
    if !anything_pending {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "NO_PENDING_CALLBACKS", "message": "there is nothing queued to release" })),
        )
            .into_response();
    }

    let variant = match req.r#as.as_deref() {
        None | Some("queued") => Variant::Honest,
        Some("forged") => Variant::Forged,
        Some("simulated") => Variant::Simulated,
        Some("wrong_amount") => Variant::WrongAmount,
        Some("return_code") => {
            let code = req
                .rtn_code
                .as_deref()
                .and_then(|s| s.parse::<i32>().ok())
                .unwrap_or(0);
            Variant::ReturnCode(code)
        }
        Some(_) => Variant::Honest,
    };

    let deliveries = callbacks::release(&state, None, req.times.max(1), variant).await;
    Json(json!({ "deliveries": deliveries })).into_response()
}

#[derive(Debug, Deserialize, Default)]
pub struct ArmQueryRequest {
    trade_status: Option<String>,
    #[serde(rename = "as", default)]
    r#as: Option<String>,
}

pub async fn arm_query(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ArmQueryRequest>,
) -> StatusCode {
    let armed = match req.r#as.as_deref() {
        Some("forged") => ArmedQuery::Forged,
        Some("throttled") => ArmedQuery::Throttled,
        _ => ArmedQuery::TradeStatus(req.trade_status.unwrap_or_else(|| "0".to_string())),
    };
    *state.armed_query.lock().expect("armed_query lock") = Some(armed);
    StatusCode::OK
}

pub async fn list_queries(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let queries = state.query_log.lock().expect("query_log lock").clone();
    Json(json!({ "queries": queries }))
}

pub async fn list_requests(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let charges = state
        .charge_requests
        .lock()
        .expect("charge_requests lock")
        .clone();
    let refunds = state
        .refund_requests
        .lock()
        .expect("refund_requests lock")
        .clone();
    Json(json!({ "charges": charges, "refunds": refunds }))
}
