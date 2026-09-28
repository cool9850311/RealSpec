//! The provider trade number. ECPay's own rule, not a stylistic choice:
//! `MerchantTradeNo` must be at most twenty characters, mixed alphanumerics
//! only, and never reused — whatever became of the order it named. See
//! spec.md, "One order, three numbers".
//!
//! The format is `<prefix><8-digit sequence>SN<5 random alphanumerics>`,
//! which is `len(prefix) + 15` characters — 8 for the sequence, 2 for the
//! literal `SN`, 5 for the random suffix. For the whole thing to fit twenty,
//! the prefix can be at most five characters; ECPay's own WooCommerce
//! plugin shipped a changelog entry ("fixed duplicate order numbers caused
//! by an over-long prefix setting") for exactly the failure that happens
//! when a deployment ignores that arithmetic, which is why
//! `validate_trade_no_prefix` is a startup check rather than a comment.

use crate::error::DomainError;
use rand::distributions::Alphanumeric;
use rand::Rng;

/// How many characters the sequence, the literal `SN` and the random suffix
/// take up together — the part of the twenty a prefix cannot use.
const FIXED_SUFFIX_LEN: usize = 15;
/// ECPay's own limit on `MerchantTradeNo`.
const MAX_TRADE_NO_LEN: usize = 20;
const RANDOM_SUFFIX_LEN: usize = 5;

/// Mint a provider trade number. Pure and total: any `u64` sequence and any
/// RNG produce a string, though a caller should have already rejected the
/// prefix with [`validate_trade_no_prefix`] so the result actually fits
/// ECPay's twenty-character rule.
pub fn provider_trade_no(prefix: &str, sequence: u64, rng: &mut impl Rng) -> String {
    let random: String = (0..RANDOM_SUFFIX_LEN)
        .map(|_| rng.sample(Alphanumeric) as char)
        .collect();
    format!("{prefix}{sequence:08}SN{random}")
}

/// Startup validation for `PROVIDER_TRADE_NO_PREFIX`: the prefix must be
/// `[A-Za-z0-9]+` (ECPay's own alphabet for the whole number) and leave room
/// for the fixed fifteen-character suffix within twenty characters total.
pub fn validate_trade_no_prefix(prefix: &str) -> Result<(), DomainError> {
    let ok = !prefix.is_empty()
        && prefix.chars().all(|c| c.is_ascii_alphanumeric())
        && prefix.chars().count() + FIXED_SUFFIX_LEN <= MAX_TRADE_NO_LEN;
    if ok {
        Ok(())
    } else {
        Err(DomainError::InvalidTradeNoPrefix(prefix.to_string()))
    }
}

/// ECPay's own rule for the whole number, independent of how it was built:
/// `^[A-Za-z0-9]{1,20}$`. This is also `payment_attempts`'s CHECK constraint,
/// repeated here so a caller can refuse a bad one before ever reaching the
/// database.
pub fn is_valid_provider_trade_no(s: &str) -> bool {
    let len = s.chars().count();
    (1..=MAX_TRADE_NO_LEN).contains(&len) && s.chars().all(|c| c.is_ascii_alphanumeric())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;
    use std::collections::HashSet;

    fn rng() -> rand::rngs::StdRng {
        rand::rngs::StdRng::seed_from_u64(0xC0FFEE)
    }

    #[test]
    fn shape_is_prefix_then_eight_digit_sequence_then_sn_then_five_alphanumerics() {
        let mut r = rng();
        let no = provider_trade_no("ACME", 42, &mut r);
        assert_eq!(no.len(), "ACME".len() + 8 + 2 + 5);
        assert!(no.starts_with("ACME00000042SN"));
        let suffix = &no[no.len() - 5..];
        assert_eq!(suffix.len(), 5);
        assert!(suffix.chars().all(|c| c.is_ascii_alphanumeric()));
    }

    #[test]
    fn character_set_is_alphanumeric_only_end_to_end() {
        let mut r = rng();
        for seq in 0..200u64 {
            let no = provider_trade_no("ABC12", seq, &mut r);
            assert!(is_valid_provider_trade_no(&no), "{no:?} should be valid");
        }
    }

    #[test]
    fn a_five_character_prefix_still_fits_twenty() {
        // len("ABCDE") + 15 == 20, exactly the limit.
        assert!(validate_trade_no_prefix("ABCDE").is_ok());
        let mut r = rng();
        let no = provider_trade_no("ABCDE", 1, &mut r);
        assert_eq!(no.len(), 20);
        assert!(is_valid_provider_trade_no(&no));
    }

    #[test]
    fn a_six_character_prefix_is_the_ecpay_plugin_bug_and_is_rejected() {
        // len("ABCDEF") + 15 == 21: exactly the over-long prefix that "squeezed
        // out the unique part" in ECPay's own plugin changelog. It must never
        // reach provider_trade_no in the first place.
        assert_eq!(
            validate_trade_no_prefix("ABCDEF"),
            Err(DomainError::InvalidTradeNoPrefix("ABCDEF".to_string()))
        );
    }

    #[test]
    fn prefix_must_be_alphanumeric_only() {
        assert!(validate_trade_no_prefix("AB-C").is_err());
        assert!(validate_trade_no_prefix("AB_C").is_err());
        assert!(validate_trade_no_prefix("").is_err());
    }

    #[test]
    fn a_thousand_numbers_for_one_order_are_all_distinct() {
        let mut r = rng();
        let mut seen = HashSet::new();
        for _ in 0..1000 {
            // Same prefix AND the same sequence every time: a real order
            // retried a thousand times would still only ever mint a fresh
            // sequence, but the random suffix alone must already be enough
            // to keep every number distinct.
            let no = provider_trade_no("ACME1", 7, &mut r);
            assert!(seen.insert(no), "provider_trade_no produced a repeat");
        }
        assert_eq!(seen.len(), 1000);
    }

    #[test]
    fn is_valid_provider_trade_no_matches_ecpays_pattern() {
        assert!(is_valid_provider_trade_no("A"));
        assert!(is_valid_provider_trade_no(&"A".repeat(20)));
        assert!(!is_valid_provider_trade_no(&"A".repeat(21)));
        assert!(!is_valid_provider_trade_no(""));
        assert!(!is_valid_provider_trade_no("has space"));
        assert!(!is_valid_provider_trade_no("dash-not-ok"));
        assert!(!is_valid_provider_trade_no("under_score"));
    }
}
