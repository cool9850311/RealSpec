//! Shared test fixtures: a `Config` that never touches a real network, and a
//! tiny fake paygate — just enough of `POST /api/v1/payments` and
//! `GET /api/v1/payments/{id}` for this crate's own handlers to be tested
//! against, without a real `api` binary running anywhere.

#![cfg(test)]

use axum::extract::Path;
use axum::routing::{get, post};
use axum::{Json, Router};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::config::Config;

pub fn config_with_gateway(gateway_url: String) -> Config {
    Config {
        port: 0,
        instance_id: "test".to_string(),
        log_level: "error".to_string(),
        shutdown_grace: Duration::from_secs(1),
        gateway_url,
        api_key: "sk_test_fixture".to_string(),
        merchant_hash_key: "acmehashkey0123456789abcdef01234".to_string(),
        merchant_hash_iv: "acmehashiv012345".to_string(),
        notify_timeout: Duration::from_millis(20),
    }
}

/// A gateway that refuses every call — stands in for "could not be reached
/// or refused the order" (`spec/openapi/demo-merchant.yaml`, `POST /orders`).
pub async fn spawn_failing_gateway() -> String {
    let app = Router::new().fallback(|| async { axum::http::StatusCode::INTERNAL_SERVER_ERROR });
    spawn(app).await
}

/// A gateway that always creates an order successfully, and whose answer to
/// `GET /payments/{id}` is whatever the test last set through the returned
/// handle — so a test can ask for "an attempt failed" or "nothing yet"
/// without standing up a second server.
///
/// Every `POST /payments` — real paygate's own rule — mints a NEW provider
/// order number even for the SAME `merchant_trade_no`; this fake mirrors
/// that by numbering the `MerchantTradeNo` field it echoes back with a call
/// counter, so a test can tell two mints apart and count exactly how many
/// happened.
pub struct FakeGateway {
    pub base_url: String,
    get_response: Arc<Mutex<serde_json::Value>>,
    create_calls: Arc<AtomicUsize>,
}

impl FakeGateway {
    pub fn set_get_response(&self, value: serde_json::Value) {
        *self
            .get_response
            .lock()
            .expect("fake gateway mutex poisoned") = value;
    }

    /// How many times `POST /api/v1/payments` has been called so far.
    pub fn create_calls(&self) -> usize {
        self.create_calls.load(Ordering::SeqCst)
    }
}

pub async fn spawn_fake_gateway(initial_get_response: serde_json::Value) -> FakeGateway {
    let get_response = Arc::new(Mutex::new(initial_get_response));
    let for_get = get_response.clone();
    let create_calls = Arc::new(AtomicUsize::new(0));
    let for_create_calls = create_calls.clone();

    let app = Router::new()
        .route(
            "/api/v1/payments",
            post(move |Json(body): Json<serde_json::Value>| {
                let create_calls = for_create_calls.clone();
                async move {
                    let call_number = create_calls.fetch_add(1, Ordering::SeqCst) + 1;
                    let merchant_trade_no = body
                        .get("merchant_trade_no")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown")
                        .to_string();
                    let response = serde_json::json!({
                        "id": format!("paygate-id-{merchant_trade_no}"),
                        "status": "pending",
                        "provider": "ecpay",
                        "action": "/provider/ecpay/Cashier/AioCheckOut/V5",
                        // A distinct "provider order number" per call, the
                        // same way ECPay refuses to reuse one — this is what
                        // lets a test tell two mints apart.
                        "fields": { "MerchantTradeNo": format!("{merchant_trade_no}-SN{call_number}") },
                    });
                    (axum::http::StatusCode::CREATED, Json(response))
                }
            }),
        )
        .route(
            "/api/v1/payments/{id}",
            get(move |Path(_id): Path<String>| {
                let get_response = for_get.clone();
                async move {
                    let body = get_response
                        .lock()
                        .expect("fake gateway mutex poisoned")
                        .clone();
                    (axum::http::StatusCode::OK, Json(body))
                }
            }),
        );

    let base_url = spawn(app).await;
    FakeGateway {
        base_url,
        get_response,
        create_calls,
    }
}

async fn spawn(app: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind an ephemeral port for the fake gateway");
    let addr = listener
        .local_addr()
        .expect("local addr of the fake gateway");
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("fake gateway server");
    });
    format!("http://{addr}")
}
