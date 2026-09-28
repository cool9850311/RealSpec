//! The test-card table, `spec.md` → "The payment provider mock", as a pure
//! function from a card number to what 3-D Secure will eventually report.
//! The outcome is decided here, at card entry, and only *released* later —
//! by the issuer page or by `/__control/callbacks` — unchanged by which
//! release path is used, except that failing the issuer's own challenge
//! (`outcome=fail`) overrides it to a 3-D-Secure failure regardless of what
//! the card would otherwise have said (`shop.feature`, "A customer who fails
//! authentication can try again").

use crate::state::CardOutcome;
use crate::util::{bin6, last4, luhn_valid};

/// `Err` is a card the provider's own validation refuses before the issuer
/// ever sees it — Luhn-invalid, or not shaped like a card at all.
pub fn decide(
    card_number: &str,
    card_expiry: &str,
    card_cvc: &str,
) -> Result<CardOutcome, &'static str> {
    let digits: String = card_number.chars().filter(|c| !c.is_whitespace()).collect();
    if digits.len() < 12 || digits.len() > 19 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err("Invalid card number");
    }
    if !is_expiry_shaped(card_expiry) {
        return Err("Invalid expiry");
    }
    if !(card_cvc.len() == 3 || card_cvc.len() == 4)
        || !card_cvc.bytes().all(|b| b.is_ascii_digit())
    {
        return Err("Invalid CVC");
    }
    if !luhn_valid(&digits) {
        return Err("CheckMacValue Error");
    }

    let (rtn_code, failure_code) = match digits.as_str() {
        "4000000000000002" => (10_100_058, Some("card_declined")),
        "4000000000009995" => (10_100_058, Some("insufficient_funds")),
        "4000000000003063" => (10_100_058, Some("three_ds_failed")),
        _ => (1, None),
    };
    Ok(CardOutcome {
        rtn_code,
        failure_code: failure_code.map(str::to_string),
        card6: bin6(&digits),
        card4: last4(&digits),
        eci: "05".to_string(),
        auth_code: if rtn_code == 1 {
            Some("777888".to_string())
        } else {
            None
        },
    })
}

fn is_expiry_shaped(expiry: &str) -> bool {
    let bytes = expiry.as_bytes();
    bytes.len() == 5
        && bytes[0].is_ascii_digit()
        && bytes[1].is_ascii_digit()
        && bytes[2] == b'/'
        && bytes[3].is_ascii_digit()
        && bytes[4].is_ascii_digit()
}

/// What the issuer's own "Fail authentication" button reports, regardless of
/// the underlying card: a 3-D Secure failure, exactly as declining to
/// complete the challenge would in life.
pub fn three_ds_failed(card: &CardOutcome) -> CardOutcome {
    CardOutcome {
        rtn_code: 10_100_058,
        failure_code: Some("three_ds_failed".to_string()),
        card6: card.card6.clone(),
        card4: card.card4.clone(),
        eci: card.eci.clone(),
        auth_code: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_success_card_pays() {
        let outcome = decide("4242424242424242", "12/30", "123").unwrap();
        assert_eq!(outcome.rtn_code, 1);
        assert_eq!(outcome.card4, "4242");
        assert_eq!(outcome.card6, "424242");
        assert!(outcome.failure_code.is_none());
    }

    #[test]
    fn any_other_luhn_valid_card_pays() {
        let outcome = decide("4111111111111111", "12/30", "123").unwrap();
        assert_eq!(outcome.rtn_code, 1);
    }

    #[test]
    fn the_declined_cards_carry_their_own_failure_reason() {
        assert_eq!(
            decide("4000000000000002", "12/30", "123")
                .unwrap()
                .failure_code
                .as_deref(),
            Some("card_declined")
        );
        assert_eq!(
            decide("4000000000009995", "12/30", "123")
                .unwrap()
                .failure_code
                .as_deref(),
            Some("insufficient_funds")
        );
        assert_eq!(
            decide("4000000000003063", "12/30", "123")
                .unwrap()
                .failure_code
                .as_deref(),
            Some("three_ds_failed")
        );
    }

    #[test]
    fn the_luhn_invalid_card_is_refused_before_the_issuer() {
        assert!(decide("4242424242424241", "12/30", "123").is_err());
    }

    #[test]
    fn malformed_expiry_or_cvc_is_refused() {
        assert!(decide("4242424242424242", "1230", "123").is_err());
        assert!(decide("4242424242424242", "12/30", "12").is_err());
    }

    #[test]
    fn failing_the_issuer_overrides_a_card_that_would_have_paid() {
        let paid = decide("4242424242424242", "12/30", "123").unwrap();
        let failed = three_ds_failed(&paid);
        assert_eq!(failed.rtn_code, 10_100_058);
        assert_eq!(failed.failure_code.as_deref(), Some("three_ds_failed"));
        assert_eq!(failed.card4, "4242");
    }
}
