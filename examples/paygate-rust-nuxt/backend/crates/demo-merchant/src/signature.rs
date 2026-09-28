//! Verifying paygate's callback the way `/notify-strict` promises to:
//! recompute `CheckMacValue` with the MERCHANT's own `HashKey`/`HashIV`
//! (`MERCHANT_HASH_KEY` / `MERCHANT_HASH_IV`) — never the provider's pair,
//! which this crate never sees — and compare in constant time.
//!
//! `paygate_provider::ecpay::check_mac_value` is the same five-step algorithm
//! paygate itself signs the hand-off with; the merchant runs it again over
//! the callback's own fields (`spec.md`, "Telling the merchant": "paygate
//! sends its merchants the same shape it is sent, one level up"). This crate
//! has no dependency on `subtle` (`Cargo.toml` does not list it), so the
//! comparison below is a manual constant-time xor-fold instead.

use std::collections::BTreeMap;

/// Recomputes `CheckMacValue` over every field but itself and compares it,
/// constant-time and case-insensitively (as ECPay's own algorithm is
/// specified — ECPay uppercases the digest, but a defensive comparison does
/// not assume every caller already has).
pub fn verify_check_mac_value(
    hash_key: &str,
    hash_iv: &str,
    form: &BTreeMap<String, String>,
) -> bool {
    let Some(provided) = form.get("CheckMacValue") else {
        return false;
    };
    let mut signed_fields = form.clone();
    signed_fields.remove("CheckMacValue");
    let expected = paygate_provider::ecpay::check_mac_value(hash_key, hash_iv, &signed_fields);
    constant_time_eq_ignore_case(&expected, provided)
}

/// Constant-time (with respect to content; the length check and the
/// uppercasing are not, which leaks only the LENGTH of a valid signature —
/// a fixed, public constant, never the secret itself) case-insensitive
/// comparison.
fn constant_time_eq_ignore_case(expected: &str, provided: &str) -> bool {
    let expected = expected.as_bytes().to_ascii_uppercase();
    let provided = provided.as_bytes().to_ascii_uppercase();
    if expected.len() != provided.len() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in expected.iter().zip(provided.iter()) {
        diff |= a ^ b;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signed_form(hash_key: &str, hash_iv: &str) -> BTreeMap<String, String> {
        let mut form = BTreeMap::new();
        form.insert("MerchantID".to_string(), "2000132".to_string());
        form.insert("MerchantTradeNo".to_string(), "ACME-1".to_string());
        form.insert("TradeNo".to_string(), "2109210000000".to_string());
        form.insert("RtnCode".to_string(), "1".to_string());
        form.insert("RtnMsg".to_string(), "paid".to_string());
        form.insert("TradeAmt".to_string(), "1250".to_string());
        form.insert("PaymentDate".to_string(), "2026/09/21 14:03:11".to_string());
        let mac = paygate_provider::ecpay::check_mac_value(hash_key, hash_iv, &form);
        form.insert("CheckMacValue".to_string(), mac);
        form
    }

    #[test]
    fn a_correctly_signed_callback_verifies() {
        let form = signed_form("acmehashkey0123456789abcdef01234", "acmehashiv012345");
        assert!(verify_check_mac_value(
            "acmehashkey0123456789abcdef01234",
            "acmehashiv012345",
            &form
        ));
    }

    #[test]
    fn a_callback_signed_with_the_wrong_key_is_refused() {
        let form = signed_form("someone-elses-key", "someone-elses-iv");
        assert!(!verify_check_mac_value(
            "acmehashkey0123456789abcdef01234",
            "acmehashiv012345",
            &form
        ));
    }

    #[test]
    fn a_tampered_amount_is_refused() {
        let mut form = signed_form("acmehashkey0123456789abcdef01234", "acmehashiv012345");
        form.insert("TradeAmt".to_string(), "999999".to_string());
        assert!(!verify_check_mac_value(
            "acmehashkey0123456789abcdef01234",
            "acmehashiv012345",
            &form
        ));
    }

    #[test]
    fn a_missing_check_mac_value_is_refused() {
        let mut form = signed_form("acmehashkey0123456789abcdef01234", "acmehashiv012345");
        form.remove("CheckMacValue");
        assert!(!verify_check_mac_value(
            "acmehashkey0123456789abcdef01234",
            "acmehashiv012345",
            &form
        ));
    }

    #[test]
    fn the_comparison_is_case_insensitive() {
        let mut form = signed_form("acmehashkey0123456789abcdef01234", "acmehashiv012345");
        let lower = form.get("CheckMacValue").unwrap().to_ascii_lowercase();
        form.insert("CheckMacValue".to_string(), lower);
        assert!(verify_check_mac_value(
            "acmehashkey0123456789abcdef01234",
            "acmehashiv012345",
            &form
        ));
    }
}
