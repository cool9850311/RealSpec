//! `GET /health/live` and `GET /health/ready` — no credential of any kind
//! (`openapi.yaml`: "carries no security scheme"), and answers exactly the
//! two documented shapes, nothing more (`observability.feature`: "a build
//! version, a hostname or a connection string here would hand a scanner...
//! exactly the fingerprint an orchestrator never needed").

use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde_json::json;

use crate::state::AppState;

const READY_PROBE_TIMEOUT: Duration = Duration::from_millis(200);

pub async fn live() -> impl IntoResponse {
    Json(json!({ "status": "live" }))
}

pub async fn ready(State(state): State<AppState>) -> impl IntoResponse {
    let postgres_up = tokio::time::timeout(READY_PROBE_TIMEOUT, state.db.ping())
        .await
        .unwrap_or(false);
    let redis_up = tokio::time::timeout(READY_PROBE_TIMEOUT, state.redis.ping())
        .await
        .unwrap_or(false);

    let status = if postgres_up { "ready" } else { "unready" };
    let http_status = if postgres_up {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    let body = json!({
        "status": status,
        "checks": {
            "postgres": if postgres_up { "up" } else { "down" },
            "redis": if redis_up { "up" } else { "down" },
        }
    });
    (http_status, Json(body))
}
