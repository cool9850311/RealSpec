//! Callbacks from the payment provider (`openapi.yaml`, tag `Provider`) —
//! "the only thing that settles an order". No credential of the kind every
//! other endpoint here accepts: the provider's signature IS the credential,
//! verified per request by `paygate_provider::adapter(code).parse_callback`.

use std::collections::BTreeMap;

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

use paygate_domain::{CallbackOutcome, ProviderCode};
use paygate_infra::repo;
use paygate_provider::PlatformCredentials;

use crate::db;
use crate::state::AppState;

fn parse_provider_code(raw: &str) -> Option<ProviderCode> {
    match raw {
        "ecpay" => Some(ProviderCode::Ecpay),
        "newebpay" => Some(ProviderCode::Newebpay),
        _ => None,
    }
}

fn text_response(status: StatusCode, body: &'static str) -> Response {
    let mut response = (status, body).into_response();
    response.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("text/plain"),
    );
    response
}

pub async fn receive_callback(
    State(state): State<AppState>,
    Path(provider): Path<String>,
    body: Bytes,
) -> Response {
    let Some(code) = parse_provider_code(&provider) else {
        return text_response(StatusCode::NOT_FOUND, "0|Error");
    };

    let provider_row = match db::fetch_provider(&state.db, code).await {
        Ok(Some(row)) => row,
        Ok(None) => {
            tracing::error!(provider = %provider, "no platform credentials configured for this provider");
            return text_response(StatusCode::INTERNAL_SERVER_ERROR, "0|Error");
        }
        Err(err) => {
            tracing::error!(error = %err, "database error loading provider credentials");
            return text_response(StatusCode::INTERNAL_SERVER_ERROR, "0|Error");
        }
    };
    let creds = PlatformCredentials {
        platform_id: provider_row.platform_id.clone(),
        hash_key: provider_row.hash_key.clone(),
        hash_iv: provider_row.hash_iv.clone(),
    };

    let form: BTreeMap<String, String> = match serde_urlencoded::from_bytes(&body) {
        Ok(form) => form,
        Err(_) => return text_response(StatusCode::BAD_REQUEST, "0|CheckMacValue Error"),
    };

    let adapter = paygate_provider::adapter(code);
    // Verify first (`webhooks.feature`): a body nobody signed with this
    // provider's key changes nothing at all, and is not this function's
    // concern beyond refusing it.
    let facts = match adapter.parse_callback(&creds, &form) {
        Ok(facts) => facts,
        Err(_) => return text_response(StatusCode::BAD_REQUEST, "0|CheckMacValue Error"),
    };

    // A read-only lookup, purely to find which merchant this callback is
    // about — `apply_callback` redoes this under its own row lock and is the
    // one whose answer actually counts.
    let attempt_payment_id =
        match find_payment_id_for_attempt(&state, code, &facts.provider_trade_no).await {
            Ok(Some(id)) => id,
            Ok(None) => return text_response(StatusCode::BAD_REQUEST, "0|UnknownOrder"),
            Err(err) => {
                tracing::error!(error = %err, "database error looking up the callback's attempt");
                return text_response(StatusCode::INTERNAL_SERVER_ERROR, "0|Error");
            }
        };
    let payment = match db::find_payment_by_id_unscoped(&state.db, attempt_payment_id).await {
        Ok(Some(p)) => p,
        Ok(None) => return text_response(StatusCode::BAD_REQUEST, "0|UnknownOrder"),
        Err(err) => {
            tracing::error!(error = %err, "database error loading the callback's payment");
            return text_response(StatusCode::INTERNAL_SERVER_ERROR, "0|Error");
        }
    };
    let merchant = match db::fetch_merchant(&state.db, payment.merchant_id).await {
        Ok(Some(m)) => m,
        Ok(None) => {
            tracing::error!(
                merchant_id = payment.merchant_id,
                "callback for a payment whose merchant no longer exists"
            );
            return text_response(StatusCode::INTERNAL_SERVER_ERROR, "0|Error");
        }
        Err(err) => {
            tracing::error!(error = %err, "database error loading the callback's merchant");
            return text_response(StatusCode::INTERNAL_SERVER_ERROR, "0|Error");
        }
    };

    let merchant_ctx = repo::MerchantContext {
        merchant_id: merchant.id,
        provider_merchant_id: merchant.provider_merchant_id.clone(),
        merchant_trade_no: payment.merchant_trade_no.clone(),
        notify_url: payment.notify_url.clone(),
    };

    let application = match repo::apply_callback(
        &state.db,
        code,
        &facts,
        &merchant_ctx,
        &merchant.hash_key,
        &merchant.hash_iv,
    )
    .await
    {
        Ok(app) => app,
        Err(paygate_infra::Error::PaymentNotFound) => {
            return text_response(StatusCode::BAD_REQUEST, "0|UnknownOrder")
        }
        Err(err) => {
            // Nothing was committed — the provider must send it again
            // (`openapi.yaml`, `receiveEcpayCallback`, `500`).
            tracing::error!(error = %err, "applying the callback failed; nothing was committed");
            return text_response(StatusCode::INTERNAL_SERVER_ERROR, "0|Error");
        }
    };

    // `spec.md`, "When the customer pays twice": the duplicate is recorded
    // (already true — `apply_callback`'s own transaction just committed it)
    // before any refund is ever attempted, and the refund itself is this
    // merchant's own declared preference.
    if application.outcome == CallbackOutcome::Duplicate {
        if let Some(amount) = application.duplicate_amount {
            if merchant.duplicate_auto_refund {
                let payment_for_refund = payment.clone();
                match crate::refund_flow::execute(
                    &state,
                    &payment_for_refund,
                    &merchant,
                    Some(application.attempt_id),
                    amount,
                    Some("duplicate"),
                    None,
                )
                .await
                {
                    Ok(result) if !result.succeeded => {
                        tracing::warn!(
                            payment_id = %application.payment_id,
                            "the automatic refund of a duplicate payment was refused by the provider; \
                             the reconciler will send it again"
                        );
                    }
                    Err(err) => {
                        tracing::warn!(
                            payment_id = %application.payment_id,
                            error = ?err,
                            "could not even attempt the automatic refund of a duplicate payment"
                        );
                    }
                    Ok(_) => {}
                }
            }
        }
    }

    let (status, ack_body) = ack_response(application.outcome, adapter.ack_body());
    text_response(status, ack_body)
}

/// The pure half of "answer `1|OK` only when the outcome earns it"
/// (`spec.md`, "Being told by the provider"): every outcome except
/// `amount_mismatch` is acknowledged. Split out from [`receive_callback`] so
/// this decision — the one this whole endpoint exists to get right — is
/// unit-testable without a database (`spec.md`, "Test plan" -> `api`: "`1|OK`
/// written only after the commit"; the "after the commit" half is structural
/// — this function is never reached until `apply_callback`'s own transaction
/// has already returned — and this is the other half, "only when it should
/// be").
fn ack_response(outcome: CallbackOutcome, ack_body: &'static str) -> (StatusCode, &'static str) {
    if repo::should_acknowledge(outcome) {
        (StatusCode::OK, ack_body)
    } else {
        (StatusCode::BAD_REQUEST, "0|AmountMismatch")
    }
}

async fn find_payment_id_for_attempt(
    state: &AppState,
    code: ProviderCode,
    provider_trade_no: &str,
) -> paygate_infra::Result<Option<uuid::Uuid>> {
    let code_str = match code {
        ProviderCode::Ecpay => "ecpay",
        ProviderCode::Newebpay => "newebpay",
    };
    let row: Option<(uuid::Uuid,)> = sqlx::query_as(
        "SELECT payment_id FROM payment_attempts WHERE provider_code = $1 AND provider_trade_no = $2",
    )
    .bind(code_str)
    .bind(provider_trade_no)
    .fetch_optional(&state.db.0)
    .await?;
    Ok(row.map(|(id,)| id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_outcome_but_amount_mismatch_is_acknowledged_with_the_adapters_own_body() {
        for outcome in [
            CallbackOutcome::Applied,
            CallbackOutcome::NoOp,
            CallbackOutcome::Conflict,
            CallbackOutcome::Duplicate,
            CallbackOutcome::UnknownCode,
        ] {
            let (status, body) = ack_response(outcome, "1|OK");
            assert_eq!(status, StatusCode::OK, "{outcome:?} should be acknowledged");
            assert_eq!(body, "1|OK");
        }
    }

    #[test]
    fn an_amount_mismatch_is_refused_on_purpose_so_the_provider_sends_it_again() {
        let (status, body) = ack_response(CallbackOutcome::AmountMismatch, "1|OK");
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_ne!(body, "1|OK");
    }

    #[test]
    fn only_the_two_known_provider_codes_route_anywhere() {
        assert_eq!(parse_provider_code("ecpay"), Some(ProviderCode::Ecpay));
        assert_eq!(
            parse_provider_code("newebpay"),
            Some(ProviderCode::Newebpay)
        );
        assert_eq!(parse_provider_code("ECPay"), None);
        assert_eq!(parse_provider_code("stripe"), None);
        assert_eq!(parse_provider_code(""), None);
    }
}
