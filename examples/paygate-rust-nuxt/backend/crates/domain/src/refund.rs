//! Refund arithmetic. Enforced three times over in the running system — the
//! row lock, this check, and `payments`'s own
//! `CHECK (amount_refunded BETWEEN 0 AND amount)` — but this is the one place
//! it is spelled out as a pure function, so it can be tested at both
//! boundaries without a database.

use crate::error::DomainError;
use crate::ids::Amount;

/// How much of `amount` remains to be refunded.
pub fn refundable(amount: Amount, amount_refunded: Amount) -> Amount {
    amount - amount_refunded
}

/// The rule from spec.md, "Refunds": `0 < req <= amount - amount_refunded`.
pub fn check_refund(
    amount: Amount,
    amount_refunded: Amount,
    req: Amount,
) -> Result<(), DomainError> {
    if req <= 0 {
        return Err(DomainError::InvalidRefundAmount);
    }
    let remaining = refundable(amount, amount_refunded);
    if req > remaining {
        return Err(DomainError::RefundExceedsRemaining {
            requested: req,
            remaining,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refundable_is_the_gap_between_amount_and_what_was_already_given_back() {
        assert_eq!(refundable(1000, 0), 1000);
        assert_eq!(refundable(1000, 400), 600);
        assert_eq!(refundable(1000, 1000), 0);
    }

    #[test]
    fn the_smallest_legal_refund_is_accepted() {
        assert_eq!(check_refund(1000, 0, 1), Ok(()));
    }

    #[test]
    fn a_refund_that_exactly_exhausts_the_remainder_is_accepted() {
        assert_eq!(check_refund(1000, 400, 600), Ok(()));
    }

    #[test]
    fn one_more_than_remains_is_refused() {
        assert_eq!(
            check_refund(1000, 400, 601),
            Err(DomainError::RefundExceedsRemaining {
                requested: 601,
                remaining: 600
            })
        );
    }

    #[test]
    fn zero_is_refused() {
        assert_eq!(
            check_refund(1000, 0, 0),
            Err(DomainError::InvalidRefundAmount)
        );
    }

    #[test]
    fn negative_is_refused() {
        assert_eq!(
            check_refund(1000, 0, -1),
            Err(DomainError::InvalidRefundAmount)
        );
    }

    #[test]
    fn nothing_remains_once_fully_refunded() {
        assert_eq!(
            check_refund(1000, 1000, 1),
            Err(DomainError::RefundExceedsRemaining {
                requested: 1,
                remaining: 0
            })
        );
    }
}
