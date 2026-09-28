//! Wrapping and unwrapping ECPay's clear-text `CheckMacValue` form and
//! NewebPay's encrypted `TradeInfo`/`TradeSha`, on top of the primitives
//! `paygate_provider` exports for exactly this: verifying with
//! `paygate_provider`'s own functions is what makes "paygate signed it
//! correctly" a real cryptographic fact rather than a matching pair of bugs
//! (`spec.md`, "Authentication").
//!
//! Every verification here is the mock refusing what does not check out;
//! every signing is the mock speaking as the provider — either honestly, or,
//! for the `forged` callback/query variants, with a key pair
//! [`forged_ecpay_creds`]/[`forged_newebpay_creds`] invents on the spot,
//! standing in for "a key this provider never issued".

use paygate_provider::{ecpay, newebpay, PlatformCredentials};
use std::collections::BTreeMap;

pub fn ecpay_verify(creds: &PlatformCredentials, form: &BTreeMap<String, String>) -> bool {
    match form.get("CheckMacValue") {
        None => false,
        Some(provided) => {
            let mut signed = form.clone();
            signed.remove("CheckMacValue");
            let expected = ecpay::check_mac_value(&creds.hash_key, &creds.hash_iv, &signed);
            expected.eq_ignore_ascii_case(provided)
        }
    }
}

pub fn ecpay_sign(creds: &PlatformCredentials, form: &mut BTreeMap<String, String>) {
    form.remove("CheckMacValue");
    let mac = ecpay::check_mac_value(&creds.hash_key, &creds.hash_iv, form);
    form.insert("CheckMacValue".to_string(), mac);
}

/// Verifies `TradeSha` and, only if it verifies, decrypts and returns the
/// inner field map. `None` either way is "refused" to the caller — the mock
/// never distinguishes "wrong signature" from "would not decrypt" to the
/// wire, exactly as `paygate_provider::newebpay::NewebpayAdapter` does not.
pub fn newebpay_verify(
    creds: &PlatformCredentials,
    form: &BTreeMap<String, String>,
) -> Option<BTreeMap<String, String>> {
    let info = form.get("TradeInfo")?;
    let provided_sha = form.get("TradeSha")?;
    let expected_sha = newebpay::trade_sha(&creds.hash_key, &creds.hash_iv, info);
    if !expected_sha.eq_ignore_ascii_case(provided_sha) {
        return None;
    }
    newebpay::decrypt_trade_info(&creds.hash_key, &creds.hash_iv, info).ok()
}

/// Wraps `inner` into the outer `MerchantID`/`TradeInfo`/`TradeSha`/`Version`
/// form NewebPay's own shape uses everywhere: the cashier, the query answer,
/// the callback.
pub fn newebpay_wrap(
    creds: &PlatformCredentials,
    merchant_id: &str,
    inner: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let info = newebpay::trade_info(&creds.hash_key, &creds.hash_iv, inner);
    let sha = newebpay::trade_sha(&creds.hash_key, &creds.hash_iv, &info);
    let mut form = BTreeMap::new();
    form.insert("MerchantID".to_string(), merchant_id.to_string());
    form.insert("TradeInfo".to_string(), info);
    form.insert("TradeSha".to_string(), sha);
    form.insert("Version".to_string(), "2.0".to_string());
    form
}

/// A key pair this provider never issued anyone — ECPay's shape. Any string
/// works for `CheckMacValue`'s SHA-256, so these need only differ from the
/// platform's real pair.
pub fn forged_ecpay_creds() -> PlatformCredentials {
    PlatformCredentials {
        platform_id: "0000000".to_string(),
        hash_key: "forged-hash-key-nobody-issued".to_string(),
        hash_iv: "forged-hash-iv-nobody-issued".to_string(),
    }
}

/// A key pair this provider never issued anyone — NewebPay's shape. Unlike
/// ECPay's, this pair doubles as an AES-256 key/IV, so it must keep the real
/// pair's lengths (32 bytes / 16 bytes) or `trade_info` silently produces
/// nothing to sign at all.
pub fn forged_newebpay_creds() -> PlatformCredentials {
    PlatformCredentials {
        platform_id: "FORGED0000".to_string(),
        hash_key: "x".repeat(32),
        hash_iv: "y".repeat(16),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn creds() -> PlatformCredentials {
        PlatformCredentials {
            platform_id: "3002607".to_string(),
            hash_key: "pwFHCqoQZGmho4w6".to_string(),
            hash_iv: "EkRm7iFT261dpevs".to_string(),
        }
    }

    /// NewebPay's own fixture pair (32-byte key, 16-byte IV) — distinct from
    /// [`creds`], which is ECPay's shape and far too short to be an AES-256
    /// key.
    fn newebpay_creds() -> PlatformCredentials {
        PlatformCredentials {
            platform_id: "MS12345678".to_string(),
            hash_key: "Fs5cX1TGqYZ3kLmN8pQrS2tUvW4xY6zA".to_string(),
            hash_iv: "B7cD9eF1gH3iJ5kL".to_string(),
        }
    }

    #[test]
    fn ecpay_round_trips_through_sign_and_verify() {
        let mut form = BTreeMap::new();
        form.insert("A".to_string(), "1".to_string());
        form.insert("B".to_string(), "2".to_string());
        ecpay_sign(&creds(), &mut form);
        assert!(ecpay_verify(&creds(), &form));
    }

    #[test]
    fn ecpay_forged_signature_does_not_verify_against_the_real_pair() {
        let mut form = BTreeMap::new();
        form.insert("A".to_string(), "1".to_string());
        ecpay_sign(&forged_ecpay_creds(), &mut form);
        assert!(!ecpay_verify(&creds(), &form));
    }

    #[test]
    fn newebpay_round_trips_through_wrap_and_verify() {
        let mut inner = BTreeMap::new();
        inner.insert("TotalAmount".to_string(), "1250".to_string());
        let form = newebpay_wrap(&newebpay_creds(), "MS12345678", &inner);
        let recovered = newebpay_verify(&newebpay_creds(), &form).expect("verifies");
        assert_eq!(recovered.get("TotalAmount"), Some(&"1250".to_string()));
    }

    #[test]
    fn newebpay_forged_signature_does_not_verify_against_the_real_pair() {
        let mut inner = BTreeMap::new();
        inner.insert("TotalAmount".to_string(), "1250".to_string());
        let form = newebpay_wrap(&forged_newebpay_creds(), "MS12345678", &inner);
        assert!(newebpay_verify(&newebpay_creds(), &form).is_none());
    }
}
