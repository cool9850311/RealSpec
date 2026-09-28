//! Merchant API: create an order and its provider form, read it, refund it
//! (`openapi.yaml`, tag `Orders`).

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::Utc;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use paygate_domain::{idempotency_fingerprint, validate_create_order, PaymentStatus, ProviderCode};
use paygate_infra::repo;
use paygate_provider::{HandOffRequest, PlatformCredentials};

use crate::auth::MerchantAuth;
use crate::db;
use crate::dto::{extract_idempotency_key, parse_json_body, payment_json};
use crate::error::{ApiError, ApiResult};
use crate::idempotency::{self, Outcome};
use crate::state::AppState;

fn provider_code_str(code: ProviderCode) -> &'static str {
    match code {
        ProviderCode::Ecpay => "ecpay",
        ProviderCode::Newebpay => "newebpay",
    }
}

/// A stored idempotent response's status is replayed verbatim — but only
/// when it still parses as one. Falling back to `200 OK` here would turn a
/// replayed FAILURE (a cached `422`, say) into an apparent success, the one
/// fallback in this whole path that manufactures a lie out of corrupted
/// data; an idempotency record this session cannot trust is a `500`, loudly,
/// not a silent `200`.
fn status_from_u16(status: u16) -> Option<StatusCode> {
    StatusCode::from_u16(status).ok()
}

fn json_response(status: u16, body: Value, replayed: bool) -> Response {
    let Some(status_code) = status_from_u16(status) else {
        tracing::error!(
            status,
            "stored idempotent response has an unparseable HTTP status; refusing to replay it \
             as anything other than a failure"
        );
        return ApiError::Internal.into_response();
    };
    let mut response = (status_code, Json(body)).into_response();
    if replayed {
        response
            .headers_mut()
            .insert("Idempotent-Replayed", HeaderValue::from_static("true"));
    }
    response
}

async fn resolve_idempotency(
    state: &AppState,
    merchant_id: i64,
    key: &str,
    fingerprint: &str,
) -> ApiResult<Option<Response>> {
    match idempotency::begin(state, merchant_id, key, fingerprint)
        .await
        .map_err(ApiError::from)?
    {
        Outcome::Replay { status, body } => Ok(Some(json_response(status as u16, body, true))),
        Outcome::InUse => Err(ApiError::IdempotencyKeyInUse),
        Outcome::Reused => Err(ApiError::IdempotencyKeyReused),
        Outcome::Started => Ok(None),
    }
}

// ---------------------------------------------------------------------
// POST /payments
// ---------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateOrderBody {
    merchant_trade_no: String,
    amount: i64,
    currency: String,
    item_desc: String,
    notify_url: String,
    client_back_url: String,
}

pub async fn create_order(
    State(state): State<AppState>,
    auth: MerchantAuth,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Response> {
    let idem_key = extract_idempotency_key(&headers)?;
    let raw = parse_json_body(&body)?;
    let fingerprint = idempotency_fingerprint(auth.merchant_id, &raw);

    if let Some(replay) =
        resolve_idempotency(&state, auth.merchant_id, &idem_key, &fingerprint).await?
    {
        return Ok(replay);
    }

    match do_create_order(&state, auth.merchant_id, raw).await {
        Ok((status, body)) => {
            idempotency::complete(&state, auth.merchant_id, &idem_key, status, &body).await;
            Ok(json_response(status, body, false))
        }
        Err(err) => {
            idempotency::release(&state, auth.merchant_id, &idem_key).await;
            Err(err)
        }
    }
}

async fn do_create_order(
    state: &AppState,
    merchant_id: i64,
    raw: Value,
) -> ApiResult<(u16, Value)> {
    let parsed: CreateOrderBody =
        serde_json::from_value(raw).map_err(|_| ApiError::InvalidRequest)?;
    let domain_req = paygate_domain::CreateOrderRequest {
        merchant_trade_no: parsed.merchant_trade_no,
        amount: parsed.amount,
        currency: parsed.currency,
        item_desc: parsed.item_desc,
        notify_url: parsed.notify_url,
        client_back_url: parsed.client_back_url,
    };
    validate_create_order(&domain_req)
        .map_err(|e| ApiError::ValidationFailed { field: e.field })?;

    let merchant = db::fetch_merchant(&state.db, merchant_id)
        .await
        .map_err(ApiError::from)?
        .ok_or(ApiError::Internal)?;
    if merchant.currency != domain_req.currency {
        return Err(ApiError::CurrencyNotSupported);
    }

    let new_order = repo::NewOrder {
        merchant_id,
        merchant_trade_no: domain_req.merchant_trade_no.clone(),
        amount: domain_req.amount,
        currency: domain_req.currency.clone(),
        item_desc: domain_req.item_desc.clone(),
        notify_url: domain_req.notify_url.clone(),
        client_back_url: domain_req.client_back_url.clone(),
    };

    let payment_id = match repo::create_order(&state.db, new_order).await {
        Ok(row) => row.id,
        Err(paygate_infra::Error::MerchantTradeNoTaken) => {
            // `spec.md`, "Two kinds of duplicate" / `handoff.feature`, the
            // pinned rule: an existing order under this number is a fresh
            // form on the SAME order when the parameters still match and it
            // is still `pending`; anything else (settled already, or the
            // parameters differ) is the real conflict.
            let existing = db::find_payment_by_merchant_trade_no(
                &state.db,
                merchant_id,
                &domain_req.merchant_trade_no,
            )
            .await
            .map_err(ApiError::from)?
            .ok_or(ApiError::Internal)?;
            let same_params = existing.amount == domain_req.amount
                && existing.currency == domain_req.currency
                && existing.item_desc == domain_req.item_desc
                && existing.notify_url == domain_req.notify_url
                && existing.client_back_url == domain_req.client_back_url;
            if same_params && existing.status == PaymentStatus::Pending {
                existing.id
            } else {
                return Err(ApiError::MerchantTradeNoTaken);
            }
        }
        Err(e) => return Err(e.into()),
    };

    let opened = repo::open_attempt(
        &state.db,
        merchant_id,
        payment_id,
        merchant.provider_code,
        &state.config.provider_trade_no_prefix,
    )
    .await
    .map_err(ApiError::from)?;

    let provider_row = db::fetch_provider(&state.db, merchant.provider_code)
        .await
        .map_err(ApiError::from)?
        .ok_or(ApiError::Internal)?;
    let payment = db::find_payment_by_id_unscoped(&state.db, payment_id)
        .await
        .map_err(ApiError::from)?
        .ok_or(ApiError::Internal)?;

    let creds = PlatformCredentials {
        platform_id: provider_row.platform_id.clone(),
        hash_key: provider_row.hash_key.clone(),
        hash_iv: provider_row.hash_iv.clone(),
    };
    // `openapi.yaml`'s `/webhooks/{provider}`: `ReturnURL` is
    // `{PUBLIC_BASE_URL}/api/v1/webhooks/{code}` — `payment-provider.yaml`'s
    // `ClientBackURL` is why `OrderResultURL` is never registered instead.
    let return_url = format!(
        "{}/api/v1/webhooks/{}",
        state.config.public_base_url,
        provider_code_str(merchant.provider_code)
    );
    let hand_off_req = HandOffRequest {
        cashier_url: provider_row.cashier_url.clone(),
        provider_merchant_id: merchant.provider_merchant_id.clone(),
        provider_trade_no: opened.provider_trade_no.clone(),
        trade_date: Utc::now(),
        amount: payment.amount,
        item_desc: payment.item_desc.clone(),
        return_url,
        client_back_url: payment.client_back_url.clone(),
    };
    let adapter = paygate_provider::adapter(merchant.provider_code);
    let hand_off = adapter.hand_off(&creds, &hand_off_req).map_err(|err| {
        tracing::error!(error = %err, "signing the hand-off form failed");
        ApiError::Internal
    })?;

    let last_attempt = db::find_latest_attempt(&state.db, payment_id)
        .await
        .map_err(ApiError::from)?;
    let mut body = payment_json(&payment, last_attempt.as_ref());
    if let Value::Object(map) = &mut body {
        map.insert(
            "provider".to_string(),
            json!(provider_code_str(merchant.provider_code)),
        );
        map.insert("action".to_string(), json!(hand_off.action));
        map.insert("fields".to_string(), json!(hand_off.fields));
    }
    Ok((201, body))
}

// ---------------------------------------------------------------------
// GET /payments (find by merchant_trade_no) and GET /payments/{id}
// ---------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct FindOrderQuery {
    merchant_trade_no: String,
}

pub async fn find_order_by_trade_no(
    State(state): State<AppState>,
    auth: MerchantAuth,
    Query(query): Query<FindOrderQuery>,
) -> ApiResult<Json<Value>> {
    let payment = db::find_payment_by_merchant_trade_no(
        &state.db,
        auth.merchant_id,
        &query.merchant_trade_no,
    )
    .await
    .map_err(ApiError::from)?
    .ok_or(ApiError::PaymentNotFound)?;
    let last_attempt = db::find_latest_attempt(&state.db, payment.id)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(payment_json(&payment, last_attempt.as_ref())))
}

pub async fn get_order(
    State(state): State<AppState>,
    auth: MerchantAuth,
    Path(payment_id): Path<String>,
) -> ApiResult<Json<Value>> {
    let payment_id = Uuid::parse_str(&payment_id).map_err(|_| ApiError::PaymentNotFound)?;
    let payment = db::find_payment_by_id(&state.db, payment_id, auth.merchant_id)
        .await
        .map_err(ApiError::from)?
        .ok_or(ApiError::PaymentNotFound)?;
    let last_attempt = db::find_latest_attempt(&state.db, payment_id)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(payment_json(&payment, last_attempt.as_ref())))
}

// ---------------------------------------------------------------------
// POST /payments/{id}/refunds
// ---------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRefundBody {
    amount: Value,
    #[serde(default)]
    reason: Option<Value>,
}

/// `CreateRefundRequest.amount` is `integer, minimum: 1, maximum: 99999999`.
/// Unlike `CreateOrderRequest.amount` (deserialised straight into an `i64`
/// field, so a fraction or an unrepresentable literal is a `400` body-shape
/// failure — `orders.feature`), `refunds.feature`'s own "amount that breaks a
/// rule" scenario groups a fraction (`12.5`) with zero and a too-large value
/// under `422`, while its "malformed request" scenario keeps a wrong JSON
/// TYPE (a string) and an integer too large to exist in any numeric type at
/// `400`. This function keeps both files' scenarios true at once: a value
/// that is not a JSON number at all, or a whole number bcame Postgres itself
/// cannot hold, is a shape problem; a number that IS a number but is not a
/// positive integer in range is a field rule.
fn parse_refund_amount(value: &Value) -> ApiResult<i64> {
    let number = value.as_number().ok_or(ApiError::InvalidRequest)?;
    if let Some(i) = number.as_i64() {
        return Ok(i);
    }
    let f = number.as_f64().ok_or(ApiError::InvalidRequest)?;
    if f.fract() != 0.0 {
        // A number, but not a whole one. `refunds.feature` groups 12.5 with 0
        // and -500 under `422`, so this is a field rule rather than a shape
        // problem.
        return Err(ApiError::ValidationFailed { field: "amount" });
    }
    // Whole — and `-0` lands here rather than in `as_i64` above, because
    // serde_json keeps the sign and declines to call it an integer. It is a
    // perfectly representable amount whose only problem is that it is not
    // positive, which is exactly what the field rule is for. `refunds.feature`
    // spells it out: "Negative zero is still not a positive integer: the same
    // rule that refuses 0 must refuse the sign-carrying spelling of it too."
    if (i64::MIN as f64..=i64::MAX as f64).contains(&f) {
        return Ok(f as i64);
    }
    // Whole, but bigger than a minor-unit integer could be anywhere — that one
    // really is a shape problem (`refunds.feature`: "too large to be a
    // minor-unit integer anywhere can't be deserialised into one").
    Err(ApiError::InvalidRequest)
}

const REFUND_REASONS: &[&str] = &["requested_by_customer", "duplicate", "fraudulent"];

fn parse_refund_reason(value: Option<&Value>) -> ApiResult<Option<String>> {
    match value {
        None => Ok(None),
        Some(Value::String(s)) if REFUND_REASONS.contains(&s.as_str()) => Ok(Some(s.clone())),
        Some(Value::String(_)) => Err(ApiError::ValidationFailed { field: "reason" }),
        Some(_) => Err(ApiError::InvalidRequest),
    }
}

pub async fn create_refund(
    State(state): State<AppState>,
    auth: MerchantAuth,
    Path(payment_id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Response> {
    let idem_key = extract_idempotency_key(&headers)?;
    let raw = parse_json_body(&body)?;
    // The fingerprint folds the path in too (`repo::idempotency`'s own doc
    // comment; `idempotency.feature`: "a key spent on one order's refund
    // cannot be spent on another's").
    let fingerprint_input = json!({ "path": payment_id, "body": raw });
    let fingerprint = idempotency_fingerprint(auth.merchant_id, &fingerprint_input);

    if let Some(replay) =
        resolve_idempotency(&state, auth.merchant_id, &idem_key, &fingerprint).await?
    {
        return Ok(replay);
    }

    match do_create_refund(&state, auth.merchant_id, &payment_id, raw, &idem_key).await {
        Ok((status, body)) => {
            idempotency::complete(&state, auth.merchant_id, &idem_key, status, &body).await;
            Ok(json_response(status, body, false))
        }
        Err(err) => {
            idempotency::release(&state, auth.merchant_id, &idem_key).await;
            Err(err)
        }
    }
}

async fn do_create_refund(
    state: &AppState,
    merchant_id: i64,
    payment_id_raw: &str,
    raw: Value,
    idem_key: &str,
) -> ApiResult<(u16, Value)> {
    let payment_id = Uuid::parse_str(payment_id_raw).map_err(|_| ApiError::PaymentNotFound)?;
    let existing = db::find_payment_by_id(&state.db, payment_id, merchant_id)
        .await
        .map_err(ApiError::from)?
        .ok_or(ApiError::PaymentNotFound)?;

    let parsed: RawRefundBody =
        serde_json::from_value(raw).map_err(|_| ApiError::InvalidRequest)?;
    let amount = parse_refund_amount(&parsed.amount)?;
    let reason = parse_refund_reason(parsed.reason.as_ref())?;

    if existing.status != PaymentStatus::Succeeded {
        return Err(ApiError::PaymentNotRefundable);
    }

    let merchant = db::fetch_merchant(&state.db, merchant_id)
        .await
        .map_err(ApiError::from)?
        .ok_or(ApiError::Internal)?;

    let result = crate::refund_flow::execute(
        state,
        &existing,
        &merchant,
        None,
        amount,
        reason.as_deref(),
        Some(idem_key),
    )
    .await?;

    if result.succeeded {
        Ok((
            201,
            json!({
                "id": result.refund_id,
                "payment_id": payment_id,
                "amount": amount,
                "currency": existing.currency,
                "status": "succeeded",
                "reason": reason,
                "created_at": Utc::now().to_rfc3339(),
            }),
        ))
    } else {
        Err(ApiError::ProviderUnavailable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_valid_stored_status_replays_verbatim() {
        assert_eq!(status_from_u16(200), Some(StatusCode::OK));
        assert_eq!(status_from_u16(422), Some(StatusCode::UNPROCESSABLE_ENTITY));
    }

    #[test]
    fn a_corrupted_stored_status_does_not_parse_at_all() {
        // Bug: this used to fall back to `200 OK`, turning a replayed
        // FAILURE into an apparent success. `StatusCode::from_u16` refuses
        // anything outside 100..=999, so a corrupted value never silently
        // becomes `OK` — `json_response` is what turns this `None` into a
        // loud `500` instead.
        assert_eq!(status_from_u16(0), None);
        assert_eq!(status_from_u16(1000), None);
    }

    #[test]
    fn a_corrupted_stored_status_answers_500_internal_not_200_ok() {
        let response = json_response(0, json!({"error": "whatever was cached"}), true);
        assert_eq!(
            response.status(),
            StatusCode::INTERNAL_SERVER_ERROR,
            "an idempotency record this session cannot trust must never be replayed as a success"
        );
    }

    #[test]
    fn a_positive_integer_amount_is_accepted() {
        assert_eq!(parse_refund_amount(&json!(1000)).unwrap(), 1000);
        assert_eq!(parse_refund_amount(&json!(1)).unwrap(), 1);
    }

    #[test]
    fn zero_and_negative_amounts_are_shape_valid_integers_left_to_check_refund() {
        // This function's only job is "is this a JSON integer at all" —
        // `repo::create_refund` (`paygate_domain::check_refund`) is what
        // actually refuses zero and negative amounts as a business rule
        // (`refunds.feature`'s own `VALIDATION_FAILED, field: amount` for
        // these comes from that downstream check, not from here).
        assert_eq!(parse_refund_amount(&json!(0)).unwrap(), 0);
        assert_eq!(parse_refund_amount(&json!(-500)).unwrap(), -500);
        assert_eq!(parse_refund_amount(&json!(-0)).unwrap(), 0);
    }

    #[test]
    fn a_fraction_is_a_field_rule_violation_refunds_feature_expects_422_for() {
        // `-0` is a whole, representable amount that simply is not positive, so
        // it must reach the field rule as 0 rather than be refused as malformed.
        // serde_json keeps the sign, so `as_i64` declines it and the float path
        // has to recognise it (`refunds.feature`, "Negative zero is still not a
        // positive integer").
        assert_eq!(parse_refund_amount(&json!(-0.0)).unwrap(), 0);
        assert_eq!(
            parse_refund_amount(&serde_json::from_str::<Value>("-0").unwrap()).unwrap(),
            0
        );

        let err = parse_refund_amount(&json!(12.5)).unwrap_err();
        assert!(matches!(
            err,
            ApiError::ValidationFailed { field: "amount" }
        ));
    }

    #[test]
    fn a_whole_number_too_large_to_be_an_i64_is_a_shape_problem_not_a_field_rule() {
        // `refunds.feature`: "too large to be a minor-unit integer anywhere
        // can't be deserialised into one" -> `400 INVALID_REQUEST`, unlike the
        // fraction case above.
        let huge = serde_json::from_str::<Value>("99999999999999999999999999").unwrap();
        let err = parse_refund_amount(&huge).unwrap_err();
        assert!(matches!(err, ApiError::InvalidRequest));
    }

    #[test]
    fn a_string_or_other_non_number_amount_is_a_shape_problem() {
        assert!(matches!(
            parse_refund_amount(&json!("1000")).unwrap_err(),
            ApiError::InvalidRequest
        ));
        assert!(matches!(
            parse_refund_amount(&json!(null)).unwrap_err(),
            ApiError::InvalidRequest
        ));
        assert!(matches!(
            parse_refund_amount(&json!([1000])).unwrap_err(),
            ApiError::InvalidRequest
        ));
    }

    #[test]
    fn an_absent_reason_is_fine_and_a_known_one_round_trips() {
        assert_eq!(parse_refund_reason(None).unwrap(), None);
        assert_eq!(
            parse_refund_reason(Some(&json!("duplicate"))).unwrap(),
            Some("duplicate".to_string())
        );
        assert_eq!(
            parse_refund_reason(Some(&json!("requested_by_customer"))).unwrap(),
            Some("requested_by_customer".to_string())
        );
        assert_eq!(
            parse_refund_reason(Some(&json!("fraudulent"))).unwrap(),
            Some("fraudulent".to_string())
        );
    }

    #[test]
    fn a_reason_outside_the_closed_set_is_a_field_rule_violation() {
        let err = parse_refund_reason(Some(&json!("because"))).unwrap_err();
        assert!(matches!(
            err,
            ApiError::ValidationFailed { field: "reason" }
        ));
    }

    #[test]
    fn a_non_string_reason_is_a_shape_problem() {
        let err = parse_refund_reason(Some(&json!(42))).unwrap_err();
        assert!(matches!(err, ApiError::InvalidRequest));
    }
}
