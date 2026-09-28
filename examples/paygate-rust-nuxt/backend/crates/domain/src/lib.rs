//! `paygate-domain` — the pure core: the order state machine, refund
//! arithmetic, the provider trade-number generator, and request validation.
//! No I/O, no async, no `sqlx`, no HTTP. Everything here is a function of its
//! arguments, which is what makes every rule in it testable without a
//! database, a container, or a clock — see `spec.md`, "Test plan" →
//! "Unit tests, by crate" → `domain`.

mod enums;
mod error;
mod fingerprint;
mod ids;
mod refund;
mod state;
mod trade_no;
mod validation;

pub use enums::{
    AttemptStatus, CallbackOutcome, EventType, PaymentStatus, ProviderCode, RefundStatus,
};
pub use error::{DomainError, ValidationError};
pub use fingerprint::idempotency_fingerprint;
pub use ids::{Amount, MerchantId};
pub use refund::{check_refund, refundable};
pub use state::{apply, Command, PaymentState, Transition};
pub use trade_no::{is_valid_provider_trade_no, provider_trade_no, validate_trade_no_prefix};
pub use validation::{validate_create_order, CreateOrderRequest, MAX_AMOUNT, MIN_AMOUNT};
