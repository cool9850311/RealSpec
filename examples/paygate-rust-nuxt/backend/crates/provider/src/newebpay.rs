//! NewebPay's shape: the parameters urlencoded, then packed into
//! `TradeInfo` (AES-256-CBC/PKCS7, hex-encoded lowercase) and signed with
//! `TradeSha` — SHA-256 over `"HashKey=<key>&" + TradeInfo + "&HashIV=<iv>"`,
//! uppercased. The same `HashKey`/`HashIV` pair NewebPay issued the platform
//! is both the AES key/IV and the signing wrapper, exactly as ECPay's
//! `CheckMacValue` reuses its own pair (spec.md, "Choosing a provider").
//!
//! See `crate::common`'s module doc for why the field vocabulary *inside*
//! `TradeInfo` is the same one ECPay uses in the clear: nothing in
//! `payment-provider.yaml` fixes NewebPay's internal field names (they are
//! ciphertext to everyone outside this crate), so both adapters share one
//! vocabulary and differ only in how they wrap it.

use crate::common::{
    core_hand_off_fields, extract_callback_facts, parse_key_value_body, signatures_match,
    CallbackFacts, HandOff, HandOffRequest, PlatformCredentials, QueryAnswer, QueryRequest,
    RefundAnswer, RefundRequest, SignedRequest,
};
use crate::error::ProviderError;
use crate::ProviderAdapter;
use chrono::Utc;
use cipher::block_padding::Pkcs7;
use cipher::{BlockDecryptMut, BlockEncryptMut, KeyIvInit};
use paygate_domain::ProviderCode;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

type Aes256CbcEnc = cbc::Encryptor<aes::Aes256>;
type Aes256CbcDec = cbc::Decryptor<aes::Aes256>;

/// The parameter map, urlencoded, then AES-256-CBC/PKCS7 encrypted with the
/// `HashKey` as key and `HashIV` as IV, hex-encoded lowercase. NewebPay's
/// `HashKey` is 32 bytes (AES-256) and `HashIV` 16 (one block) — a pair of
/// the wrong length cannot encrypt anything, so this returns an empty string
/// rather than panicking; `crates/infra`'s startup validation is the place
/// that refuses a misconfigured pair before it ever reaches here.
pub fn trade_info(hash_key: &str, hash_iv: &str, params: &BTreeMap<String, String>) -> String {
    let plaintext = serde_urlencoded::to_string(params).unwrap_or_default();
    match Aes256CbcEnc::new_from_slices(hash_key.as_bytes(), hash_iv.as_bytes()) {
        Ok(enc) => hex::encode(enc.encrypt_padded_vec_mut::<Pkcs7>(plaintext.as_bytes())),
        Err(_) => String::new(),
    }
}

/// `TradeSha`: `SHA256("HashKey=<key>&" + TradeInfo + "&HashIV=<iv>")`, uppercased.
pub fn trade_sha(hash_key: &str, hash_iv: &str, trade_info: &str) -> String {
    let wrapped = format!("HashKey={hash_key}&{trade_info}&HashIV={hash_iv}");
    hex::encode_upper(Sha256::digest(wrapped.as_bytes()))
}

/// The inverse of [`trade_info`]: hex-decode, AES-256-CBC/PKCS7 decrypt, then
/// parse the recovered urlencoded string back into a field map.
pub fn decrypt_trade_info(
    hash_key: &str,
    hash_iv: &str,
    hex_ciphertext: &str,
) -> Result<BTreeMap<String, String>, ProviderError> {
    let ciphertext = hex::decode(hex_ciphertext).map_err(|_| ProviderError::DecryptionFailed)?;
    let dec = Aes256CbcDec::new_from_slices(hash_key.as_bytes(), hash_iv.as_bytes())
        .map_err(|_| ProviderError::DecryptionFailed)?;
    let plaintext = dec
        .decrypt_padded_vec_mut::<Pkcs7>(&ciphertext)
        .map_err(|_| ProviderError::DecryptionFailed)?;
    Ok(form_urlencoded::parse(&plaintext)
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect())
}

fn verify(
    creds: &PlatformCredentials,
    form: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, ProviderError> {
    let info = form
        .get("TradeInfo")
        .ok_or(ProviderError::MissingField("TradeInfo"))?;
    let provided_sha = form
        .get("TradeSha")
        .ok_or(ProviderError::MissingField("TradeSha"))?;
    let expected_sha = trade_sha(&creds.hash_key, &creds.hash_iv, info);
    if !signatures_match(&expected_sha, provided_sha) {
        return Err(ProviderError::InvalidSignature);
    }
    decrypt_trade_info(&creds.hash_key, &creds.hash_iv, info)
}

pub struct NewebpayAdapter;

impl ProviderAdapter for NewebpayAdapter {
    fn code(&self) -> ProviderCode {
        ProviderCode::Newebpay
    }

    fn hand_off(
        &self,
        creds: &PlatformCredentials,
        req: &HandOffRequest,
    ) -> Result<HandOff, ProviderError> {
        let inner = core_hand_off_fields(req);
        let info = trade_info(&creds.hash_key, &creds.hash_iv, &inner);
        let sha = trade_sha(&creds.hash_key, &creds.hash_iv, &info);
        let mut fields = BTreeMap::new();
        fields.insert("MerchantID".to_string(), req.provider_merchant_id.clone());
        fields.insert("TradeInfo".to_string(), info);
        fields.insert("TradeSha".to_string(), sha);
        fields.insert("Version".to_string(), "2.0".to_string());
        Ok(HandOff {
            action: req.cashier_url.clone(),
            fields,
        })
    }

    fn parse_callback(
        &self,
        creds: &PlatformCredentials,
        form: &BTreeMap<String, String>,
    ) -> Result<CallbackFacts, ProviderError> {
        let inner = verify(creds, form)?;
        extract_callback_facts(&inner, 0)
    }

    fn ack_body(&self) -> &'static str {
        "1|OK"
    }

    fn build_query(&self, creds: &PlatformCredentials, q: &QueryRequest) -> SignedRequest {
        let mut inner = BTreeMap::new();
        inner.insert("MerchantID".to_string(), q.provider_merchant_id.clone());
        inner.insert("MerchantTradeNo".to_string(), q.provider_trade_no.clone());
        inner.insert("TimeStamp".to_string(), Utc::now().timestamp().to_string());
        let info = trade_info(&creds.hash_key, &creds.hash_iv, &inner);
        let sha = trade_sha(&creds.hash_key, &creds.hash_iv, &info);
        let mut form = BTreeMap::new();
        form.insert("MerchantID".to_string(), q.provider_merchant_id.clone());
        form.insert("TradeInfo".to_string(), info);
        form.insert("TradeSha".to_string(), sha);
        form.insert("Version".to_string(), "2.0".to_string());
        SignedRequest {
            url_path: q.query_url.clone(),
            form,
        }
    }

    fn parse_query_answer(
        &self,
        creds: &PlatformCredentials,
        body: &str,
    ) -> Result<QueryAnswer, ProviderError> {
        let form = parse_key_value_body(body);
        let inner = verify(creds, &form)?;
        let status = inner
            .get("TradeStatus")
            .ok_or(ProviderError::MissingField("TradeStatus"))?;
        match status.as_str() {
            "1" => Ok(QueryAnswer::Paid {
                facts: extract_callback_facts(&inner, 1)?,
            }),
            "10200095" => Ok(QueryAnswer::NeverCompleted),
            _ => Ok(QueryAnswer::Unpaid),
        }
    }

    fn build_refund(&self, creds: &PlatformCredentials, r: &RefundRequest) -> SignedRequest {
        let mut inner = BTreeMap::new();
        inner.insert("MerchantID".to_string(), r.provider_merchant_id.clone());
        inner.insert("MerchantTradeNo".to_string(), r.provider_trade_no.clone());
        inner.insert("TradeNo".to_string(), r.provider_charge_id.clone());
        inner.insert("TotalAmount".to_string(), r.amount.to_string());
        inner.insert("RefundTradeNo".to_string(), r.refund_id.clone());
        let post_data = trade_info(&creds.hash_key, &creds.hash_iv, &inner);
        let mut form = BTreeMap::new();
        form.insert("MerchantID_".to_string(), r.provider_merchant_id.clone());
        form.insert("PostData_".to_string(), post_data);
        SignedRequest {
            url_path: r.refund_url.clone(),
            form,
        }
    }

    fn parse_refund_answer(
        &self,
        _creds: &PlatformCredentials,
        body: &str,
    ) -> Result<RefundAnswer, ProviderError> {
        match serde_json::from_str::<serde_json::Value>(body) {
            Ok(value) => {
                let status = value.get("Status").and_then(|s| s.as_str()).unwrap_or("");
                if status.eq_ignore_ascii_case("SUCCESS") {
                    Ok(RefundAnswer::Succeeded)
                } else {
                    Ok(RefundAnswer::Failed(body.to_string()))
                }
            }
            Err(_) => Ok(RefundAnswer::Failed(body.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn creds() -> PlatformCredentials {
        PlatformCredentials {
            platform_id: "MS12345678".to_string(),
            hash_key: "Fs5cX1TGqYZ3kLmN8pQrS2tUvW4xY6zA".to_string(),
            hash_iv: "B7cD9eF1gH3iJ5kL".to_string(),
        }
    }

    /// Pinned regression vector for the AES-256-CBC/PKCS7 step, cross-checked
    /// against the system `openssl` binary — a tool with no relationship to
    /// this crate's `aes`/`cbc` dependencies — encrypting the exact same
    /// plaintext with the exact same key and IV (see this crate's build
    /// report for the commands). This is the check that catches a systematic
    /// error `trade_info`/`decrypt_trade_info` could share (a self-consistent
    /// round trip alone would not).
    #[test]
    fn trade_info_matches_the_openssl_cross_checked_vector() {
        let mut params = BTreeMap::new();
        params.insert("Amt".to_string(), "1000".to_string());
        params.insert("ClientBackURL".to_string(), "/shop/result".to_string());
        params.insert("ItemName".to_string(), "Beans".to_string());
        params.insert("MerchantID".to_string(), "MS12345678".to_string());
        params.insert(
            "MerchantTradeNo".to_string(),
            "GLBX00000001SNa1b2c".to_string(),
        );

        // Pin (and document) the exact plaintext this produces before AES,
        // since the openssl vector below was generated against this string.
        let plaintext = serde_urlencoded::to_string(&params).unwrap();
        assert_eq!(
            plaintext,
            "Amt=1000&ClientBackURL=%2Fshop%2Fresult&ItemName=Beans&MerchantID=MS12345678&MerchantTradeNo=GLBX00000001SNa1b2c"
        );

        let info = trade_info(&creds().hash_key, &creds().hash_iv, &params);
        assert_eq!(
            info,
            "0c8521b73168119feb95e9db7e292bee98aee1888f601bfd9fe7b749918736876f29aff03d14aeae77f9336a87d5301337113c897e9d405ad2069864919fc12d96d22d1621923a3955a2bf608e34f23a58bca98356861ee9f0c6ea6a2b046170a0a3ccfabf97c35b1da54894e77fb09d654cb6d573108885e3e45f8049b1112f"
        );

        let sha = trade_sha(&creds().hash_key, &creds().hash_iv, &info);
        assert_eq!(
            sha,
            "925114C81FB510057E0E54A17343659B1F62CF93B1DD5B6F6917EAE7E2539E2E"
        );
    }

    #[test]
    fn trade_info_round_trips_through_decrypt_trade_info() {
        let mut params = BTreeMap::new();
        params.insert("MerchantID".to_string(), "MS12345678".to_string());
        params.insert(
            "MerchantTradeNo".to_string(),
            "GLBX00000002SNzzzzz".to_string(),
        );
        params.insert("TotalAmount".to_string(), "2000".to_string());
        params.insert(
            "ItemName".to_string(),
            "Widget & Gadget (100%) ☕".to_string(),
        );

        let info = trade_info(&creds().hash_key, &creds().hash_iv, &params);
        let decrypted = decrypt_trade_info(&creds().hash_key, &creds().hash_iv, &info).unwrap();
        assert_eq!(decrypted, params);
    }

    #[test]
    fn a_tampered_ciphertext_does_not_decrypt_to_the_original() {
        let mut params = BTreeMap::new();
        params.insert("Amt".to_string(), "1000".to_string());
        let info = trade_info(&creds().hash_key, &creds().hash_iv, &params);
        let mut bytes = hex::decode(&info).unwrap();
        bytes[0] ^= 0xFF;
        let tampered = hex::encode(bytes);
        // Either the padding no longer validates (DecryptionFailed) or it
        // "succeeds" into garbage that is not the original map — either way
        // it must never silently equal what was actually sent.
        match decrypt_trade_info(&creds().hash_key, &creds().hash_iv, &tampered) {
            Ok(map) => assert_ne!(map, params),
            Err(ProviderError::DecryptionFailed) => {}
            Err(other) => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn hand_off_produces_no_cleartext_amount_and_verifies_with_the_platform_pair() {
        let req = HandOffRequest {
            cashier_url: "/provider/newebpay/MPG/mpg_gateway".to_string(),
            provider_merchant_id: "MS150086913".to_string(),
            provider_trade_no: "GLBX00000001SNa1b2c".to_string(),
            trade_date: Utc.with_ymd_and_hms(2026, 9, 21, 14, 3, 11).unwrap(),
            amount: 2000,
            item_desc: "Widget".to_string(),
            return_url: "https://paygate.example.com/api/v1/webhooks/newebpay".to_string(),
            client_back_url: "/shop/result".to_string(),
        };
        let handoff = NewebpayAdapter.hand_off(&creds(), &req).unwrap();
        assert_eq!(
            handoff.fields.get("MerchantID"),
            Some(&"MS150086913".to_string())
        );
        assert_eq!(handoff.fields.get("Version"), Some(&"2.0".to_string()));
        assert!(handoff.fields.contains_key("TradeInfo"));
        assert!(handoff.fields.contains_key("TradeSha"));
        for (k, v) in &handoff.fields {
            assert!(
                !v.contains("2000"),
                "amount leaked in the clear via field {k}"
            );
        }

        let inner = verify(&creds(), &handoff.fields).unwrap();
        assert_eq!(inner.get("TotalAmount"), Some(&"2000".to_string()));
        assert_eq!(
            inner.get("MerchantTradeNo"),
            Some(&"GLBX00000001SNa1b2c".to_string())
        );
    }

    #[test]
    fn a_callback_signed_with_the_wrong_pair_is_refused() {
        let mut inner = BTreeMap::new();
        inner.insert(
            "MerchantTradeNo".to_string(),
            "GLBX00000001SNa1b2c".to_string(),
        );
        inner.insert("RtnCode".to_string(), "1".to_string());
        inner.insert("TradeAmt".to_string(), "2000".to_string());
        let forged_key = "notThePlatformsRealKey000000000";
        let forged_iv = "notTheRealIV0000";
        let info = trade_info(forged_key, forged_iv, &inner);
        let sha = trade_sha(forged_key, forged_iv, &info);
        let mut form = BTreeMap::new();
        form.insert("MerchantID".to_string(), "MS150086913".to_string());
        form.insert("TradeInfo".to_string(), info);
        form.insert("TradeSha".to_string(), sha);
        form.insert("Version".to_string(), "2.0".to_string());

        assert_eq!(
            NewebpayAdapter.parse_callback(&creds(), &form),
            Err(ProviderError::InvalidSignature)
        );
    }

    #[test]
    fn a_correctly_signed_callback_verifies_and_parses() {
        let mut inner = BTreeMap::new();
        inner.insert(
            "MerchantTradeNo".to_string(),
            "GLBX00000001SNa1b2c".to_string(),
        );
        inner.insert("TradeNo".to_string(), "np_2109210000000".to_string());
        inner.insert("RtnCode".to_string(), "1".to_string());
        inner.insert("TradeAmt".to_string(), "2000".to_string());
        inner.insert("card4no".to_string(), "4242".to_string());
        inner.insert("card6no".to_string(), "424242".to_string());
        let info = trade_info(&creds().hash_key, &creds().hash_iv, &inner);
        let sha = trade_sha(&creds().hash_key, &creds().hash_iv, &info);
        let mut form = BTreeMap::new();
        form.insert("MerchantID".to_string(), "MS150086913".to_string());
        form.insert("TradeInfo".to_string(), info);
        form.insert("TradeSha".to_string(), sha);
        form.insert("Version".to_string(), "2.0".to_string());

        let facts = NewebpayAdapter.parse_callback(&creds(), &form).unwrap();
        assert_eq!(facts.provider_trade_no, "GLBX00000001SNa1b2c");
        assert_eq!(facts.amount, 2000);
        assert_eq!(facts.card_brand.as_deref(), Some("visa"));
    }

    #[test]
    fn refund_answer_reads_the_status_field() {
        assert_eq!(
            NewebpayAdapter
                .parse_refund_answer(&creds(), r#"{"Status":"SUCCESS","Message":"OK"}"#)
                .unwrap(),
            RefundAnswer::Succeeded
        );
        assert_eq!(
            NewebpayAdapter
                .parse_refund_answer(&creds(), r#"{"Status":"FAIL","Message":"bad signature"}"#)
                .unwrap(),
            RefundAnswer::Failed(r#"{"Status":"FAIL","Message":"bad signature"}"#.to_string())
        );
        assert_eq!(
            NewebpayAdapter
                .parse_refund_answer(&creds(), "not json")
                .unwrap(),
            RefundAnswer::Failed("not json".to_string())
        );
    }

    #[test]
    fn build_refund_never_puts_the_amount_in_the_clear() {
        let r = RefundRequest {
            refund_url: "/provider/newebpay/API/CreditCard/Close".to_string(),
            provider_merchant_id: "MS150086913".to_string(),
            provider_trade_no: "GLBX00000001SNa1b2c".to_string(),
            provider_charge_id: "np_2109210000000".to_string(),
            amount: 500,
            refund_id: "01931c4f-0000-7000-8000-00000000002b".to_string(),
        };
        let signed = NewebpayAdapter.build_refund(&creds(), &r);
        assert!(signed.form.contains_key("MerchantID_"));
        let post_data = signed.form.get("PostData_").expect("PostData_ present");
        assert!(
            !post_data.contains("500"),
            "refund amount leaked in the clear"
        );
        assert!(
            !post_data.contains("2109210000000"),
            "charge id leaked in the clear"
        );
    }
}
