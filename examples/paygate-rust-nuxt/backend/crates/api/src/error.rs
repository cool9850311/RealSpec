//! `paygate-api`'s own error type: everything a handler can fail with, mapped
//! to exactly the `{"error": "CODE", ...}` shapes `spec/openapi/openapi.yaml`
//! documents. One enum, one `IntoResponse` impl, so every handler answers in
//! the same shape without repeating the mapping (`openapi.yaml`'s "Errors
//! and common headers": "every error body is a flat object with a stable
//! `error` code" — the CODE strings themselves are in the OpenAPI file and
//! the features; copy them exactly).

use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};

#[derive(Debug, Clone)]
pub enum ApiError {
    InvalidRequest,
    IdempotencyKeyMissing,
    IdempotencyKeyInvalid,
    IdempotencyKeyInUse,
    IdempotencyKeyReused,
    MerchantTradeNoTaken,
    Unauthenticated,
    InvalidCredentials,
    PaymentNotFound,
    ApiKeyNotFound,
    CurrencyNotSupported,
    PaymentNotRefundable,
    RefundExceedsRemaining { remaining: i64 },
    RangeTooLarge,
    RateLimited { retry_after_secs: u64 },
    ProviderUnavailable,
    SessionStoreUnavailable,
    ReportingUnavailable,
    Internal,
    ValidationFailed { field: &'static str },
}

impl ApiError {
    fn parts(&self) -> (StatusCode, &'static str, Value) {
        match self {
            ApiError::InvalidRequest => (StatusCode::BAD_REQUEST, "INVALID_REQUEST", Value::Null),
            ApiError::IdempotencyKeyMissing => (
                StatusCode::BAD_REQUEST,
                "IDEMPOTENCY_KEY_MISSING",
                Value::Null,
            ),
            ApiError::IdempotencyKeyInvalid => (
                StatusCode::BAD_REQUEST,
                "IDEMPOTENCY_KEY_INVALID",
                Value::Null,
            ),
            ApiError::IdempotencyKeyInUse => {
                (StatusCode::CONFLICT, "IDEMPOTENCY_KEY_IN_USE", Value::Null)
            }
            ApiError::IdempotencyKeyReused => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "IDEMPOTENCY_KEY_REUSED",
                Value::Null,
            ),
            ApiError::MerchantTradeNoTaken => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "MERCHANT_TRADE_NO_TAKEN",
                Value::Null,
            ),
            ApiError::Unauthenticated => (StatusCode::UNAUTHORIZED, "UNAUTHENTICATED", Value::Null),
            ApiError::InvalidCredentials => {
                (StatusCode::UNAUTHORIZED, "INVALID_CREDENTIALS", Value::Null)
            }
            ApiError::PaymentNotFound => (StatusCode::NOT_FOUND, "PAYMENT_NOT_FOUND", Value::Null),
            ApiError::ApiKeyNotFound => (StatusCode::NOT_FOUND, "API_KEY_NOT_FOUND", Value::Null),
            ApiError::CurrencyNotSupported => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "CURRENCY_NOT_SUPPORTED",
                Value::Null,
            ),
            ApiError::PaymentNotRefundable => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "PAYMENT_NOT_REFUNDABLE",
                Value::Null,
            ),
            ApiError::RefundExceedsRemaining { remaining } => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "REFUND_EXCEEDS_REMAINING",
                json!({ "remaining": remaining }),
            ),
            ApiError::RangeTooLarge => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "RANGE_TOO_LARGE",
                Value::Null,
            ),
            ApiError::RateLimited { .. } => {
                (StatusCode::TOO_MANY_REQUESTS, "RATE_LIMITED", Value::Null)
            }
            ApiError::ProviderUnavailable => {
                (StatusCode::BAD_GATEWAY, "PROVIDER_UNAVAILABLE", Value::Null)
            }
            ApiError::SessionStoreUnavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "SESSION_STORE_UNAVAILABLE",
                Value::Null,
            ),
            ApiError::ReportingUnavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "REPORTING_UNAVAILABLE",
                Value::Null,
            ),
            ApiError::Internal => (StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL", Value::Null),
            ApiError::ValidationFailed { field } => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "VALIDATION_FAILED",
                json!({ "field": field }),
            ),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code, extra) = self.parts();
        let mut body = json!({ "error": code });
        if let (Value::Object(base), Value::Object(more)) = (&mut body, extra) {
            base.extend(more);
        }
        let mut response = (status, Json(body)).into_response();
        if let ApiError::RateLimited { retry_after_secs } = &self {
            if let Ok(value) = HeaderValue::from_str(&retry_after_secs.to_string()) {
                response.headers_mut().insert("Retry-After", value);
            }
        }
        if matches!(self, ApiError::Unauthenticated) {
            // `spec.md`, "Authentication" / `dashboard_sessions.feature`:
            // `session_hint` "grants nothing, and every 401 expires it" —
            // whichever credential a caller presented (or failed to), the
            // front end's own "am I signed in" hint goes stale with it.
            if let Ok(value) =
                HeaderValue::from_str("session_hint=; Path=/; SameSite=Lax; Max-Age=0")
            {
                response.headers_mut().append("Set-Cookie", value);
            }
        }
        response
    }
}

/// Maps every I/O-touching outcome `paygate-infra` can hand back. Only the
/// variants a handler has not already turned into something more specific
/// (a payment lookup that already became `PaymentNotFound`, say) fall
/// through here, which is why database and provider-adapter failures default
/// to `INTERNAL`: they are programmer- or infrastructure-level surprises on
/// a path that already checked what it could check.
impl From<paygate_infra::Error> for ApiError {
    fn from(err: paygate_infra::Error) -> Self {
        use paygate_infra::Error;
        match err {
            Error::MerchantTradeNoTaken => ApiError::MerchantTradeNoTaken,
            Error::PaymentNotFound => ApiError::PaymentNotFound,
            Error::PaymentNotRefundable => ApiError::PaymentNotRefundable,
            Error::PaymentNotPending => ApiError::MerchantTradeNoTaken,
            Error::RefundExceedsRemaining { remaining, .. } => {
                ApiError::RefundExceedsRemaining { remaining }
            }
            Error::IdempotencyKeyInUse => ApiError::IdempotencyKeyInUse,
            Error::IdempotencyKeyReused => ApiError::IdempotencyKeyReused,
            Error::ClickHouseUnavailable(_) => ApiError::ReportingUnavailable,
            Error::ProviderTimedOut | Error::ProviderUnavailable(_) => {
                ApiError::ProviderUnavailable
            }
            Error::Validation(e) => ApiError::ValidationFailed { field: e.field },
            Error::Database(e) => {
                tracing::error!(error = %e, "database error");
                ApiError::Internal
            }
            // `repo::create_refund` maps `check_refund`'s own
            // `InvalidRefundAmount` (a non-positive amount) through here
            // rather than through its own `Error` variant — `refunds.feature`:
            // a zero, negative or negative-zero refund amount is
            // `422 VALIDATION_FAILED, field: amount`, the same code a
            // shape-level range check would give.
            Error::Domain(paygate_domain::DomainError::InvalidRefundAmount) => {
                ApiError::ValidationFailed { field: "amount" }
            }
            Error::Domain(e) => {
                tracing::error!(error = %e, "domain error surfaced to the api boundary");
                ApiError::Internal
            }
            Error::Provider(e) => {
                tracing::error!(error = %e, "provider adapter error surfaced to the api boundary");
                ApiError::Internal
            }
            Error::RedisUnavailable => {
                tracing::error!("redis unavailable error reached the api boundary unhandled");
                ApiError::Internal
            }
        }
    }
}

pub type ApiResult<T> = Result<T, ApiError>;

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    async fn body_json(response: Response) -> Value {
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn every_error_body_is_the_flat_error_code_shape_openapi_documents() {
        let cases: &[(ApiError, StatusCode, &str)] = &[
            (
                ApiError::InvalidRequest,
                StatusCode::BAD_REQUEST,
                "INVALID_REQUEST",
            ),
            (
                ApiError::IdempotencyKeyMissing,
                StatusCode::BAD_REQUEST,
                "IDEMPOTENCY_KEY_MISSING",
            ),
            (
                ApiError::IdempotencyKeyInUse,
                StatusCode::CONFLICT,
                "IDEMPOTENCY_KEY_IN_USE",
            ),
            (
                ApiError::MerchantTradeNoTaken,
                StatusCode::UNPROCESSABLE_ENTITY,
                "MERCHANT_TRADE_NO_TAKEN",
            ),
            (
                ApiError::Unauthenticated,
                StatusCode::UNAUTHORIZED,
                "UNAUTHENTICATED",
            ),
            (
                ApiError::PaymentNotFound,
                StatusCode::NOT_FOUND,
                "PAYMENT_NOT_FOUND",
            ),
            (
                ApiError::ProviderUnavailable,
                StatusCode::BAD_GATEWAY,
                "PROVIDER_UNAVAILABLE",
            ),
            (
                ApiError::SessionStoreUnavailable,
                StatusCode::SERVICE_UNAVAILABLE,
                "SESSION_STORE_UNAVAILABLE",
            ),
            (
                ApiError::Internal,
                StatusCode::INTERNAL_SERVER_ERROR,
                "INTERNAL",
            ),
        ];
        for (err, expected_status, expected_code) in cases.iter().cloned() {
            let response = err.into_response();
            assert_eq!(response.status(), expected_status);
            let body = body_json(response).await;
            assert_eq!(body["error"], expected_code);
        }
    }

    #[tokio::test]
    async fn validation_failed_carries_the_offending_field() {
        let response = ApiError::ValidationFailed { field: "amount" }.into_response();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let body = body_json(response).await;
        assert_eq!(body["error"], "VALIDATION_FAILED");
        assert_eq!(body["field"], "amount");
    }

    #[tokio::test]
    async fn refund_exceeds_remaining_carries_the_remaining_amount() {
        let response = ApiError::RefundExceedsRemaining { remaining: 600 }.into_response();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let body = body_json(response).await;
        assert_eq!(body["error"], "REFUND_EXCEEDS_REMAINING");
        assert_eq!(body["remaining"], 600);
    }

    #[test]
    fn rate_limited_sets_the_retry_after_header() {
        let response = ApiError::RateLimited {
            retry_after_secs: 20,
        }
        .into_response();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(response.headers().get("Retry-After").unwrap(), "20");
    }

    #[test]
    fn unauthenticated_always_expires_the_session_hint_cookie() {
        let response = ApiError::Unauthenticated.into_response();
        let cookie = response
            .headers()
            .get("Set-Cookie")
            .and_then(|v| v.to_str().ok())
            .unwrap();
        assert!(cookie.starts_with("session_hint=;"));
        assert!(cookie.contains("Max-Age=0"));
    }

    #[test]
    fn a_non_unauthenticated_error_never_touches_the_session_hint_cookie() {
        let response = ApiError::PaymentNotFound.into_response();
        assert!(response.headers().get("Set-Cookie").is_none());
    }

    #[test]
    fn a_non_positive_refund_amount_from_check_refund_becomes_validation_failed() {
        // `repo::create_refund` reports a zero/negative amount as
        // `Error::Domain(DomainError::InvalidRefundAmount)`, not its own
        // dedicated variant — this is the mapping `refunds.feature`'s
        // zero/negative/negative-zero scenarios depend on (`422
        // VALIDATION_FAILED, field: amount`, not a `500`).
        let err: ApiError =
            paygate_infra::Error::Domain(paygate_domain::DomainError::InvalidRefundAmount).into();
        assert!(matches!(
            err,
            ApiError::ValidationFailed { field: "amount" }
        ));
    }

    #[test]
    fn a_refund_exceeding_the_remaining_balance_carries_the_remaining_figure() {
        let err: ApiError = paygate_infra::Error::RefundExceedsRemaining {
            requested: 601,
            remaining: 600,
        }
        .into();
        assert!(matches!(
            err,
            ApiError::RefundExceedsRemaining { remaining: 600 }
        ));
    }
}
