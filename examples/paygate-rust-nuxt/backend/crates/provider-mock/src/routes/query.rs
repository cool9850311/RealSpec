//! `QueryTradeInfo` — the reconciler's whole world. A scenario arms the next
//! answer through `/__control/query`; every query this endpoint answers is
//! logged so that "paygate asked" (and, more often, "paygate did NOT ask
//! yet") is checkable. Queried too fast, it answers `403` and stops
//! answering for a while, exactly as the real one does.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::{Form, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use paygate_domain::ProviderCode;
use paygate_provider::ecpay::is_timestamp_fresh;

use crate::state::{AppState, ArmedQuery, QueryLogEntry};
use crate::util::now_taipei_string;
use crate::wire;

const THROTTLE_WINDOW: Duration = Duration::from_secs(30 * 60);

/// The one amount whose QUERY answer is slow. A scenario that needs a provider
/// call to still be in flight while something else happens says so with the
/// order's own amount, rather than the step registry growing a word for it.
///
/// It matters for exactly one thing: the reconciler holds its batch claim for
/// as long as the provider takes to answer, so a scenario racing a
/// reconciliation pass against anything else needs that window to be wider than
/// the microseconds an instant answer leaves (`spec.md`, *Concurrency*).
///
/// Deliberately NOT `777`, the refund side's slow amount: an order of 777 would
/// have a slow refund as well, and 800 ms of it inside `PSP_TIMEOUT_MS` (1000 ms
/// in the suites) is a scenario that fails on a busy machine rather than on a
/// bug. One number per slow CALL, not one number for "slow".
const SLOW_ANSWER_AMOUNT: i64 = 888;
const SLOW_ANSWER_DELAY: Duration = Duration::from_millis(800);

/// How long this answer is held before it is sent — a pure decision, so the
/// rule is a unit test and not something only a live query can show.
fn answer_delay(amount: Option<i64>) -> Duration {
    match amount {
        Some(SLOW_ANSWER_AMOUNT) => SLOW_ANSWER_DELAY,
        _ => Duration::ZERO,
    }
}

/// Whether the blackout `armed_query::Throttled` starts is still in effect —
/// a pure decision, pulled out of the mutex-guarded handler so the "403,
/// then silence for a while" rule (spec.md, "The payment provider mock") is
/// a real unit test rather than something only a live query can exercise.
fn is_throttled(until: Option<Instant>, now: Instant) -> bool {
    matches!(until, Some(u) if now < u)
}

fn build_answer(
    state: &AppState,
    provider: ProviderCode,
    provider_trade_no: &str,
    merchant_id: &str,
    trade_status: &str,
    forged: bool,
) -> String {
    let mut fields = BTreeMap::new();
    fields.insert("MerchantID".to_string(), merchant_id.to_string());
    fields.insert("MerchantTradeNo".to_string(), provider_trade_no.to_string());
    fields.insert("TradeStatus".to_string(), trade_status.to_string());

    if trade_status == "1" {
        let attempt = state
            .attempts
            .lock()
            .expect("attempts lock")
            .get(provider_trade_no)
            .cloned();
        let amount = attempt.as_ref().map(|a| a.amount).unwrap_or(0);
        fields.insert("TradeAmt".to_string(), amount.to_string());
        fields.insert("PaymentDate".to_string(), now_taipei_string());
        match attempt.as_ref().and_then(|a| a.card.clone()) {
            Some(card) => {
                fields.insert("card4no".to_string(), card.card4);
                fields.insert("card6no".to_string(), card.card6);
                fields.insert("eci".to_string(), card.eci);
                if let Some(auth) = card.auth_code {
                    fields.insert("auth_code".to_string(), auth);
                }
            }
            None => {
                fields.insert("card4no".to_string(), "4242".to_string());
                fields.insert("card6no".to_string(), "424242".to_string());
            }
        }
    }

    match provider {
        ProviderCode::Ecpay => {
            let creds = if forged {
                wire::forged_ecpay_creds()
            } else {
                state.config.ecpay.clone()
            };
            wire::ecpay_sign(&creds, &mut fields);
            fields
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join("&")
        }
        ProviderCode::Newebpay => {
            let creds = if forged {
                wire::forged_newebpay_creds()
            } else {
                state.config.newebpay.clone()
            };
            let outer = wire::newebpay_wrap(&creds, merchant_id, &fields);
            outer
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join("&")
        }
    }
}

async fn handle_query(
    state: Arc<AppState>,
    provider: ProviderCode,
    provider_trade_no: String,
    merchant_id: String,
    verified: bool,
) -> Response {
    if !verified {
        state
            .query_log
            .lock()
            .expect("query_log lock")
            .push(QueryLogEntry {
                provider_trade_no,
                signature_verified: false,
                answered: "refused".to_string(),
            });
        return (StatusCode::BAD_REQUEST, "CheckMacValue Error").into_response();
    }

    let now = Instant::now();
    let currently_throttled = {
        let until = state.query_throttled_until.lock().expect("throttle lock");
        is_throttled(*until, now)
    };
    if currently_throttled {
        state
            .query_log
            .lock()
            .expect("query_log lock")
            .push(QueryLogEntry {
                provider_trade_no,
                signature_verified: true,
                answered: "403".to_string(),
            });
        return (StatusCode::FORBIDDEN, "Too many requests").into_response();
    }

    let armed = state.armed_query.lock().expect("armed_query lock").take();
    let (status, body, answered) = match armed {
        Some(ArmedQuery::Throttled) => {
            *state.query_throttled_until.lock().expect("throttle lock") =
                Some(now + THROTTLE_WINDOW);
            (
                StatusCode::FORBIDDEN,
                "Too many requests".to_string(),
                "403".to_string(),
            )
        }
        Some(ArmedQuery::Forged) => {
            let body = build_answer(
                &state,
                provider,
                &provider_trade_no,
                &merchant_id,
                "1",
                true,
            );
            (StatusCode::OK, body, "1 (forged)".to_string())
        }
        Some(ArmedQuery::TradeStatus(trade_status)) => {
            let body = build_answer(
                &state,
                provider,
                &provider_trade_no,
                &merchant_id,
                &trade_status,
                false,
            );
            (StatusCode::OK, body, trade_status)
        }
        // Unarmed defaults to "still in progress" — the harmless answer that
        // asks paygate to do nothing and try again later.
        None => {
            let body = build_answer(
                &state,
                provider,
                &provider_trade_no,
                &merchant_id,
                "0",
                false,
            );
            (StatusCode::OK, body, "0".to_string())
        }
    };

    let amount = state
        .attempts
        .lock()
        .expect("attempts lock")
        .get(&provider_trade_no)
        .map(|a| a.amount);

    state
        .query_log
        .lock()
        .expect("query_log lock")
        .push(QueryLogEntry {
            provider_trade_no,
            signature_verified: true,
            answered,
        });

    // Held AFTER the asking is logged: the question reached the provider, and
    // it is the ANSWER that is late.
    let delay = answer_delay(amount);
    if !delay.is_zero() {
        tokio::time::sleep(delay).await;
    }

    (status, body).into_response()
}

pub async fn ecpay_query(
    State(state): State<Arc<AppState>>,
    Form(form): Form<BTreeMap<String, String>>,
) -> Response {
    let signature_ok = wire::ecpay_verify(&state.config.ecpay, &form);
    let fresh = form
        .get("TimeStamp")
        .is_some_and(|ts| is_timestamp_fresh(ts, chrono::Utc::now()));
    let verified = signature_ok && fresh;
    let provider_trade_no = form.get("MerchantTradeNo").cloned().unwrap_or_default();
    let merchant_id = form.get("MerchantID").cloned().unwrap_or_default();
    handle_query(
        state,
        ProviderCode::Ecpay,
        provider_trade_no,
        merchant_id,
        verified,
    )
    .await
}

pub async fn newebpay_query(
    State(state): State<Arc<AppState>>,
    Form(form): Form<BTreeMap<String, String>>,
) -> Response {
    let decrypted = wire::newebpay_verify(&state.config.newebpay, &form);
    let fresh = decrypted
        .as_ref()
        .and_then(|inner| inner.get("TimeStamp"))
        .is_some_and(|ts| is_timestamp_fresh(ts, chrono::Utc::now()));
    let verified = decrypted.is_some() && fresh;
    let provider_trade_no = decrypted
        .as_ref()
        .and_then(|inner| inner.get("MerchantTradeNo").cloned())
        .unwrap_or_default();
    let merchant_id = decrypted
        .as_ref()
        .and_then(|inner| inner.get("MerchantID").cloned())
        .unwrap_or_default();
    handle_query(
        state,
        ProviderCode::Newebpay,
        provider_trade_no,
        merchant_id,
        verified,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::state::{Attempt, CardOutcome};
    use paygate_provider::PlatformCredentials;

    fn test_config() -> Config {
        Config {
            port: 0,
            callback_urls: vec!["http://replica".to_string()],
            ecpay: PlatformCredentials {
                platform_id: "3002607".to_string(),
                hash_key: "pwFHCqoQZGmho4w6".to_string(),
                hash_iv: "EkRm7iFT261dpevs".to_string(),
            },
            newebpay: PlatformCredentials {
                platform_id: "MS12345678".to_string(),
                hash_key: "Fs5cX1TGqYZ3kLmN8pQrS2tUvW4xY6zA".to_string(),
                hash_iv: "B7cD9eF1gH3iJ5kL".to_string(),
            },
        }
    }

    fn state_with_paid_attempt() -> AppState {
        let state = AppState::new(test_config());
        state.attempts.lock().unwrap().insert(
            "ACME00000001SNtest".to_string(),
            Attempt {
                provider: ProviderCode::Ecpay,
                provider_merchant_id: "2000132".to_string(),
                provider_trade_no: "ACME00000001SNtest".to_string(),
                amount: 2500,
                return_url: "https://paygate.example.com/api/v1/webhooks/ecpay".to_string(),
                client_back_url: "/shop/result".to_string(),
                original_form: std::collections::BTreeMap::new(),
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
            },
        );
        state
    }

    fn parse_kv(body: &str) -> BTreeMap<String, String> {
        body.split('&')
            .filter_map(|pair| pair.split_once('='))
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn every_armed_trade_status_answers_signed_and_verifiable() {
        let state = state_with_paid_attempt();
        for status in ["0", "1", "10200095"] {
            let body = build_answer(
                &state,
                ProviderCode::Ecpay,
                "ACME00000001SNtest",
                "2000132",
                status,
                false,
            );
            let fields = parse_kv(&body);
            assert_eq!(fields.get("TradeStatus"), Some(&status.to_string()));
            assert!(wire::ecpay_verify(&state.config.ecpay, &fields));
        }
    }

    #[test]
    fn a_paid_answer_carries_the_attempts_own_amount_and_card() {
        let state = state_with_paid_attempt();
        let body = build_answer(
            &state,
            ProviderCode::Ecpay,
            "ACME00000001SNtest",
            "2000132",
            "1",
            false,
        );
        let fields = parse_kv(&body);
        assert_eq!(fields.get("TradeAmt"), Some(&"2500".to_string()));
        assert_eq!(fields.get("card4no"), Some(&"4242".to_string()));
    }

    #[test]
    fn a_forged_answer_does_not_verify_against_the_real_pair() {
        let state = state_with_paid_attempt();
        let body = build_answer(
            &state,
            ProviderCode::Ecpay,
            "ACME00000001SNtest",
            "2000132",
            "1",
            true,
        );
        let fields = parse_kv(&body);
        assert!(!wire::ecpay_verify(&state.config.ecpay, &fields));
    }

    #[test]
    fn newebpays_answer_is_signed_and_verifiable_too() {
        let state = state_with_paid_attempt();
        let body = build_answer(
            &state,
            ProviderCode::Newebpay,
            "ACME00000001SNtest",
            "MS12345678",
            "1",
            false,
        );
        let fields = parse_kv(&body);
        assert!(wire::newebpay_verify(&state.config.newebpay, &fields).is_some());
    }

    #[test]
    fn the_403_throttle_holds_until_its_own_deadline_and_then_lifts() {
        let now = Instant::now();
        assert!(!is_throttled(None, now));
        assert!(is_throttled(Some(now + Duration::from_secs(1)), now));
        assert!(!is_throttled(Some(now - Duration::from_secs(1)), now));
    }

    #[test]
    fn only_the_slow_amount_holds_the_answer() {
        assert_eq!(answer_delay(Some(888)), Duration::from_millis(800));
        // 777 is the REFUND side's slow amount and must not be this one too.
        assert_eq!(answer_delay(Some(777)), Duration::ZERO);
        assert_eq!(answer_delay(Some(2500)), Duration::ZERO);
        assert_eq!(answer_delay(None), Duration::ZERO);
    }
}
