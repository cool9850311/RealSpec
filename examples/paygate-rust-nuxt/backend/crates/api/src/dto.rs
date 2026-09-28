//! Small, shared request/response shaping: the `Idempotency-Key` header rule
//! every mutating endpoint applies identically, the raw-JSON-first parsing
//! `POST /payments` and `POST /payments/{id}/refunds` both need (so an
//! idempotency fingerprint can be taken before the body is judged well- or
//! ill-formed), and the `Payment` JSON shape both order endpoints answer
//! with.

use axum::http::HeaderMap;
use paygate_domain::ProviderCode;
use serde_json::{json, Value};

use crate::db::{AttemptSummary, PaymentRow};
use crate::error::ApiError;

const MAX_IDEMPOTENCY_KEY_LEN: usize = 255;

/// `openapi.yaml`'s `IdempotencyKey` parameter: required, 1 to 255 characters.
/// Absent is one mistake (`400 IDEMPOTENCY_KEY_MISSING`); present but empty,
/// blank or oversized is a different one (`400 IDEMPOTENCY_KEY_INVALID`).
pub fn extract_idempotency_key(headers: &HeaderMap) -> Result<String, ApiError> {
    let raw = headers
        .get("Idempotency-Key")
        .ok_or(ApiError::IdempotencyKeyMissing)?;
    let value = raw.to_str().map_err(|_| ApiError::IdempotencyKeyInvalid)?;
    if value.is_empty() || value.chars().count() > MAX_IDEMPOTENCY_KEY_LEN {
        return Err(ApiError::IdempotencyKeyInvalid);
    }
    Ok(value.to_string())
}

/// The first, and only syntactic, step of reading a JSON body: this is not
/// yet the "body shape" check `openapi.yaml`'s order-of-checks names (that
/// happens once the idempotency key has already been looked up, against the
/// raw value this function hands back) — it only rules out a body that is
/// not JSON at all.
pub fn parse_json_body(bytes: &[u8]) -> Result<Value, ApiError> {
    if bytes.is_empty() {
        return Ok(Value::Object(Default::default()));
    }
    serde_json::from_slice(bytes).map_err(|_| ApiError::InvalidRequest)
}

fn provider_code_str(code: ProviderCode) -> &'static str {
    match code {
        ProviderCode::Ecpay => "ecpay",
        ProviderCode::Newebpay => "newebpay",
    }
}

/// `#/components/schemas/Payment`, shared by `GET /payments/{id}`,
/// `GET /payments`, and the `CreatedOrder` response (`allOf` `Payment` plus
/// `ProviderHandOff`) `POST /payments` answers with.
pub fn payment_json(payment: &PaymentRow, last_attempt: Option<&AttemptSummary>) -> Value {
    let last_attempt_json = match last_attempt {
        None => Value::Null,
        Some(a) => json!({
            "provider": provider_code_str(a.provider_code),
            "status": status_str(a.status),
            "failure_code": a.failure_code,
        }),
    };
    // `payments` itself has no `failure_code` column — a failed attempt's
    // reason lives on the attempt (`spec.md`, "Events": a decline is a fact
    // about the ATTEMPT, not the order). The top-level field `openapi.yaml`'s
    // `Payment` schema declares mirrors whatever the most recent attempt
    // says, which is `null` once an order has settled or a later attempt
    // supersedes a decline.
    let failure_code = last_attempt.and_then(|a| a.failure_code.clone());
    json!({
        "id": payment.id,
        "merchant_trade_no": payment.merchant_trade_no,
        "status": payment_status_str(payment.status),
        "amount": payment.amount,
        "currency": payment.currency,
        "item_desc": payment.item_desc,
        "amount_refunded": payment.amount_refunded,
        "card_brand": payment.card_brand,
        "card_last4": payment.card_last4,
        "failure_code": failure_code,
        "client_back_url": payment.client_back_url,
        "last_attempt": last_attempt_json,
        "created_at": payment.created_at.to_rfc3339(),
    })
}

fn payment_status_str(status: paygate_domain::PaymentStatus) -> &'static str {
    use paygate_domain::PaymentStatus::*;
    match status {
        Pending => "pending",
        Succeeded => "succeeded",
        Refunded => "refunded",
    }
}

fn status_str(status: paygate_domain::AttemptStatus) -> &'static str {
    use paygate_domain::AttemptStatus::*;
    match status {
        Redirected => "redirected",
        Succeeded => "succeeded",
        Failed => "failed",
        Abandoned => "abandoned",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;
    use paygate_domain::{AttemptStatus, PaymentStatus};
    use uuid::Uuid;

    fn headers_with(key_value: Option<&str>) -> HeaderMap {
        let mut headers = HeaderMap::new();
        if let Some(v) = key_value {
            headers.insert("Idempotency-Key", HeaderValue::from_str(v).unwrap());
        }
        headers
    }

    #[test]
    fn a_missing_idempotency_key_is_400_missing() {
        let err = extract_idempotency_key(&headers_with(None)).unwrap_err();
        assert!(matches!(err, ApiError::IdempotencyKeyMissing));
    }

    #[test]
    fn an_empty_idempotency_key_is_400_invalid_not_missing() {
        let err = extract_idempotency_key(&headers_with(Some(""))).unwrap_err();
        assert!(matches!(err, ApiError::IdempotencyKeyInvalid));
    }

    #[test]
    fn a_key_at_the_255_boundary_is_accepted_and_256_is_not() {
        let ok = "x".repeat(255);
        assert_eq!(
            extract_idempotency_key(&headers_with(Some(&ok))).unwrap(),
            ok
        );
        let too_long = "x".repeat(256);
        let err = extract_idempotency_key(&headers_with(Some(&too_long))).unwrap_err();
        assert!(matches!(err, ApiError::IdempotencyKeyInvalid));
    }

    #[test]
    fn an_ordinary_key_round_trips() {
        let key = extract_idempotency_key(&headers_with(Some("create-1001"))).unwrap();
        assert_eq!(key, "create-1001");
    }

    #[test]
    fn an_empty_body_parses_as_an_empty_object() {
        let value = parse_json_body(b"").unwrap();
        assert_eq!(value, json!({}));
    }

    #[test]
    fn a_syntactically_malformed_body_is_invalid_request() {
        let err = parse_json_body(b"{not json}").unwrap_err();
        assert!(matches!(err, ApiError::InvalidRequest));
    }

    #[test]
    fn a_well_formed_body_parses_to_its_own_value() {
        let value = parse_json_body(br#"{"amount": 1000}"#).unwrap();
        assert_eq!(value, json!({ "amount": 1000 }));
    }

    fn sample_payment() -> PaymentRow {
        PaymentRow {
            id: Uuid::nil(),
            merchant_id: 1,
            merchant_trade_no: "ACME-1".to_string(),
            amount: 1250,
            currency: "USD".to_string(),
            status: PaymentStatus::Pending,
            item_desc: "Beans".to_string(),
            card_brand: None,
            card_last4: None,
            notify_url: "/demo-merchant/api/notify".to_string(),
            client_back_url: "/shop/result".to_string(),
            amount_refunded: 0,
            created_at: chrono::DateTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn a_payment_with_no_attempt_yet_has_a_null_last_attempt_and_failure_code() {
        let body = payment_json(&sample_payment(), None);
        assert_eq!(body["status"], "pending");
        assert_eq!(body["last_attempt"], Value::Null);
        assert_eq!(body["failure_code"], Value::Null);
    }

    #[test]
    fn a_failed_attempt_surfaces_its_own_failure_code_at_the_top_level_too() {
        let attempt = AttemptSummary {
            provider_code: ProviderCode::Ecpay,
            status: AttemptStatus::Failed,
            failure_code: Some("card_declined".to_string()),
        };
        let body = payment_json(&sample_payment(), Some(&attempt));
        assert_eq!(body["last_attempt"]["provider"], "ecpay");
        assert_eq!(body["last_attempt"]["status"], "failed");
        assert_eq!(body["last_attempt"]["failure_code"], "card_declined");
        assert_eq!(body["failure_code"], "card_declined");
    }
}
