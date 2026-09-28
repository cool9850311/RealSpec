//! The callback queue and its one release mechanism, shared by the issuer's
//! 3-D Secure page (one attempt, released by a browser) and
//! `POST /__control/callbacks` (every pending attempt, released by the API
//! harness playing the customer). It queues callbacks; it does not send them
//! on its own. Delivery is in queue order; all copies of one callback are
//! released together at one instant, their answers awaited, then the next
//! callback's (`spec/openapi/payment-provider.yaml`, `releaseQueuedCallbacks`:
//! "copies of one callback are fired at one instant").

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use paygate_domain::ProviderCode;

use crate::state::{AppState, Attempt, CallbackRecord, DeliveredRecord};
use crate::util::{now_taipei_string, path_of};
use crate::{state::provider_str, wire};

/// The four things a real provider can do to a callback besides reporting it
/// honestly (spec.md, "The payment provider mock").
#[derive(Debug, Clone, Copy)]
pub enum Variant {
    Honest,
    Forged,
    Simulated,
    WrongAmount,
    ReturnCode(i32),
}

struct Logical {
    rtn_code: i32,
    rtn_msg: String,
    trade_amt: i64,
    simulate_paid: bool,
    card4: Option<String>,
    card6: Option<String>,
    eci: Option<String>,
    auth_code: Option<String>,
    failure_code: Option<String>,
}

fn logical_for(attempt: &Attempt, variant: Variant) -> Logical {
    let card = attempt.card.clone();
    let (mut rtn_code, mut failure_code, card4, card6, eci, auth_code) = match card {
        Some(c) => (
            c.rtn_code,
            c.failure_code,
            Some(c.card4),
            Some(c.card6),
            Some(c.eci),
            c.auth_code,
        ),
        None => (1, None, None, None, None, None),
    };
    let mut trade_amt = attempt.amount;
    let mut simulate_paid = false;

    match variant {
        Variant::Honest | Variant::Forged => {}
        Variant::Simulated => simulate_paid = true,
        Variant::WrongAmount => trade_amt += 1,
        Variant::ReturnCode(code) => {
            rtn_code = code;
            failure_code = None;
        }
    }

    let rtn_msg = if rtn_code == 1 {
        "Succeeded".to_string()
    } else {
        "Failed".to_string()
    };

    Logical {
        rtn_code,
        rtn_msg,
        trade_amt,
        simulate_paid,
        card4,
        card6,
        eci,
        auth_code,
        failure_code,
    }
}

/// The wire-shaped record of what an attempt's callback currently says — used
/// both for `GET /__control/callbacks`'s `pending` list (always `Honest`)
/// and, after a release, for the log entries [`release`] appends.
pub fn record_for(attempt: &Attempt, variant: Variant) -> CallbackRecord {
    to_record(attempt.provider, attempt, &logical_for(attempt, variant))
}

fn to_record(provider: ProviderCode, attempt: &Attempt, logical: &Logical) -> CallbackRecord {
    CallbackRecord {
        provider: provider_str(provider),
        provider_trade_no: attempt.provider_trade_no.clone(),
        event_id: attempt.charge_id.clone(),
        rtn_code: logical.rtn_code.to_string(),
        rtn_msg: logical.rtn_msg.clone(),
        simulate_paid: if logical.simulate_paid { "1" } else { "0" }.to_string(),
        trade_amt: logical.trade_amt.to_string(),
        card4no: logical.card4.clone(),
        card6no: logical.card6.clone(),
        eci: logical.eci.clone(),
        auth_code: logical.auth_code.clone(),
        failure_code: logical.failure_code.clone(),
    }
}

fn ecpay_wire_form(
    state: &AppState,
    attempt: &Attempt,
    logical: &Logical,
    forged: bool,
) -> BTreeMap<String, String> {
    let mut form = BTreeMap::new();
    form.insert(
        "MerchantID".to_string(),
        attempt.provider_merchant_id.clone(),
    );
    form.insert(
        "MerchantTradeNo".to_string(),
        attempt.provider_trade_no.clone(),
    );
    form.insert("TradeNo".to_string(), attempt.charge_id.clone());
    form.insert("RtnCode".to_string(), logical.rtn_code.to_string());
    form.insert("RtnMsg".to_string(), logical.rtn_msg.clone());
    form.insert("TradeAmt".to_string(), logical.trade_amt.to_string());
    form.insert("PaymentDate".to_string(), now_taipei_string());
    form.insert(
        "SimulatePaid".to_string(),
        if logical.simulate_paid { "1" } else { "0" }.to_string(),
    );
    if let Some(c) = &logical.card4 {
        form.insert("card4no".to_string(), c.clone());
    }
    if let Some(c) = &logical.card6 {
        form.insert("card6no".to_string(), c.clone());
    }
    if let Some(c) = &logical.eci {
        form.insert("eci".to_string(), c.clone());
    }
    if let Some(c) = &logical.auth_code {
        form.insert("auth_code".to_string(), c.clone());
    }
    if let Some(c) = &logical.failure_code {
        form.insert("failure_code".to_string(), c.clone());
    }
    let creds = if forged {
        wire::forged_ecpay_creds()
    } else {
        state.config.ecpay.clone()
    };
    wire::ecpay_sign(&creds, &mut form);
    form
}

fn newebpay_wire_form(
    state: &AppState,
    attempt: &Attempt,
    logical: &Logical,
    forged: bool,
) -> BTreeMap<String, String> {
    let mut inner = BTreeMap::new();
    inner.insert(
        "MerchantTradeNo".to_string(),
        attempt.provider_trade_no.clone(),
    );
    inner.insert("TradeNo".to_string(), attempt.charge_id.clone());
    inner.insert("RtnCode".to_string(), logical.rtn_code.to_string());
    inner.insert("RtnMsg".to_string(), logical.rtn_msg.clone());
    inner.insert("TradeAmt".to_string(), logical.trade_amt.to_string());
    inner.insert("PaymentDate".to_string(), now_taipei_string());
    inner.insert(
        "SimulatePaid".to_string(),
        if logical.simulate_paid { "1" } else { "0" }.to_string(),
    );
    if let Some(c) = &logical.card4 {
        inner.insert("card4no".to_string(), c.clone());
    }
    if let Some(c) = &logical.card6 {
        inner.insert("card6no".to_string(), c.clone());
    }
    if let Some(c) = &logical.eci {
        inner.insert("eci".to_string(), c.clone());
    }
    if let Some(c) = &logical.auth_code {
        inner.insert("auth_code".to_string(), c.clone());
    }
    if let Some(c) = &logical.failure_code {
        inner.insert("failure_code".to_string(), c.clone());
    }
    let creds = if forged {
        wire::forged_newebpay_creds()
    } else {
        state.config.newebpay.clone()
    };
    wire::newebpay_wrap(&creds, &attempt.provider_merchant_id, &inner)
}

async fn post_form(
    http: reqwest::Client,
    url: String,
    form: BTreeMap<String, String>,
) -> (Option<i64>, String) {
    match http.post(&url).form(&form).send().await {
        Ok(resp) => {
            let status = Some(i64::from(resp.status().as_u16()));
            let body = resp.text().await.unwrap_or_default();
            (status, body)
        }
        // The target process was gone. Recorded as refused, with no status
        // (`spec/openapi/payment-provider.yaml`, `DeliveredCallback`), never
        // a failed step.
        Err(_) => (None, String::new()),
    }
}

/// Releases either one attempt's callback (`only = Some(provider_trade_no)`,
/// the issuer's own action) or every currently pending one in queue order
/// (`only = None`, `POST /__control/callbacks`), `times` copies each, with
/// `variant` applied uniformly to whatever this call releases.
/// The one amount whose provider is SLOW, the same number the query side holds
/// its answer for. The callback is late by less than that answer, so a scenario
/// that releases both at one instant knows the callback arrives while the query
/// is still outstanding — which is the only arrangement that can show whether
/// the claim a reconciliation pass holds is taken in the right order.
const SLOW_PROVIDER_AMOUNT: i64 = 888;
const SLOW_CALLBACK_DELAY: Duration = Duration::from_millis(300);

/// How long this callback waits before it is posted — a pure decision, so the
/// rule is a unit test rather than something only a live delivery can show.
fn delivery_delay(amount: i64) -> Duration {
    match amount {
        SLOW_PROVIDER_AMOUNT => SLOW_CALLBACK_DELAY,
        _ => Duration::ZERO,
    }
}

pub async fn release(
    state: &Arc<AppState>,
    only: Option<&str>,
    times: u32,
    variant: Variant,
) -> Vec<DeliveredRecord> {
    let queue: Vec<String> = match only {
        Some(trade_no) => vec![trade_no.to_string()],
        None => state
            .pending_order
            .lock()
            .expect("pending_order lock")
            .clone(),
    };

    let n = state.config.callback_urls.len().max(1);
    let mut delivery_index: usize = 0;
    let mut all_delivered = Vec::new();

    for trade_no in queue {
        let attempt = {
            let attempts = state.attempts.lock().expect("attempts lock");
            attempts.get(&trade_no).cloned()
        };
        let Some(attempt) = attempt else { continue };
        if !attempt.pending || attempt.card.is_none() {
            continue;
        }

        let logical = logical_for(&attempt, variant);
        let record = to_record(attempt.provider, &attempt, &logical);
        let ack_body = "1|OK";

        // A provider that is slow about this order is slow about ALL of it:
        // the callback is late by the same rule that holds its query answer
        // (`routes::query`, `SLOW_ANSWER_AMOUNT`). One amount, one story.
        // It is what lets a scenario put a callback INSIDE a window another
        // actor is holding open, rather than hoping the two land in the order
        // it needs — see `reconcile.feature`, "a reconciliation pass and the
        // duplicate's own callback reach the same attempt at once".
        let delay = delivery_delay(attempt.amount);
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }

        let mut handles = Vec::with_capacity(times as usize);
        for _ in 0..times.max(1) {
            let target_base = state.config.callback_urls[delivery_index % n].clone();
            delivery_index += 1;
            let url = format!("{target_base}{}", path_of(&attempt.return_url));
            let form = match attempt.provider {
                ProviderCode::Ecpay => ecpay_wire_form(
                    state,
                    &attempt,
                    &logical,
                    matches!(variant, Variant::Forged),
                ),
                ProviderCode::Newebpay => newebpay_wire_form(
                    state,
                    &attempt,
                    &logical,
                    matches!(variant, Variant::Forged),
                ),
            };
            let http = state.http.clone();
            handles.push(tokio::spawn(post_form(http, url, form)));
        }

        let mut delivered_for_this = Vec::with_capacity(handles.len());
        for handle in handles {
            let (status, body) = handle.await.unwrap_or((None, String::new()));
            let accepted = body == ack_body;
            delivered_for_this.push(DeliveredRecord {
                callback: record.clone(),
                status,
                body,
                accepted,
            });
        }

        let any_accepted = delivered_for_this.iter().any(|d| d.accepted);
        if any_accepted {
            let mut attempts = state.attempts.lock().expect("attempts lock");
            if let Some(a) = attempts.get_mut(&trade_no) {
                a.pending = false;
            }
            drop(attempts);
            let mut pending_order = state.pending_order.lock().expect("pending_order lock");
            pending_order.retain(|t| t != &trade_no);
        }

        {
            let mut log = state.delivered_log.lock().expect("delivered_log lock");
            log.extend(delivered_for_this.clone());
        }
        all_delivered.extend(delivered_for_this);
    }

    all_delivered
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::state::CardOutcome;
    use std::collections::BTreeMap as Map;

    fn paid_attempt() -> Attempt {
        Attempt {
            provider: ProviderCode::Ecpay,
            provider_merchant_id: "2000132".to_string(),
            provider_trade_no: "ACME00000001SNtest".to_string(),
            amount: 2500,
            return_url: "https://paygate.example.com/api/v1/webhooks/ecpay".to_string(),
            client_back_url: "/shop/result".to_string(),
            original_form: Map::new(),
            charge_id: "2109210000000".to_string(),
            card: Some(CardOutcome {
                rtn_code: 1,
                failure_code: None,
                card6: "424242".to_string(),
                card4: "4242".to_string(),
                eci: "05".to_string(),
                auth_code: Some("777888".to_string()),
            }),
            pending: true,
        }
    }

    #[test]
    fn honest_and_forged_report_the_cards_own_outcome_unchanged() {
        for variant in [Variant::Honest, Variant::Forged] {
            let logical = logical_for(&paid_attempt(), variant);
            assert_eq!(logical.rtn_code, 1);
            assert_eq!(logical.trade_amt, 2500);
            assert!(!logical.simulate_paid);
        }
    }

    #[test]
    fn simulated_sets_simulate_paid_without_touching_rtn_code() {
        let logical = logical_for(&paid_attempt(), Variant::Simulated);
        assert!(logical.simulate_paid);
        assert_eq!(
            logical.rtn_code, 1,
            "SimulatePaid is independent of RtnCode"
        );
    }

    #[test]
    fn wrong_amount_sends_an_amount_that_is_not_the_orders() {
        let logical = logical_for(&paid_attempt(), Variant::WrongAmount);
        assert_ne!(logical.trade_amt, 2500);
    }

    #[test]
    fn return_code_overrides_rtn_code_and_drops_any_failure_code() {
        let logical = logical_for(&paid_attempt(), Variant::ReturnCode(10_300_066));
        assert_eq!(logical.rtn_code, 10_300_066);
        assert!(logical.failure_code.is_none());
    }

    #[test]
    fn record_for_carries_the_provider_and_trade_number_through() {
        let record = record_for(&paid_attempt(), Variant::Honest);
        assert_eq!(record.provider, "ecpay");
        assert_eq!(record.provider_trade_no, "ACME00000001SNtest");
        assert_eq!(record.rtn_code, "1");
        assert_eq!(record.trade_amt, "2500");
    }

    #[test]
    fn ecpay_wire_form_is_signed_with_the_platforms_pair_and_verifies() {
        let config = Config {
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
        };
        let state = AppState::new(config);
        let attempt = paid_attempt();
        let logical = logical_for(&attempt, Variant::Honest);
        let honest = ecpay_wire_form(&state, &attempt, &logical, false);
        assert!(wire::ecpay_verify(&state.config.ecpay, &honest));

        let forged = ecpay_wire_form(&state, &attempt, &logical, true);
        assert!(!wire::ecpay_verify(&state.config.ecpay, &forged));
    }

    /// Pins the queue-release rule (spec.md / `format.yml`,
    /// `provider_delivers_callbacks`): "a delivery paygate ACKNOWLEDGED...
    /// leaves the queue. One it refused stays queued and can be released
    /// again." A callback answered anything but exactly `1|OK` must still be
    /// deliverable on the next release; one answered `1|OK` must not be
    /// delivered again.
    #[tokio::test]
    async fn a_refused_delivery_stays_queued_and_an_accepted_one_leaves_it() {
        use axum::routing::post;
        use std::sync::atomic::{AtomicBool, Ordering};

        let accept = Arc::new(AtomicBool::new(false));
        let accept_for_handler = accept.clone();
        let app = axum::Router::new().route(
            "/api/v1/webhooks/ecpay",
            post(move || {
                let accept = accept_for_handler.clone();
                async move {
                    if accept.load(Ordering::SeqCst) {
                        "1|OK".to_string()
                    } else {
                        "0|Refused".to_string()
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let config = Config {
            port: 0,
            callback_urls: vec![format!("http://{addr}")],
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
        };
        let state = Arc::new(AppState::new(config));
        let attempt = paid_attempt();
        let trade_no = attempt.provider_trade_no.clone();
        state
            .attempts
            .lock()
            .unwrap()
            .insert(trade_no.clone(), attempt);
        state.pending_order.lock().unwrap().push(trade_no.clone());

        // Refused: the delivery is recorded but the attempt stays owed.
        accept.store(false, Ordering::SeqCst);
        let first = release(&state, None, 1, Variant::Honest).await;
        assert_eq!(first.len(), 1);
        assert!(!first[0].accepted);
        assert!(
            state.pending_order.lock().unwrap().contains(&trade_no),
            "a refused callback must stay in the queue"
        );
        assert!(
            state
                .attempts
                .lock()
                .unwrap()
                .get(&trade_no)
                .unwrap()
                .pending
        );

        // Released again, now accepted: it leaves the queue for good.
        accept.store(true, Ordering::SeqCst);
        let second = release(&state, None, 1, Variant::Honest).await;
        assert_eq!(second.len(), 1);
        assert!(second[0].accepted);
        assert!(
            !state.pending_order.lock().unwrap().contains(&trade_no),
            "an acknowledged callback must leave the queue"
        );
        assert!(
            !state
                .attempts
                .lock()
                .unwrap()
                .get(&trade_no)
                .unwrap()
                .pending
        );

        // A third release finds nothing left to deliver.
        let third = release(&state, None, 1, Variant::Honest).await;
        assert!(third.is_empty());
        assert_eq!(state.delivered_log.lock().unwrap().len(), 2);
    }

    #[test]
    fn only_the_slow_provider_holds_its_callback_back() {
        assert_eq!(delivery_delay(888), Duration::from_millis(300));
        assert_eq!(delivery_delay(2500), Duration::ZERO);
        // Shorter than the query answer it has to arrive inside.
        assert!(delivery_delay(888) < Duration::from_millis(800));
    }
}
