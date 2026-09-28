//! The shop-facing error responses. `demo-merchant.yaml` pins one body
//! exactly — `{"error":"GATEWAY_UNAVAILABLE"}` on a `502` — and leaves the
//! `404`/`409` cases with no declared content, so this still answers those
//! with a small JSON body in the same shape, which is compatible with an
//! unspecified schema without contradicting the one response that IS pinned.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;

#[derive(Debug)]
pub enum AppError {
    /// `POST /orders`: the amount did not match
    /// `^[0-9]+(\.[0-9]{1,2})?$`. Undocumented in `demo-merchant.yaml` (only
    /// `201`/`502` are listed there), but a malformed request is not a
    /// gateway failure, so it is answered as the bad request it is rather
    /// than folded into `502`.
    InvalidRequest,
    /// No order exists for that `merchantTradeNo`.
    NotFound,
    /// `GET /orders/{no}/form` on an order that is no longer
    /// `awaiting_payment`: "A paid order has no form."
    AlreadySettled,
    /// paygate could not be reached, or refused the call.
    GatewayUnavailable,
}

#[derive(Serialize)]
struct ErrorBody {
    error: &'static str,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, code) = match self {
            AppError::InvalidRequest => (StatusCode::BAD_REQUEST, "INVALID_REQUEST"),
            AppError::NotFound => (StatusCode::NOT_FOUND, "ORDER_NOT_FOUND"),
            AppError::AlreadySettled => (StatusCode::CONFLICT, "ORDER_ALREADY_SETTLED"),
            AppError::GatewayUnavailable => (StatusCode::BAD_GATEWAY, "GATEWAY_UNAVAILABLE"),
        };
        (status, Json(ErrorBody { error: code })).into_response()
    }
}
