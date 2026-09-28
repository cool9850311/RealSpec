//! Refunds — the one call paygate makes and waits for, so the one place a
//! provider's silence is paygate's problem (spec.md, "Refunds"). The amount
//! chooses the behaviour; `RefundTradeNo` — paygate's own id — is the
//! deduplication key that makes a retry safe, checked before the amount is
//! ever looked at.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Form, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use paygate_domain::ProviderCode;
use paygate_provider::newebpay;

use crate::state::{provider_str, AppState, RecordedRequest, RefundOutcome};
use crate::wire;

/// `777` holds the answer for 800ms, `119` always fails, `408` is answered
/// never — but is refunded internally all the same, so a retry with the same
/// `RefundTradeNo` finds it already settled (spec.md, "The payment provider
/// mock").
const DELAYED_SUCCESS_AMOUNT: i64 = 777;
const ALWAYS_FAILS_AMOUNT: i64 = 119;
const LOST_ANSWER_AMOUNT: i64 = 408;

fn success_body(provider: ProviderCode) -> String {
    match provider {
        ProviderCode::Ecpay => "1|OK".to_string(),
        ProviderCode::Newebpay => r#"{"Status":"SUCCESS","Message":"OK"}"#.to_string(),
    }
}

fn failure_body(provider: ProviderCode, message: &str) -> String {
    match provider {
        ProviderCode::Ecpay => format!("0|{message}"),
        ProviderCode::Newebpay => format!(r#"{{"Status":"FAIL","Message":"{message}"}}"#),
    }
}

/// ECPay's answer is `text/plain` (`key|message`); NewebPay's is
/// `application/json` (`payment-provider.yaml`'s `newebpayRefund` response) —
/// both content-addressable purely from `body`'s own shape everywhere else in
/// this crate, but a real client is entitled to trust the header too.
fn status_response(provider: ProviderCode, status: u16, body: String) -> Response {
    let code = StatusCode::from_u16(status).unwrap_or(StatusCode::OK);
    let content_type = match provider {
        ProviderCode::Ecpay => "text/plain; charset=utf-8",
        ProviderCode::Newebpay => "application/json",
    };
    (code, [(header::CONTENT_TYPE, content_type)], body).into_response()
}

/// The amount-driven behaviour, as a pure decision separate from the async
/// I/O (the sleep, the mutex) that carries it out — spec.md, "The payment
/// provider mock": "any amount succeeds... `777` succeeds after 800ms...
/// `119` answers 503... `408` never answers and refunds it anyway."
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RefundAction {
    AlwaysFails,
    LostButSucceeds,
    DelayedSuccess,
    ImmediateSuccess,
}

fn classify_amount(amount: Option<i64>) -> RefundAction {
    match amount {
        Some(ALWAYS_FAILS_AMOUNT) => RefundAction::AlwaysFails,
        Some(LOST_ANSWER_AMOUNT) => RefundAction::LostButSucceeds,
        Some(DELAYED_SUCCESS_AMOUNT) => RefundAction::DelayedSuccess,
        _ => RefundAction::ImmediateSuccess,
    }
}

async fn handle_refund(
    state: Arc<AppState>,
    provider: ProviderCode,
    outer_form: BTreeMap<String, String>,
) -> Response {
    let inner: Option<BTreeMap<String, String>> = match provider {
        ProviderCode::Ecpay => {
            if wire::ecpay_verify(&state.config.ecpay, &outer_form) {
                Some(outer_form.clone())
            } else {
                None
            }
        }
        ProviderCode::Newebpay => outer_form.get("PostData_").and_then(|post_data| {
            newebpay::decrypt_trade_info(
                &state.config.newebpay.hash_key,
                &state.config.newebpay.hash_iv,
                post_data,
            )
            .ok()
        }),
    };

    let provider_trade_no = inner
        .as_ref()
        .and_then(|f| f.get("MerchantTradeNo").cloned())
        .unwrap_or_default();
    let refund_trade_no = inner.as_ref().and_then(|f| f.get("RefundTradeNo").cloned());
    let amount = inner
        .as_ref()
        .and_then(|f| f.get("TotalAmount"))
        .and_then(|s| s.parse::<i64>().ok());

    let Some(refund_trade_no) = refund_trade_no.filter(|_| inner.is_some()) else {
        state
            .refund_requests
            .lock()
            .expect("refund_requests lock")
            .push(RecordedRequest {
                provider: provider_str(provider),
                provider_trade_no,
                amount,
                refund_trade_no: None,
                signature_verified: false,
                deduplicated: None,
            });
        return status_response(provider, 400, failure_body(provider, "CheckMacValue Error"));
    };

    let existing = state
        .refund_settled
        .lock()
        .expect("refund_settled lock")
        .get(&refund_trade_no)
        .cloned();

    if let Some(RefundOutcome { status, body }) = existing {
        state
            .refund_requests
            .lock()
            .expect("refund_requests lock")
            .push(RecordedRequest {
                provider: provider_str(provider),
                provider_trade_no,
                amount,
                refund_trade_no: Some(refund_trade_no),
                signature_verified: true,
                deduplicated: Some(true),
            });
        return status_response(provider, status, body);
    }

    state
        .refund_requests
        .lock()
        .expect("refund_requests lock")
        .push(RecordedRequest {
            provider: provider_str(provider),
            provider_trade_no,
            amount,
            refund_trade_no: Some(refund_trade_no.clone()),
            signature_verified: true,
            deduplicated: Some(false),
        });

    match classify_amount(amount) {
        RefundAction::AlwaysFails => {
            status_response(provider, 503, failure_body(provider, "Service Unavailable"))
        }
        RefundAction::LostButSucceeds => {
            let body = success_body(provider);
            state
                .refund_settled
                .lock()
                .expect("refund_settled lock")
                .insert(refund_trade_no, RefundOutcome { status: 200, body });
            // The lost-answer case: refunded for real, but no answer ever
            // comes back on THIS call. A retry (a fresh call, above) finds it
            // already settled and answers immediately.
            tokio::time::sleep(Duration::from_secs(6 * 3600)).await;
            status_response(provider, 200, success_body(provider))
        }
        RefundAction::DelayedSuccess => {
            tokio::time::sleep(Duration::from_millis(800)).await;
            let body = success_body(provider);
            state
                .refund_settled
                .lock()
                .expect("refund_settled lock")
                .insert(
                    refund_trade_no,
                    RefundOutcome {
                        status: 200,
                        body: body.clone(),
                    },
                );
            status_response(provider, 200, body)
        }
        RefundAction::ImmediateSuccess => {
            let body = success_body(provider);
            state
                .refund_settled
                .lock()
                .expect("refund_settled lock")
                .insert(
                    refund_trade_no,
                    RefundOutcome {
                        status: 200,
                        body: body.clone(),
                    },
                );
            status_response(provider, 200, body)
        }
    }
}

pub async fn ecpay_refund(
    State(state): State<Arc<AppState>>,
    Form(form): Form<BTreeMap<String, String>>,
) -> Response {
    handle_refund(state, ProviderCode::Ecpay, form).await
}

pub async fn newebpay_refund(
    State(state): State<Arc<AppState>>,
    Form(form): Form<BTreeMap<String, String>>,
) -> Response {
    handle_refund(state, ProviderCode::Newebpay, form).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_refund_amount_from_the_test_plan_classifies_correctly() {
        assert_eq!(classify_amount(Some(119)), RefundAction::AlwaysFails);
        assert_eq!(classify_amount(Some(408)), RefundAction::LostButSucceeds);
        assert_eq!(classify_amount(Some(777)), RefundAction::DelayedSuccess);
        assert_eq!(classify_amount(Some(2500)), RefundAction::ImmediateSuccess);
        assert_eq!(classify_amount(Some(1)), RefundAction::ImmediateSuccess);
        assert_eq!(classify_amount(None), RefundAction::ImmediateSuccess);
    }

    #[test]
    fn success_and_failure_bodies_match_each_providers_own_shape() {
        assert_eq!(success_body(ProviderCode::Ecpay), "1|OK");
        assert!(success_body(ProviderCode::Newebpay).contains("\"Status\":\"SUCCESS\""));
        assert!(failure_body(ProviderCode::Ecpay, "x").starts_with("0|"));
        assert!(failure_body(ProviderCode::Newebpay, "x").contains("\"Status\":\"FAIL\""));
    }

    fn test_config() -> crate::config::Config {
        crate::config::Config {
            port: 0,
            callback_urls: vec!["http://replica".to_string()],
            ecpay: paygate_provider::PlatformCredentials {
                platform_id: "3002607".to_string(),
                hash_key: "pwFHCqoQZGmho4w6".to_string(),
                hash_iv: "EkRm7iFT261dpevs".to_string(),
            },
            newebpay: paygate_provider::PlatformCredentials {
                platform_id: "MS12345678".to_string(),
                hash_key: "Fs5cX1TGqYZ3kLmN8pQrS2tUvW4xY6zA".to_string(),
                hash_iv: "B7cD9eF1gH3iJ5kL".to_string(),
            },
        }
    }

    fn signed_ecpay_refund_form(amount: i64, refund_trade_no: &str) -> BTreeMap<String, String> {
        let mut form = BTreeMap::new();
        form.insert("MerchantID".to_string(), "2000132".to_string());
        form.insert(
            "MerchantTradeNo".to_string(),
            "ACME00000001SNtest".to_string(),
        );
        form.insert("TradeNo".to_string(), "2109210000000".to_string());
        form.insert("Action".to_string(), "R".to_string());
        form.insert("TotalAmount".to_string(), amount.to_string());
        form.insert("RefundTradeNo".to_string(), refund_trade_no.to_string());
        let creds = test_config().ecpay;
        crate::wire::ecpay_sign(&creds, &mut form);
        form
    }

    /// Pins the whole point of the lost-answer case (spec.md, "Refunds": "a
    /// refund whose answer was lost is safe to send again... the provider
    /// deduplicates a retry"): a retry that reaches `handle_refund` again
    /// while the first call is still asleep, carrying the SAME
    /// `RefundTradeNo`, is answered immediately from `refund_settled` rather
    /// than waiting behind the first call's six-hour sleep. The insert into
    /// `refund_settled` happens before that sleep specifically so this holds.
    #[tokio::test]
    async fn a_retry_with_the_same_refund_trade_no_is_answered_immediately() {
        let state = Arc::new(AppState::new(test_config()));
        let form = signed_ecpay_refund_form(LOST_ANSWER_AMOUNT, "refund-1");

        let state2 = state.clone();
        let form2 = form.clone();
        let first =
            tokio::spawn(async move { handle_refund(state2, ProviderCode::Ecpay, form2).await });

        // Let the first call run its synchronous prefix (including the
        // insert into `refund_settled`) up to its first real await point
        // (the six-hour sleep) without waiting on it.
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }

        let second = handle_refund(state.clone(), ProviderCode::Ecpay, form.clone()).await;
        assert_eq!(
            second.status(),
            StatusCode::OK,
            "a retry with the same RefundTradeNo must be deduplicated, not re-processed"
        );

        first.abort();
    }

    /// The same guarantee, over a real HTTP round trip with a client-side
    /// timeout shaped like paygate's own `PSP_TIMEOUT_MS` — the first call
    /// times out from the caller's side while the mock is still "asleep",
    /// and the retry (same `RefundTradeNo`, a fresh connection) still comes
    /// back `1|OK` well within the timeout.
    #[tokio::test]
    async fn a_retry_over_real_http_is_answered_within_the_callers_timeout() {
        use axum::routing::post;
        use std::time::Duration;

        let state = Arc::new(AppState::new(test_config()));
        let app = axum::Router::new()
            .route("/refund", post(ecpay_refund))
            .with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(1000))
            .build()
            .unwrap();
        let url = format!("http://{addr}/refund");
        let form = signed_ecpay_refund_form(LOST_ANSWER_AMOUNT, "refund-http-1");

        // The first call times out client-side, exactly as paygate's own
        // `PSP_TIMEOUT_MS` would.
        assert!(client.post(&url).form(&form).send().await.is_err());

        let second = client
            .post(&url)
            .form(&form)
            .send()
            .await
            .expect("the retry must not also time out");
        assert_eq!(second.status(), 200);
        assert_eq!(second.text().await.unwrap(), "1|OK");
    }
}
