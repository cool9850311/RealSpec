//! The seven callback receivers `demo-merchant.yaml` and `notify.feature`
//! pin by name. Each one states the merchant's behaviour as data — always
//! `1|OK`, verify first, fail twice, never accept, the wrong body, the right
//! body with a newline, the right body too late — and every one of them
//! records the delivery (`GET /__deliveries`), whatever it goes on to answer.

use axum::extract::{Form, State};
use axum::http::StatusCode;
use std::sync::Arc;

use crate::dto::NotificationForm;
use crate::signature::verify_check_mac_value;
use crate::state::{record_delivery, AppState, OrderStatus};

const PATH_NOTIFY: &str = "/demo-merchant/api/notify";
const PATH_NOTIFY_STRICT: &str = "/demo-merchant/api/notify-strict";
const PATH_NOTIFY_FLAKY: &str = "/demo-merchant/api/notify-flaky";
const PATH_NOTIFY_LOOSE: &str = "/demo-merchant/api/notify-loose";
const PATH_NOTIFY_SLOW: &str = "/demo-merchant/api/notify-slow";
const PATH_NOTIFY_REJECT: &str = "/demo-merchant/api/notify-reject";
const PATH_NOTIFY_MUMBLE: &str = "/demo-merchant/api/notify-mumble";

/// Applies what a genuinely-accepted notification says to this merchant's
/// own record of the order, if it has one. Delivery is at-least-once and
/// deduplicated on `MerchantTradeNo`, which the merchant already has
/// (`spec.md`, "Telling the merchant"), so this is written to be idempotent:
/// re-applying `paid` to an already-`paid` order changes nothing. A
/// `MerchantTradeNo` this merchant never created through its own `POST
/// /orders` (every direct `POST /api/v1/payments` call `notify.feature`
/// makes) is simply not in the map, and this is a deliberate no-op rather
/// than an error — the callback is still logged and still acknowledged.
async fn apply_notification(state: &AppState, form: &NotificationForm) {
    let Some(trade_no) = form.get("MerchantTradeNo") else {
        return;
    };
    let Some(rtn_msg) = form.get("RtnMsg") else {
        return;
    };

    let mut orders = state.orders.write().await;
    if let Some(order) = orders.get_mut(trade_no) {
        match rtn_msg.as_str() {
            "paid" => order.status = OrderStatus::Paid,
            "refunded" => order.status = OrderStatus::Refunded,
            _ => {}
        }
    }
}

/// `POST /notify`: verifies nothing, always answers `1|OK`.
pub async fn notify(
    State(state): State<Arc<AppState>>,
    Form(form): Form<NotificationForm>,
) -> (StatusCode, &'static str) {
    apply_notification(&state, &form).await;
    record_delivery(&state, PATH_NOTIFY, form, "1|OK").await;
    (StatusCode::OK, "1|OK")
}

/// `POST /notify-strict`: recomputes `CheckMacValue` with the MERCHANT's own
/// `HashKey`/`HashIV` first and answers `400` if it does not match — the
/// "meaningful counterpart" to `/notify` that proves a delivery it accepted
/// really was signed correctly.
pub async fn notify_strict(
    State(state): State<Arc<AppState>>,
    Form(form): Form<NotificationForm>,
) -> (StatusCode, &'static str) {
    let verified = verify_check_mac_value(
        &state.config.merchant_hash_key,
        &state.config.merchant_hash_iv,
        &form,
    );

    if verified {
        apply_notification(&state, &form).await;
        record_delivery(&state, PATH_NOTIFY_STRICT, form, "1|OK").await;
        (StatusCode::OK, "1|OK")
    } else {
        record_delivery(&state, PATH_NOTIFY_STRICT, form, "signature did not verify").await;
        (StatusCode::BAD_REQUEST, "signature did not verify")
    }
}

/// `POST /notify-flaky`: fails the first two deliveries for a given order,
/// then answers `1|OK`. Tracked per `MerchantTradeNo` rather than globally,
/// so two orders retried through this same endpoint in the same process
/// (unlikely inside one scenario, but not ruled out) do not interfere.
pub async fn notify_flaky(
    State(state): State<Arc<AppState>>,
    Form(form): Form<NotificationForm>,
) -> (StatusCode, &'static str) {
    let trade_no = form.get("MerchantTradeNo").cloned().unwrap_or_default();

    let attempt = {
        let mut attempts = state.flaky_attempts.write().await;
        let counter = attempts.entry(trade_no).or_insert(0);
        *counter += 1;
        *counter
    };

    if attempt < 3 {
        record_delivery(&state, PATH_NOTIFY_FLAKY, form, "500").await;
        (StatusCode::INTERNAL_SERVER_ERROR, "having a bad afternoon")
    } else {
        apply_notification(&state, &form).await;
        record_delivery(&state, PATH_NOTIFY_FLAKY, form, "1|OK").await;
        (StatusCode::OK, "1|OK")
    }
}

/// `POST /notify-loose`: `200` with `1|OK` plus a trailing newline — the
/// right string with trailing whitespace, which paygate must NOT accept
/// (`demo-merchant.yaml`: "the right string with trailing whitespace ...
/// paygate would ignore its contents ... a gateway that trims ... accepts
/// merchants that never acknowledged anything"). The data itself is genuine,
/// so the order's own record is still updated — only the acknowledgement is
/// wrong.
pub async fn notify_loose(
    State(state): State<Arc<AppState>>,
    Form(form): Form<NotificationForm>,
) -> (StatusCode, &'static str) {
    apply_notification(&state, &form).await;
    record_delivery(&state, PATH_NOTIFY_LOOSE, form, "1|OK\n").await;
    (StatusCode::OK, "1|OK\n")
}

/// `POST /notify-slow`: answers `1|OK`, correctly, but only after
/// `NOTIFY_TIMEOUT_MS` (plus a margin) has already elapsed — late enough that
/// the notifier has already counted the delivery failed and moved on.
///
/// The delivery is recorded the moment it ARRIVES, before the sleep, not once
/// it is finally answered. A notifier that hit `NOTIFY_TIMEOUT_MS` has already
/// dropped the connection and moved on by the time this would answer, so
/// recording after the fact would leave no trace of a delivery that
/// genuinely happened — the exact thing `notify.feature`'s "not an answer"
/// scenario checks for by reading `/__deliveries`.
pub async fn notify_slow(
    State(state): State<Arc<AppState>>,
    Form(form): Form<NotificationForm>,
) -> (StatusCode, &'static str) {
    record_delivery(&state, PATH_NOTIFY_SLOW, form.clone(), "1|OK (too late)").await;
    let delay = state.config.notify_timeout + std::time::Duration::from_millis(250);
    tokio::time::sleep(delay).await;
    apply_notification(&state, &form).await;
    (StatusCode::OK, "1|OK")
}

/// `POST /notify-reject`: a merchant down for longer than paygate will
/// retry. Always `500`; never updates the order.
pub async fn notify_reject(
    State(state): State<Arc<AppState>>,
    Form(form): Form<NotificationForm>,
) -> (StatusCode, &'static str) {
    record_delivery(&state, PATH_NOTIFY_REJECT, form, "500").await;
    (StatusCode::INTERNAL_SERVER_ERROR, "down")
}

/// `POST /notify-mumble`: `200` with the wrong body (`0|ERROR`) — "the
/// endpoint answered" and "the merchant accepted" are different facts.
/// Never updates the order: this merchant is simulated as unable to process
/// what it received, not merely bad at acknowledging it.
pub async fn notify_mumble(
    State(state): State<Arc<AppState>>,
    Form(form): Form<NotificationForm>,
) -> (StatusCode, &'static str) {
    record_delivery(&state, PATH_NOTIFY_MUMBLE, form, "0|ERROR").await;
    (StatusCode::OK, "0|ERROR")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::config_with_gateway;

    fn state() -> Arc<AppState> {
        // None of these handlers call the gateway, so nothing ever dials
        // this address.
        Arc::new(AppState::new(config_with_gateway(
            "http://127.0.0.1:1".to_string(),
        )))
    }

    fn signed_form(
        hash_key: &str,
        hash_iv: &str,
        trade_no: &str,
        rtn_msg: &str,
    ) -> NotificationForm {
        let mut form = NotificationForm::new();
        form.insert("MerchantID".to_string(), "2000132".to_string());
        form.insert("MerchantTradeNo".to_string(), trade_no.to_string());
        form.insert("TradeNo".to_string(), "2109210000000".to_string());
        form.insert("RtnCode".to_string(), "1".to_string());
        form.insert("RtnMsg".to_string(), rtn_msg.to_string());
        form.insert("TradeAmt".to_string(), "1250".to_string());
        form.insert("PaymentDate".to_string(), "2026/09/21 14:03:11".to_string());
        let mac = paygate_provider::ecpay::check_mac_value(hash_key, hash_iv, &form);
        form.insert("CheckMacValue".to_string(), mac);
        form
    }

    async fn seed_awaiting_order(state: &AppState, trade_no: &str) {
        state.orders.write().await.insert(
            trade_no.to_string(),
            crate::state::Order {
                merchant_trade_no: trade_no.to_string(),
                payment_id: "paygate-id".to_string(),
                amount_minor: 1250,
                currency: "USD".to_string(),
                item_desc: "Shop order".to_string(),
                notify_url: PATH_NOTIFY.to_string(),
                client_back_url: "/shop/result".to_string(),
                status: OrderStatus::AwaitingPayment,
                action: String::new(),
                fields: std::collections::BTreeMap::new(),
                form_issued: false,
            },
        );
    }

    #[tokio::test]
    async fn notify_always_answers_1_ok_and_settles_a_known_order() {
        let state = state();
        seed_awaiting_order(&state, "ACME-1").await;
        let form = signed_form("irrelevant", "irrelevant", "ACME-1", "paid");

        let (status, body) = notify(State(state.clone()), Form(form)).await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "1|OK");
        assert_eq!(
            state.orders.read().await.get("ACME-1").unwrap().status,
            OrderStatus::Paid
        );
        assert_eq!(
            state
                .deliveries
                .read()
                .await
                .get(PATH_NOTIFY)
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn notify_strict_accepts_a_correctly_signed_callback() {
        let state = state();
        seed_awaiting_order(&state, "ACME-2").await;
        let form = signed_form(
            &state.config.merchant_hash_key.clone(),
            &state.config.merchant_hash_iv.clone(),
            "ACME-2",
            "paid",
        );

        let (status, body) = notify_strict(State(state.clone()), Form(form)).await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "1|OK");
        assert_eq!(
            state.orders.read().await.get("ACME-2").unwrap().status,
            OrderStatus::Paid
        );
    }

    #[tokio::test]
    async fn notify_strict_refuses_a_badly_signed_callback_and_does_not_settle_it() {
        let state = state();
        seed_awaiting_order(&state, "ACME-3").await;
        let form = signed_form("some-other-key", "some-other-iv", "ACME-3", "paid");

        let (status, body) = notify_strict(State(state.clone()), Form(form)).await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_ne!(body, "1|OK");
        assert_eq!(
            state.orders.read().await.get("ACME-3").unwrap().status,
            OrderStatus::AwaitingPayment
        );
    }

    #[tokio::test]
    async fn notify_flaky_fails_twice_then_acknowledges_the_third_delivery() {
        let state = state();
        seed_awaiting_order(&state, "ACME-4").await;

        for _ in 0..2 {
            let form = signed_form("irrelevant", "irrelevant", "ACME-4", "paid");
            let (status, body) = notify_flaky(State(state.clone()), Form(form)).await;
            assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
            assert_ne!(body, "1|OK");
        }

        let form = signed_form("irrelevant", "irrelevant", "ACME-4", "paid");
        let (status, body) = notify_flaky(State(state.clone()), Form(form)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "1|OK");
        assert_eq!(
            state.orders.read().await.get("ACME-4").unwrap().status,
            OrderStatus::Paid
        );
        assert_eq!(
            state
                .deliveries
                .read()
                .await
                .get(PATH_NOTIFY_FLAKY)
                .unwrap()
                .len(),
            3
        );
    }

    #[tokio::test]
    async fn notify_loose_answers_1_ok_with_a_trailing_newline_exactly() {
        let state = state();
        let form = signed_form("irrelevant", "irrelevant", "ACME-5", "paid");

        let (status, body) = notify_loose(State(state), Form(form)).await;

        assert_eq!(status, StatusCode::OK);
        // Exactly "1|OK\n" — not "1|OK", which is a DIFFERENT string as far
        // as paygate's own comparison is concerned.
        assert_eq!(body, "1|OK\n");
        assert_ne!(body, "1|OK");
    }

    #[tokio::test]
    async fn notify_slow_answers_correctly_but_only_after_the_configured_timeout() {
        let state = state();
        let form = signed_form("irrelevant", "irrelevant", "ACME-6", "paid");

        let started = tokio::time::Instant::now();
        let (status, body) = notify_slow(State(state.clone()), Form(form)).await;
        let elapsed = started.elapsed();

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "1|OK");
        assert!(
            elapsed >= state.config.notify_timeout,
            "answered after {elapsed:?}, before the configured timeout {:?}",
            state.config.notify_timeout
        );
    }

    /// A notifier that hit `NOTIFY_TIMEOUT_MS` has already dropped the
    /// connection and moved on well before `/notify-slow` gets around to
    /// answering. If the delivery were only recorded once the handler
    /// finishes, such a delivery would leave no trace in `/__deliveries` —
    /// exactly the gap `notify.feature`'s "An answer that arrives after the
    /// notifier gave up waiting is not an answer" scenario checks for. So the
    /// delivery must show up as soon as it ARRIVES, well before the handler's
    /// own delayed answer.
    #[tokio::test]
    async fn notify_slow_records_the_delivery_immediately_not_after_answering() {
        let state = state();
        let form = signed_form("irrelevant", "irrelevant", "ACME-9", "paid");

        // `notify_timeout` is 20ms in the test fixture, so the handler will
        // not answer for at least 270ms. Give it a moment to be scheduled,
        // then check the delivery log long before that answer could exist.
        let handle = tokio::spawn(notify_slow(State(state.clone()), Form(form)));
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;

        assert!(
            !handle.is_finished(),
            "the handler answered before its configured delay; the test's timing assumption is wrong"
        );
        assert_eq!(
            state
                .deliveries
                .read()
                .await
                .get(PATH_NOTIFY_SLOW)
                .map(|d| d.len())
                .unwrap_or(0),
            1,
            "the delivery must be recorded on arrival, not once the (much later) answer is sent"
        );

        // Let the handler finish so the spawned task does not leak past the test.
        let (status, body) = handle.await.unwrap();
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "1|OK");
    }

    #[tokio::test]
    async fn notify_reject_always_answers_500_and_never_settles_anything() {
        let state = state();
        seed_awaiting_order(&state, "ACME-7").await;
        let form = signed_form("irrelevant", "irrelevant", "ACME-7", "paid");

        let (status, _) = notify_reject(State(state.clone()), Form(form)).await;

        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(
            state.orders.read().await.get("ACME-7").unwrap().status,
            OrderStatus::AwaitingPayment
        );
        assert_eq!(
            state
                .deliveries
                .read()
                .await
                .get(PATH_NOTIFY_REJECT)
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn notify_mumble_answers_200_with_the_wrong_body() {
        let state = state();
        let form = signed_form("irrelevant", "irrelevant", "ACME-8", "paid");

        let (status, body) = notify_mumble(State(state), Form(form)).await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "0|ERROR");
        assert_ne!(body, "1|OK");
    }
}
