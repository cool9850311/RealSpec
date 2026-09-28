//! The errors this crate can produce. Nothing here touches I/O; every
//! variant is something a caller can construct a response from directly.

use crate::Amount;

/// A rule of the domain was broken. Distinct from [`ValidationError`], which
/// is about the *shape* of a request; this is about what the state machine
/// or the arithmetic refuses to do.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DomainError {
    /// `validate_trade_no_prefix` refused the configured prefix.
    #[error("invalid provider trade-number prefix {0:?}: must be [A-Za-z0-9]+ and len(prefix) + 15 <= 20")]
    InvalidTradeNoPrefix(String),

    /// A callback or reconciliation answer reported an amount that is not
    /// the order's. Refused outright, in every payment state — see
    /// spec.md, "Being told by the provider".
    #[error("amount does not match the order")]
    AmountMismatch,

    /// A refund was attempted against a payment that has never succeeded
    /// (still `pending`) or has nothing left to give back (`refunded`).
    #[error("payment is not refundable")]
    NotRefundable,

    /// `check_refund` was asked to check a non-positive amount.
    #[error("refund amount must be a positive integer")]
    InvalidRefundAmount,

    /// `check_refund` was asked to refund more than remains on the charge.
    #[error("refund of {requested} exceeds the {remaining} remaining")]
    RefundExceedsRemaining {
        requested: Amount,
        remaining: Amount,
    },
}

/// A request field broke a rule of its own shape (`spec/openapi/openapi.yaml`
/// patterns and bounds), independent of any other row. Carries the field
/// name so an API layer can answer `{"error":"VALIDATION_FAILED","field":..}`
/// verbatim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationError {
    pub field: &'static str,
    pub message: String,
}

impl ValidationError {
    pub fn new(field: &'static str, message: impl Into<String>) -> Self {
        Self {
            field,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.field, self.message)
    }
}

impl std::error::Error for ValidationError {}
