//! The order state machine. Three states only — `pending`, `succeeded`,
//! `refunded` — because paygate cannot know whether a customer is mid-payment
//! or has closed the tab; that fact lives on the ATTEMPT, not the order
//! (spec.md, "Concurrency" and the state diagram under "Events").
//!
//! `apply` only ever looks at the **payment's** status, amount and
//! amount-refunded — never at which attempt a command came from. That is a
//! deliberate scope: whether a successful [`Command::Settle`] against an
//! already-`succeeded` payment is an idempotent replay of the SAME attempt
//! (`provider_events.outcome = 'no_op'`) or money that genuinely moved a
//! SECOND time on a DIFFERENT attempt (`outcome = 'duplicate'`,
//! `PaymentDuplicatePaid`) is a distinction this module cannot make with the
//! information it is given — it depends on which attempt row the caller is
//! holding, which is an `infra`-level fact. `apply` reports `Transition::NoOp`
//! either way ("the order's status does not change"), and the caller is the
//! one place with enough context (the specific `payment_attempts` row locked
//! `FOR UPDATE`) to tell the two apart and choose the right
//! [`crate::CallbackOutcome`]. The same split applies to `FailAttempt`
//! against an already-`succeeded` payment: `NoOp` here, `'conflict'` there.

use crate::enums::{EventType, PaymentStatus};
use crate::error::DomainError;
use crate::ids::Amount;
use crate::refund::check_refund;

/// Everything `apply` needs to know about a payment to decide a command.
/// Deliberately not the whole `payments` row — just the three columns the
/// state machine and the refund arithmetic read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaymentState {
    pub status: PaymentStatus,
    pub amount: Amount,
    pub amount_refunded: Amount,
}

/// A fact paygate has learned that might change a payment's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// The provider says this attempt was paid, for `amount`. Amount is part
    /// of the command, not assumed from the payment, because "the callback's
    /// amount matches the order" is exactly the check `apply` makes before
    /// anything else — an amount that is not the order's is refused outright,
    /// in every state, never reconciled away (spec.md, "Being told by the
    /// provider").
    Settle { amount: Amount },
    /// The provider says this attempt was declined, or 3-D Secure was not
    /// completed.
    FailAttempt,
    /// The provider says the consumer never completed the attempt
    /// (`TradeStatus = 10200095`). Only the provider's own word may end an
    /// attempt this way — never a local timer.
    AbandonAttempt,
    /// The merchant asked to give back `amount` of what was charged.
    Refund { amount: Amount },
}

/// What applying a [`Command`] did. `NoOp` is not an error — it is the
/// correct answer for a command that has nothing left to do, such as a
/// callback settling an order for the second time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transition {
    Applied(EventType),
    NoOp,
}

/// Decide what a [`Command`] does to a payment. Total over every
/// `(PaymentStatus, Command)` pair: every legal transition below returns
/// `Ok`, and the only `Err`s are the two rules that hold regardless of
/// state — an amount that is not the order's, and a refund that breaks the
/// charge/remaining arithmetic or is asked of a payment that was never paid.
pub fn apply(payment: &PaymentState, cmd: &Command) -> Result<Transition, DomainError> {
    match cmd {
        Command::Settle { amount } => {
            if *amount != payment.amount {
                return Err(DomainError::AmountMismatch);
            }
            match payment.status {
                PaymentStatus::Pending => Ok(Transition::Applied(EventType::PaymentSucceeded)),
                PaymentStatus::Succeeded | PaymentStatus::Refunded => Ok(Transition::NoOp),
            }
        }
        Command::FailAttempt => match payment.status {
            PaymentStatus::Pending => Ok(Transition::Applied(EventType::PaymentAttemptFailed)),
            PaymentStatus::Succeeded | PaymentStatus::Refunded => Ok(Transition::NoOp),
        },
        Command::AbandonAttempt => match payment.status {
            PaymentStatus::Pending => Ok(Transition::Applied(EventType::PaymentAttemptAbandoned)),
            PaymentStatus::Succeeded | PaymentStatus::Refunded => Ok(Transition::NoOp),
        },
        Command::Refund { amount } => {
            if payment.status != PaymentStatus::Succeeded {
                return Err(DomainError::NotRefundable);
            }
            check_refund(payment.amount, payment.amount_refunded, *amount)?;
            Ok(Transition::Applied(EventType::PaymentRefunded))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending(amount: Amount) -> PaymentState {
        PaymentState {
            status: PaymentStatus::Pending,
            amount,
            amount_refunded: 0,
        }
    }

    fn succeeded(amount: Amount, refunded: Amount) -> PaymentState {
        PaymentState {
            status: PaymentStatus::Succeeded,
            amount,
            amount_refunded: refunded,
        }
    }

    fn refunded(amount: Amount) -> PaymentState {
        PaymentState {
            status: PaymentStatus::Refunded,
            amount,
            amount_refunded: amount,
        }
    }

    // ---- legal transitions ------------------------------------------------

    #[test]
    fn pending_settle_with_matching_amount_succeeds_the_payment() {
        let p = pending(1000);
        assert_eq!(
            apply(&p, &Command::Settle { amount: 1000 }),
            Ok(Transition::Applied(EventType::PaymentSucceeded))
        );
    }

    #[test]
    fn pending_fail_attempt_stays_pending_but_is_recorded() {
        let p = pending(1000);
        assert_eq!(
            apply(&p, &Command::FailAttempt),
            Ok(Transition::Applied(EventType::PaymentAttemptFailed))
        );
    }

    #[test]
    fn pending_abandon_attempt_stays_pending_but_is_recorded() {
        let p = pending(1000);
        assert_eq!(
            apply(&p, &Command::AbandonAttempt),
            Ok(Transition::Applied(EventType::PaymentAttemptAbandoned))
        );
    }

    #[test]
    fn succeeded_partial_refund_is_applied_and_stays_succeeded() {
        let p = succeeded(1000, 0);
        assert_eq!(
            apply(&p, &Command::Refund { amount: 400 }),
            Ok(Transition::Applied(EventType::PaymentRefunded))
        );
    }

    #[test]
    fn succeeded_refund_reaching_the_full_amount_is_applied() {
        let p = succeeded(1000, 600);
        assert_eq!(
            apply(&p, &Command::Refund { amount: 400 }),
            Ok(Transition::Applied(EventType::PaymentRefunded))
        );
    }

    // ---- succeeded is final -------------------------------------------------

    #[test]
    fn succeeded_settle_again_with_the_same_amount_is_a_no_op() {
        let p = succeeded(1000, 0);
        assert_eq!(
            apply(&p, &Command::Settle { amount: 1000 }),
            Ok(Transition::NoOp)
        );
    }

    #[test]
    fn succeeded_fail_attempt_changes_nothing() {
        let p = succeeded(1000, 0);
        assert_eq!(apply(&p, &Command::FailAttempt), Ok(Transition::NoOp));
    }

    #[test]
    fn succeeded_abandon_attempt_changes_nothing() {
        let p = succeeded(1000, 0);
        assert_eq!(apply(&p, &Command::AbandonAttempt), Ok(Transition::NoOp));
    }

    #[test]
    fn refunded_settle_fail_or_abandon_are_all_no_ops() {
        let p = refunded(1000);
        assert_eq!(
            apply(&p, &Command::Settle { amount: 1000 }),
            Ok(Transition::NoOp)
        );
        assert_eq!(apply(&p, &Command::FailAttempt), Ok(Transition::NoOp));
        assert_eq!(apply(&p, &Command::AbandonAttempt), Ok(Transition::NoOp));
    }

    // ---- illegal transitions are refused, never silently accepted ---------

    #[test]
    fn a_mismatched_amount_is_refused_however_the_payment_stands() {
        assert_eq!(
            apply(&pending(1000), &Command::Settle { amount: 999 }),
            Err(DomainError::AmountMismatch)
        );
        assert_eq!(
            apply(&succeeded(1000, 0), &Command::Settle { amount: 999 }),
            Err(DomainError::AmountMismatch)
        );
        assert_eq!(
            apply(&refunded(1000), &Command::Settle { amount: 999 }),
            Err(DomainError::AmountMismatch)
        );
    }

    #[test]
    fn refunding_a_payment_that_never_succeeded_is_refused() {
        assert_eq!(
            apply(&pending(1000), &Command::Refund { amount: 100 }),
            Err(DomainError::NotRefundable)
        );
    }

    #[test]
    fn refunding_a_fully_refunded_payment_is_refused() {
        assert_eq!(
            apply(&refunded(1000), &Command::Refund { amount: 1 }),
            Err(DomainError::NotRefundable)
        );
    }

    #[test]
    fn a_refund_that_would_pass_the_charge_is_refused_without_effect() {
        let p = succeeded(1000, 800);
        assert_eq!(
            apply(&p, &Command::Refund { amount: 300 }),
            Err(DomainError::RefundExceedsRemaining {
                requested: 300,
                remaining: 200
            })
        );
    }

    #[test]
    fn a_zero_or_negative_refund_is_refused() {
        let p = succeeded(1000, 0);
        assert_eq!(
            apply(&p, &Command::Refund { amount: 0 }),
            Err(DomainError::InvalidRefundAmount)
        );
        assert_eq!(
            apply(&p, &Command::Refund { amount: -1 }),
            Err(DomainError::InvalidRefundAmount)
        );
    }
}
