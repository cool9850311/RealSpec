//! The error type every I/O-touching function in this crate returns.
//! Deliberately one enum for the whole crate rather than one per module: the
//! `api` crate matches on it to answer paygate's own error bodies
//! (`{"error": "CODE", "message": "..."}`), and one place to look is more
//! useful to that caller than five.

use paygate_domain::{Amount, DomainError, ValidationError};
use paygate_provider::ProviderError;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error(transparent)]
    Domain(#[from] DomainError),

    #[error(transparent)]
    Validation(#[from] ValidationError),

    #[error(transparent)]
    Provider(#[from] ProviderError),

    #[error("merchant_trade_no already used by this merchant")]
    MerchantTradeNoTaken,

    #[error("no such payment for this merchant")]
    PaymentNotFound,

    #[error("payment is not refundable")]
    PaymentNotRefundable,

    #[error("payment is not pending, so no attempt can be opened against it")]
    PaymentNotPending,

    #[error("refund of {requested} exceeds the {remaining} remaining")]
    RefundExceedsRemaining {
        requested: Amount,
        remaining: Amount,
    },

    #[error("idempotency key is already being processed")]
    IdempotencyKeyInUse,

    #[error("idempotency key was used for a different request")]
    IdempotencyKeyReused,

    #[error("redis is unavailable")]
    RedisUnavailable,

    #[error("clickhouse is unavailable: {0}")]
    ClickHouseUnavailable(String),

    #[error("provider call timed out")]
    ProviderTimedOut,

    #[error("provider is unavailable: {0}")]
    ProviderUnavailable(String),
}

pub type Result<T> = std::result::Result<T, Error>;
