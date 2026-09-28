//! Small, pure helpers with no state of their own: the Luhn check that
//! decides whether a card is refused before it ever reaches the issuer,
//! dollar formatting for the issuer page, and pulling a path back out of the
//! absolute `ReturnURL` paygate hands the mock so a callback can be replayed
//! against a different origin (`PROVIDER_CALLBACK_URLS`) than the one named
//! in the form.

/// The Luhn checksum, the one thing a card number is validated against
/// before the provider even looks at what it is. `4242424242424241` fails
/// this by one digit on purpose (spec.md, "The payment provider mock").
pub fn luhn_valid(digits: &str) -> bool {
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let mut sum: u32 = 0;
    let mut double = false;
    for b in digits.bytes().rev() {
        let mut d = u32::from(b - b'0');
        if double {
            d *= 2;
            if d > 9 {
                d -= 9;
            }
        }
        sum += d;
        double = !double;
    }
    sum % 10 == 0
}

/// `1250` minor units -> `"$12.50"`, the shape the issuer page's
/// `testid:issuer-amount` renders (e2e `shop.feature`). ECPay's own wire
/// protocol carries no currency field, so the mock renders every amount in
/// this one dollar shape regardless of the merchant's actual currency —
/// nothing on this page is otherwise asserted for a non-USD merchant.
pub fn format_dollars(minor_units: i64) -> String {
    let sign = if minor_units < 0 { "-" } else { "" };
    let abs = minor_units.unsigned_abs();
    format!("{sign}${}.{:02}", abs / 100, abs % 100)
}

/// Pulls the path (plus query, if any) out of an absolute URL, or returns
/// the input unchanged if it is already path-only. Used to replay a
/// callback's `ReturnURL` (`https://paygate.example.com/api/v1/webhooks/ecpay`)
/// against one of `PROVIDER_CALLBACK_URLS`'s bare origins instead of the
/// origin the form itself named.
pub fn path_of(url: &str) -> String {
    if let Some(scheme_end) = url.find("://") {
        let after_scheme = &url[scheme_end + 3..];
        match after_scheme.find('/') {
            Some(slash) => after_scheme[slash..].to_string(),
            None => "/".to_string(),
        }
    } else if url.starts_with('/') {
        url.to_string()
    } else {
        format!("/{url}")
    }
}

/// `now`, in ECPay's own date shape and Taiwan's fixed UTC+8 (no DST) —
/// `common.rs`'s `format_provider_datetime` in `paygate-provider`, but that
/// helper is `pub(crate)` there, so the mock keeps its own copy rather than
/// widen that crate's public surface for one caller.
pub fn now_taipei_string() -> String {
    let taipei = chrono::FixedOffset::east_opt(8 * 3600).expect("UTC+8 is a valid offset");
    chrono::Utc::now()
        .with_timezone(&taipei)
        .format("%Y/%m/%d %H:%M:%S")
        .to_string()
}

/// The first six / last four digits of a card number, the only fragments of
/// it that ever leave the provider (spec.md, "The card never enters this
/// system").
pub fn bin6(card_number: &str) -> String {
    card_number.chars().take(6).collect()
}

pub fn last4(card_number: &str) -> String {
    let len = card_number.chars().count();
    card_number.chars().skip(len.saturating_sub(4)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn luhn_accepts_the_standard_test_visa_number() {
        assert!(luhn_valid("4242424242424242"));
    }

    #[test]
    fn luhn_rejects_the_off_by_one_digit() {
        assert!(!luhn_valid("4242424242424241"));
    }

    #[test]
    fn luhn_rejects_non_digits_and_empty() {
        assert!(!luhn_valid(""));
        assert!(!luhn_valid("42424242424242ab"));
    }

    #[test]
    fn dollars_format_pads_cents_and_handles_round_numbers() {
        assert_eq!(format_dollars(1250), "$12.50");
        assert_eq!(format_dollars(100), "$1.00");
        assert_eq!(format_dollars(5), "$0.05");
    }

    #[test]
    fn path_of_strips_scheme_and_authority() {
        assert_eq!(
            path_of("https://paygate.example.com/api/v1/webhooks/ecpay"),
            "/api/v1/webhooks/ecpay"
        );
        assert_eq!(
            path_of("http://api-1:8080/api/v1/webhooks/newebpay?x=1"),
            "/api/v1/webhooks/newebpay?x=1"
        );
        assert_eq!(path_of("/already/a/path"), "/already/a/path");
        assert_eq!(path_of("https://host.only"), "/");
    }

    #[test]
    fn bin_and_last4_split_the_card_number() {
        assert_eq!(bin6("4242424242424242"), "424242");
        assert_eq!(last4("4242424242424242"), "4242");
    }
}
