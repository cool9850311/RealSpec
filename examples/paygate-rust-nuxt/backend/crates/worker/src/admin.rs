//! The admin HTTP server every long-running role serves on `PORT`:
//! `GET /healthz`, `GET /readyz`, `GET /status`, and `POST /run` for the
//! reconciler alone (`build_router`, below, is the shape of it).
//!
//! `GET /healthz` answers `200` the instant the listener is bound — before a
//! database, Kafka or ClickHouse connection exists — because it is the
//! stack's own readiness probe for the container's wait strategy
//! (`backend/apitest/src/stack.rs`: `http_ready("/healthz")` on every worker
//! role), which is why it must start answering promptly. `GET /readyz` and
//! `GET /status` are the ones that know whether the role can actually do
//! its job.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};

use paygate_kafkabus::Consumer;

use crate::reconcile::{self, ReconcileDeps};

/// A boolean readiness flag flipped once from the role's own startup work
/// (`relay`: the topic exists and the producer is built; `notify` and
/// `reconcile`: the database is connected). Plain `AtomicBool` rather than a
/// richer type because every one of these roles has exactly one readiness
/// question with a yes/no answer.
#[derive(Default)]
pub struct SimpleReady(AtomicBool);

impl SimpleReady {
    pub fn new() -> Self {
        Self(AtomicBool::new(false))
    }

    pub fn mark_ready(&self) {
        self.0.store(true, Ordering::Release);
    }

    pub fn is_ready(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

/// The ingester's own readiness is never a flag — it is asked of
/// `paygate_kafkabus::Consumer::status` fresh on every request, which is what
/// keeps `GET /status` honest: it can never report `ready` or a stale `lag`
/// from before an assignment settled, because nothing here caches either.
pub struct IngestAdmin {
    pub consumer: Arc<Consumer>,
    pub status_timeout: Duration,
    pub instance_id: String,
}

pub enum AdminState {
    Relay(Arc<SimpleReady>),
    Ingest(Arc<IngestAdmin>),
    Notify(Arc<SimpleReady>),
    Reconcile(Arc<SimpleReady>, ReconcileDeps),
}

impl Clone for AdminState {
    fn clone(&self) -> Self {
        match self {
            AdminState::Relay(r) => AdminState::Relay(r.clone()),
            AdminState::Ingest(i) => AdminState::Ingest(i.clone()),
            AdminState::Notify(r) => AdminState::Notify(r.clone()),
            AdminState::Reconcile(r, deps) => AdminState::Reconcile(r.clone(), deps.clone()),
        }
    }
}

impl AdminState {
    fn role_name(&self) -> &'static str {
        match self {
            AdminState::Relay(_) => "relay",
            AdminState::Ingest(_) => "ingest",
            AdminState::Notify(_) => "notify",
            AdminState::Reconcile(_, _) => "reconcile",
        }
    }

    async fn is_ready(&self) -> bool {
        match self {
            AdminState::Relay(r) | AdminState::Notify(r) | AdminState::Reconcile(r, _) => {
                r.is_ready()
            }
            AdminState::Ingest(admin) => admin
                .consumer
                .status(admin.status_timeout)
                .await
                .map(|s| s.ready)
                .unwrap_or(false),
        }
    }

    async fn status_json(&self) -> Value {
        match self {
            AdminState::Ingest(admin) => {
                let status = admin.consumer.status(admin.status_timeout).await.unwrap_or(
                    paygate_kafkabus::ConsumerStatus {
                        ready: false,
                        lag: 0,
                    },
                );
                json!({
                    "role": self.role_name(),
                    "ready": status.ready,
                    "lag": status.lag,
                    "instance_id": admin.instance_id,
                })
            }
            other => json!({
                "role": other.role_name(),
                "ready": other.is_ready().await,
            }),
        }
    }
}

pub fn build_router(state: AdminState) -> Router {
    let is_reconcile = matches!(state, AdminState::Reconcile(_, _));
    let mut router = Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/status", get(status));
    if is_reconcile {
        router = router.route("/run", post(run_reconcile_pass));
    }
    router.with_state(state)
}

async fn healthz() -> StatusCode {
    StatusCode::OK
}

async fn readyz(State(state): State<AdminState>) -> StatusCode {
    if state.is_ready().await {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    }
}

async fn status(State(state): State<AdminState>) -> Json<Value> {
    Json(state.status_json().await)
}

async fn run_reconcile_pass(State(state): State<AdminState>) -> Result<Json<Value>, StatusCode> {
    let AdminState::Reconcile(_, deps) = state else {
        return Err(StatusCode::NOT_FOUND);
    };
    match reconcile::run_pass_and_resend(&deps).await {
        Ok(summary) => Ok(Json(json!({
            "queried": summary.queried,
            "settled": summary.settled,
            "abandoned": summary.abandoned,
            "refunds_resent": summary.refunds_resent,
            "duplicate_refunds_queued": summary.duplicate_refunds_queued,
        }))),
        Err(err) => {
            tracing::error!(error = %err, "reconcile pass failed");
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// Binds `PORT` and starts serving in the background, stopping only once
/// `shutdown` fires (`crate::shutdown`) — `axum::serve`'s own graceful
/// shutdown, which stops accepting new connections and waits for in-flight
/// ones.
pub async fn serve(
    port: u16,
    state: AdminState,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) -> std::io::Result<tokio::task::JoinHandle<()>> {
    let app = build_router(state);
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port)).await?;
    Ok(tokio::spawn(async move {
        let graceful = async move {
            let _ = shutdown.wait_for(|v| *v).await;
        };
        if let Err(err) = axum::serve(listener, app)
            .with_graceful_shutdown(graceful)
            .await
        {
            tracing::error!(error = %err, "admin server error");
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Binds an ephemeral port and serves `state` in the background for the
    /// life of the test — worker has no `tower` dependency to drive the
    /// router in-process (`Router::oneshot` needs `tower::ServiceExt`), so
    /// these tests exercise the real thing: a bound socket and real HTTP
    /// requests, exactly what the stack's own readiness probes do.
    async fn spawn(state: AdminState) -> (String, tokio::task::JoinHandle<()>) {
        let app = build_router(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        (format!("http://{addr}"), handle)
    }

    #[tokio::test]
    async fn healthz_answers_200_before_anything_is_ready() {
        let ready = Arc::new(SimpleReady::new());
        let (base, _server) = spawn(AdminState::Relay(ready)).await;
        let resp = reqwest::get(format!("{base}/healthz")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn readyz_is_503_until_marked_ready_then_200() {
        let ready = Arc::new(SimpleReady::new());
        let (base, _server) = spawn(AdminState::Relay(ready.clone())).await;

        let resp = reqwest::get(format!("{base}/readyz")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);

        ready.mark_ready();
        let resp = reqwest::get(format!("{base}/readyz")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn relay_status_shape() {
        let ready = Arc::new(SimpleReady::new());
        ready.mark_ready();
        let (base, _server) = spawn(AdminState::Relay(ready)).await;

        let resp = reqwest::get(format!("{base}/status")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body: Value = resp.json().await.unwrap();
        assert_eq!(body["role"], "relay");
        assert_eq!(body["ready"], true);
        assert!(body.get("lag").is_none());
    }

    #[tokio::test]
    async fn notify_status_shape() {
        let ready = Arc::new(SimpleReady::new());
        let (base, _server) = spawn(AdminState::Notify(ready)).await;

        let resp = reqwest::get(format!("{base}/status")).await.unwrap();
        let body: Value = resp.json().await.unwrap();
        assert_eq!(body["role"], "notify");
        assert_eq!(body["ready"], false);
    }

    #[tokio::test]
    async fn run_is_not_registered_for_non_reconcile_roles() {
        let ready = Arc::new(SimpleReady::new());
        let (base, _server) = spawn(AdminState::Notify(ready)).await;

        let client = reqwest::Client::new();
        let resp = client.post(format!("{base}/run")).send().await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
