//! ECPay's shape: parameters in the clear, signed with `CheckMacValue`.
//!
//! `CheckMacValue` is the piece everybody gets wrong, so it is implemented
//! literally against the five steps in `spec.md`, "Authentication", rather
//! than against a paraphrase of them:
//!
//! 1. sort the parameters by name, join with `&` as `k=v`
//! 2. wrap: `HashKey=<key>&<sorted>&HashIV=<iv>`
//! 3. URL-encode, then lowercase the whole string
//! 4. put seven characters back the way .NET's `HttpUtility.UrlEncode`
//!    leaves them: `%2d`→`-`, `%5f`→`_`, `%2e`→`.`, `%21`→`!`, `%2a`→`*`,
//!    `%28`→`(`, `%29`→`)`
//! 5. SHA-256, then uppercase
//!
//! [`dotnet_url_encode`] does steps 3 and 4 together: rather than encode
//! everything and then selectively decode seven escapes back out (as the
//! spec's prose walks through it), it simply never encodes those seven
//! characters — and lowercases the whole result, digits included — which is
//! the same output by construction. `CheckMacValue` itself is excluded from
//! the parameters being signed, in both directions.

use crate::common::{
    core_hand_off_fields, extract_callback_facts, parse_key_value_body, signatures_match,
    CallbackFacts, HandOff, HandOffRequest, PlatformCredentials, QueryAnswer, QueryRequest,
    RefundAnswer, RefundRequest, SignedRequest,
};
use crate::error::ProviderError;
use crate::ProviderAdapter;
use chrono::{DateTime, Utc};
use paygate_domain::ProviderCode;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// .NET's `HttpUtility.UrlEncode`, as ECPay's own SDK relies on: letters,
/// digits and `-_.!*()` pass through untouched; a space becomes `+`;
/// everything else (each byte of the UTF-8 encoding, for anything outside
/// ASCII) becomes a percent-escape. Then the whole string is lowercased —
/// including the letters that were never escaped at all, which is the part a
/// signer that reuses a generic percent-encoder always forgets.
pub fn dotnet_url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + s.len() / 2);
    for byte in s.as_bytes() {
        match byte {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'!'
            | b'*'
            | b'('
            | b')' => {
                out.push(*byte as char);
            }
            b' ' => out.push('+'),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out.make_ascii_lowercase();
    out
}

/// `CheckMacValue`: SHA-256, uppercased, over the sorted-and-wrapped
/// parameter string. `CheckMacValue` itself is never part of `params` when
/// this is used correctly, but it is filtered out defensively all the same.
pub fn check_mac_value(hash_key: &str, hash_iv: &str, params: &BTreeMap<String, String>) -> String {
    let joined = params
        .iter()
        .filter(|(k, _)| k.as_str() != "CheckMacValue")
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&");
    let wrapped = format!("HashKey={hash_key}&{joined}&HashIV={hash_iv}");
    let encoded = dotnet_url_encode(&wrapped);
    let digest = Sha256::digest(encoded.as_bytes());
    hex::encode_upper(digest)
}

fn verify(
    creds: &PlatformCredentials,
    form: &BTreeMap<String, String>,
) -> Result<(), ProviderError> {
    let provided = form
        .get("CheckMacValue")
        .ok_or(ProviderError::MissingField("CheckMacValue"))?;
    let mut signed = form.clone();
    signed.remove("CheckMacValue");
    let expected = check_mac_value(&creds.hash_key, &creds.hash_iv, &signed);
    if signatures_match(&expected, provided) {
        Ok(())
    } else {
        Err(ProviderError::InvalidSignature)
    }
}

/// `TimeStamp`: Unix seconds, as ECPay's `QueryTradeInfo` requires.
pub fn generate_timestamp(now: DateTime<Utc>) -> String {
    now.timestamp().to_string()
}

/// `TimeStamp` is valid for three minutes either side of `now` — a host with
/// a wrong clock cannot query at all (spec.md, "Reconciling what never came
/// back").
pub fn is_timestamp_fresh(timestamp: &str, now: DateTime<Utc>) -> bool {
    const WINDOW_SECONDS: i64 = 180;
    match timestamp.parse::<i64>() {
        Ok(secs) => (now.timestamp() - secs).abs() <= WINDOW_SECONDS,
        Err(_) => false,
    }
}

pub struct EcpayAdapter;

impl ProviderAdapter for EcpayAdapter {
    fn code(&self) -> ProviderCode {
        ProviderCode::Ecpay
    }

    fn hand_off(
        &self,
        creds: &PlatformCredentials,
        req: &HandOffRequest,
    ) -> Result<HandOff, ProviderError> {
        let mut fields = core_hand_off_fields(req);
        fields.insert("PlatformID".to_string(), creds.platform_id.clone());
        fields.insert("PaymentType".to_string(), "aio".to_string());
        fields.insert("ChoosePayment".to_string(), "Credit".to_string());
        fields.insert("EncryptType".to_string(), "1".to_string());
        fields.insert("NeedExtraPaidInfo".to_string(), "Y".to_string());
        let mac = check_mac_value(&creds.hash_key, &creds.hash_iv, &fields);
        fields.insert("CheckMacValue".to_string(), mac);
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
        verify(creds, form)?;
        extract_callback_facts(form, 0)
    }

    fn ack_body(&self) -> &'static str {
        "1|OK"
    }

    fn build_query(&self, creds: &PlatformCredentials, q: &QueryRequest) -> SignedRequest {
        let mut form = BTreeMap::new();
        form.insert("MerchantID".to_string(), q.provider_merchant_id.clone());
        form.insert("PlatformID".to_string(), creds.platform_id.clone());
        form.insert("MerchantTradeNo".to_string(), q.provider_trade_no.clone());
        form.insert("TimeStamp".to_string(), generate_timestamp(Utc::now()));
        let mac = check_mac_value(&creds.hash_key, &creds.hash_iv, &form);
        form.insert("CheckMacValue".to_string(), mac);
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
        verify(creds, &form)?;
        let status = form
            .get("TradeStatus")
            .ok_or(ProviderError::MissingField("TradeStatus"))?;
        match status.as_str() {
            "1" => Ok(QueryAnswer::Paid {
                facts: extract_callback_facts(&form, 1)?,
            }),
            "10200095" => Ok(QueryAnswer::NeverCompleted),
            _ => Ok(QueryAnswer::Unpaid),
        }
    }

    fn build_refund(&self, creds: &PlatformCredentials, r: &RefundRequest) -> SignedRequest {
        let mut form = BTreeMap::new();
        form.insert("MerchantID".to_string(), r.provider_merchant_id.clone());
        form.insert("MerchantTradeNo".to_string(), r.provider_trade_no.clone());
        form.insert("TradeNo".to_string(), r.provider_charge_id.clone());
        form.insert("Action".to_string(), "R".to_string());
        form.insert("TotalAmount".to_string(), r.amount.to_string());
        form.insert("RefundTradeNo".to_string(), r.refund_id.clone());
        let mac = check_mac_value(&creds.hash_key, &creds.hash_iv, &form);
        form.insert("CheckMacValue".to_string(), mac);
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
        if body.trim() == "1|OK" {
            Ok(RefundAnswer::Succeeded)
        } else {
            Ok(RefundAnswer::Failed(body.trim().to_string()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::HandOffRequest;
    use chrono::TimeZone;

    fn creds() -> PlatformCredentials {
        PlatformCredentials {
            platform_id: "3002607".to_string(),
            hash_key: "pwFHCqoQZGmho4w6".to_string(),
            hash_iv: "EkRm7iFT261dpevs".to_string(),
        }
    }

    // ---- dotnet_url_encode: each of the seven restored characters, alone ---

    #[test]
    fn dash_is_left_alone() {
        assert_eq!(dotnet_url_encode("-"), "-");
    }
    #[test]
    fn underscore_is_left_alone() {
        assert_eq!(dotnet_url_encode("_"), "_");
    }
    #[test]
    fn dot_is_left_alone() {
        assert_eq!(dotnet_url_encode("."), ".");
    }
    #[test]
    fn bang_is_left_alone() {
        assert_eq!(dotnet_url_encode("!"), "!");
    }
    #[test]
    fn star_is_left_alone() {
        assert_eq!(dotnet_url_encode("*"), "*");
    }
    #[test]
    fn open_paren_is_left_alone() {
        assert_eq!(dotnet_url_encode("("), "(");
    }
    #[test]
    fn close_paren_is_left_alone() {
        assert_eq!(dotnet_url_encode(")"), ")");
    }

    #[test]
    fn space_becomes_plus() {
        assert_eq!(dotnet_url_encode(" "), "+");
    }

    #[test]
    fn everything_gets_lowercased_letters_included() {
        assert_eq!(dotnet_url_encode("ABCxyz123"), "abcxyz123");
    }

    #[test]
    fn characters_outside_the_safe_set_are_percent_escaped_lowercase() {
        assert_eq!(dotnet_url_encode("&"), "%26");
        assert_eq!(dotnet_url_encode("="), "%3d");
        assert_eq!(dotnet_url_encode("/"), "%2f");
        assert_eq!(dotnet_url_encode("%"), "%25");
    }

    #[test]
    fn multibyte_utf8_is_escaped_byte_by_byte() {
        // Cross-checked against Python's urllib.parse.quote_plus(s,
        // safe='-_.!*()').lower() — an independent implementation of the
        // same specified behaviour.
        assert_eq!(
            dotnet_url_encode("Ünïcode ☕"),
            "%c3%9cn%c3%afcode+%e2%98%95"
        );
    }

    /// ECPay's own published `AioCheckOut` V5 example, digest and all. This is
    /// the external anchor `spec.md` asks for: nothing in this repository
    /// produced the expected value, ECPay's documentation did, so the test
    /// fails if this implementation drifts from the real thing rather than
    /// merely from itself.
    ///
    /// It is also the vector that exercises the two details the algorithm is
    /// famous for losing. `ItemName` and `TradeDesc` are Chinese, so every
    /// byte of their UTF-8 encoding has to be escaped separately; and
    /// `MerchantTradeDate` contains spaces, which .NET's encoder turns into
    /// `+` rather than `%20`. Get either wrong and the digest below does not
    /// appear.
    #[test]
    fn check_mac_value_matches_ecpays_published_aiocheckout_example() {
        let mut params = BTreeMap::new();
        for (k, v) in [
            ("ChoosePayment", "ALL"),
            ("EncryptType", "1"),
            ("ItemName", "Apple iphone 7 手機殼"),
            ("MerchantID", "2000132"),
            ("MerchantTradeDate", "2018/05/04 11:07:23"),
            ("MerchantTradeNo", "ecpay20180504110723"),
            ("PaymentType", "aio"),
            ("ReturnURL", "https://www.ecpay.com.tw/receive.php"),
            ("TotalAmount", "1000"),
            ("TradeDesc", "促銷方案"),
        ] {
            params.insert(k.to_string(), v.to_string());
        }

        assert_eq!(
            check_mac_value("5294y06JbISpM5x9", "v77hoKGq4kWxNNIS", &params),
            "B4A5010C622CC8710182465D1A8CFFF29B9212264E679C8468893C4A6EBB716B"
        );
    }

    /// ECPay's second published example, from the checksum page of its
    /// developer documentation. That one signs a JSON document rather than a
    /// sorted parameter list, so it does not go through `check_mac_value` —
    /// it goes through the two steps underneath it, which is exactly why it is
    /// worth having: it pins `dotnet_url_encode` and the hash independently of
    /// how the parameters happened to be assembled.
    ///
    /// ECPay publishes the intermediate string as well, so this asserts that
    /// too. If the encoder ever stops lowercasing the characters it did NOT
    /// escape, this is the test that says so.
    #[test]
    fn dotnet_url_encode_matches_ecpays_published_checksum_example() {
        let plaintext = concat!(
            "7b53896b742849d3",
            r#"{"MerchantID":"3085676","MerchantTradeNo":"CX202202221540568521"}"#,
            "37a0ad3c6ffa428b"
        );

        let encoded = dotnet_url_encode(plaintext);
        assert_eq!(
            encoded,
            concat!(
                "7b53896b742849d3%7b%22merchantid%22%3a%223085676%22%2c",
                "%22merchanttradeno%22%3a%22cx202202221540568521%22%7d",
                "37a0ad3c6ffa428b"
            )
        );
        assert_eq!(
            hex::encode_upper(Sha256::digest(encoded.as_bytes())),
            "CE67BBD259EE38BA1C7FB7CC88C3BD91D3F082B46EAEBD4E4E5F2184CB23349A"
        );
    }

    /// A second, local vector over the parameter set the BDD fixtures actually
    /// seed (`spec/bdd/api/handoff.feature`'s platform `HashKey`/`HashIV` and
    /// `PlatformID`). The two ECPay vectors above prove the algorithm; this one
    /// documents the digest the provider mock verifies in this suite, so a
    /// change to how the hand-off is assembled shows up here as well as in a
    /// scenario.
    #[test]
    fn check_mac_value_matches_the_independently_cross_checked_vector() {
        let mut params = BTreeMap::new();
        params.insert("MerchantID".to_string(), "2000132".to_string());
        params.insert("PlatformID".to_string(), "3002607".to_string());
        params.insert(
            "MerchantTradeNo".to_string(),
            "ACME00000001SN4f2a1".to_string(),
        );
        params.insert(
            "MerchantTradeDate".to_string(),
            "2026/09/21 14:03:11".to_string(),
        );
        params.insert("PaymentType".to_string(), "aio".to_string());
        params.insert("TotalAmount".to_string(), "1250".to_string());
        params.insert("TradeDesc".to_string(), "Single origin beans".to_string());
        params.insert("ItemName".to_string(), "Single origin beans".to_string());
        params.insert(
            "ReturnURL".to_string(),
            "https://paygate.example.com/api/v1/webhooks/ecpay".to_string(),
        );
        params.insert("ClientBackURL".to_string(), "/shop/result".to_string());
        params.insert("ChoosePayment".to_string(), "Credit".to_string());
        params.insert("EncryptType".to_string(), "1".to_string());
        params.insert("NeedExtraPaidInfo".to_string(), "Y".to_string());

        let mac = check_mac_value("pwFHCqoQZGmho4w6", "EkRm7iFT261dpevs", &params);
        assert_eq!(
            mac,
            "369511A42530A1F51B6411B5CD18A1F9DCEFD6F79F5C0C48F2473C5616DE13A1"
        );
    }

    #[test]
    fn check_mac_value_excludes_itself_from_what_it_signs() {
        let mut params = BTreeMap::new();
        params.insert("A".to_string(), "1".to_string());
        params.insert("B".to_string(), "2".to_string());
        let without = check_mac_value("key", "iv", &params);
        params.insert(
            "CheckMacValue".to_string(),
            "whatever-was-here-before".to_string(),
        );
        let with = check_mac_value("key", "iv", &params);
        assert_eq!(without, with);
    }

    // ---- the platform's key pair, not the merchant's -----------------------

    #[test]
    fn hand_off_signs_with_the_platforms_key_pair_not_a_merchants() {
        let req = HandOffRequest {
            cashier_url: "/provider/ecpay/Cashier/AioCheckOut/V5".to_string(),
            provider_merchant_id: "2000132".to_string(),
            provider_trade_no: "ACME00000001SN4f2a1".to_string(),
            trade_date: Utc.with_ymd_and_hms(2026, 9, 21, 14, 3, 11).unwrap(),
            amount: 1250,
            item_desc: "Beans".to_string(),
            return_url: "https://paygate.example.com/api/v1/webhooks/ecpay".to_string(),
            client_back_url: "/shop/result".to_string(),
        };
        let handoff = EcpayAdapter.hand_off(&creds(), &req).unwrap();
        assert_eq!(
            handoff.fields.get("MerchantID"),
            Some(&"2000132".to_string())
        );
        assert_eq!(
            handoff.fields.get("PlatformID"),
            Some(&"3002607".to_string())
        );
        assert_eq!(handoff.fields.get("PaymentType"), Some(&"aio".to_string()));
        assert_eq!(
            handoff.fields.get("ChoosePayment"),
            Some(&"Credit".to_string())
        );
        assert_eq!(handoff.fields.get("EncryptType"), Some(&"1".to_string()));
        assert_eq!(
            handoff.fields.get("NeedExtraPaidInfo"),
            Some(&"Y".to_string())
        );
        assert_eq!(handoff.action, "/provider/ecpay/Cashier/AioCheckOut/V5");

        // Signed with the PLATFORM's pair: verifying with the platform's own
        // credentials succeeds...
        assert!(verify(&creds(), &handoff.fields).is_ok());
        // ...but a different key pair (what a merchant's own would be) does not.
        let wrong = PlatformCredentials {
            platform_id: "3002607".to_string(),
            hash_key: "someMerchantsOwnHashKey".to_string(),
            hash_iv: "someMerchantsOwnHashIV0".to_string(),
        };
        assert!(verify(&wrong, &handoff.fields).is_err());
    }

    // ---- callback verification ----------------------------------------------

    fn signed_callback_form(creds: &PlatformCredentials) -> BTreeMap<String, String> {
        let mut form = BTreeMap::new();
        form.insert("MerchantID".to_string(), "2000132".to_string());
        form.insert(
            "MerchantTradeNo".to_string(),
            "ACME00000001SN4f2a1".to_string(),
        );
        form.insert("TradeNo".to_string(), "2109210000000".to_string());
        form.insert("RtnCode".to_string(), "1".to_string());
        form.insert("RtnMsg".to_string(), "Succeeded".to_string());
        form.insert("TradeAmt".to_string(), "1250".to_string());
        form.insert("PaymentDate".to_string(), "2026/09/21 14:03:11".to_string());
        form.insert("SimulatePaid".to_string(), "0".to_string());
        form.insert("card4no".to_string(), "4242".to_string());
        form.insert("card6no".to_string(), "424242".to_string());
        form.insert("eci".to_string(), "05".to_string());
        form.insert("auth_code".to_string(), "777888".to_string());
        form.insert("event_id".to_string(), "evt-1".to_string());
        let mac = check_mac_value(&creds.hash_key, &creds.hash_iv, &form);
        form.insert("CheckMacValue".to_string(), mac);
        form
    }

    #[test]
    fn a_correctly_signed_callback_verifies_and_parses() {
        let form = signed_callback_form(&creds());
        let facts = EcpayAdapter.parse_callback(&creds(), &form).unwrap();
        assert_eq!(facts.provider_trade_no, "ACME00000001SN4f2a1");
        assert_eq!(facts.provider_charge_id, "2109210000000");
        assert_eq!(facts.provider_event_id, "evt-1");
        assert_eq!(facts.rtn_code, 1);
        assert_eq!(facts.amount, 1250);
        assert_eq!(facts.card_brand.as_deref(), Some("visa"));
        assert_eq!(facts.card_last4.as_deref(), Some("4242"));
    }

    #[test]
    fn a_callback_signed_with_the_wrong_key_is_refused() {
        let mut form = signed_callback_form(&creds());
        // Forged: signed with a key this provider never issued.
        let forged = PlatformCredentials {
            platform_id: "3002607".to_string(),
            hash_key: "notThePlatformsRealKey0".to_string(),
            hash_iv: "notThePlatformsRealIV00".to_string(),
        };
        form.insert(
            "CheckMacValue".to_string(),
            check_mac_value(&forged.hash_key, &forged.hash_iv, &form),
        );
        assert_eq!(
            EcpayAdapter.parse_callback(&creds(), &form),
            Err(ProviderError::InvalidSignature)
        );
    }

    #[test]
    fn a_tampered_amount_after_signing_is_refused() {
        let mut form = signed_callback_form(&creds());
        form.insert("TradeAmt".to_string(), "999999".to_string());
        assert_eq!(
            EcpayAdapter.parse_callback(&creds(), &form),
            Err(ProviderError::InvalidSignature)
        );
    }

    #[test]
    fn a_truncated_body_is_refused() {
        let mut form = signed_callback_form(&creds());
        form.remove("RtnMsg");
        assert_eq!(
            EcpayAdapter.parse_callback(&creds(), &form),
            Err(ProviderError::InvalidSignature)
        );
    }

    #[test]
    fn a_reordered_but_otherwise_identical_body_still_verifies() {
        // BTreeMap already stores by sorted key, so "re-ordering" the wire
        // form changes nothing about what is signed — which is the point of
        // sorting by name before joining.
        let form = signed_callback_form(&creds());
        let mut reinserted = BTreeMap::new();
        for (k, v) in form.iter().rev() {
            reinserted.insert(k.clone(), v.clone());
        }
        assert!(EcpayAdapter.parse_callback(&creds(), &reinserted).is_ok());
    }

    #[test]
    fn a_body_missing_check_mac_value_entirely_is_refused() {
        let mut form = signed_callback_form(&creds());
        form.remove("CheckMacValue");
        assert_eq!(
            EcpayAdapter.parse_callback(&creds(), &form),
            Err(ProviderError::MissingField("CheckMacValue"))
        );
    }

    #[test]
    fn ack_body_is_exactly_1_ok() {
        assert_eq!(EcpayAdapter.ack_body(), "1|OK");
    }

    // ---- query -----------------------------------------------------------

    #[test]
    fn build_query_is_signed_and_carries_the_platform_id() {
        let q = QueryRequest {
            query_url: "/provider/ecpay/Cashier/QueryTradeInfo/V5".to_string(),
            provider_merchant_id: "2000132".to_string(),
            provider_trade_no: "ACME00000001SN4f2a1".to_string(),
        };
        let signed = EcpayAdapter.build_query(&creds(), &q);
        assert_eq!(signed.url_path, "/provider/ecpay/Cashier/QueryTradeInfo/V5");
        assert_eq!(signed.form.get("PlatformID"), Some(&"3002607".to_string()));
        assert!(is_timestamp_fresh(
            signed.form.get("TimeStamp").unwrap(),
            Utc::now()
        ));
        assert!(verify(&creds(), &signed.form).is_ok());
    }

    #[test]
    fn timestamp_is_fresh_within_three_minutes_either_side() {
        let now = Utc.with_ymd_and_hms(2026, 1, 1, 12, 0, 0).unwrap();
        assert!(is_timestamp_fresh(
            &(now.timestamp() - 179).to_string(),
            now
        ));
        assert!(is_timestamp_fresh(
            &(now.timestamp() + 179).to_string(),
            now
        ));
        assert!(!is_timestamp_fresh(
            &(now.timestamp() - 181).to_string(),
            now
        ));
        assert!(!is_timestamp_fresh(
            &(now.timestamp() + 181).to_string(),
            now
        ));
        assert!(!is_timestamp_fresh("not-a-number", now));
    }

    #[test]
    fn parse_query_answer_paid() {
        let mut answer = BTreeMap::new();
        answer.insert("MerchantID".to_string(), "2000132".to_string());
        answer.insert(
            "MerchantTradeNo".to_string(),
            "ACME00000001SN4f2a1".to_string(),
        );
        answer.insert("TradeStatus".to_string(), "1".to_string());
        answer.insert("TradeAmt".to_string(), "2500".to_string());
        answer.insert("PaymentDate".to_string(), "2026/09/21 14:03:11".to_string());
        let mac = check_mac_value(&creds().hash_key, &creds().hash_iv, &answer);
        answer.insert("CheckMacValue".to_string(), mac);
        let body = answer
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("&");

        match EcpayAdapter.parse_query_answer(&creds(), &body).unwrap() {
            QueryAnswer::Paid { facts } => {
                assert_eq!(facts.amount, 2500);
                assert_eq!(facts.rtn_code, 1);
            }
            other => panic!("expected Paid, got {other:?}"),
        }
    }

    #[test]
    fn parse_query_answer_never_completed() {
        let mut answer = BTreeMap::new();
        answer.insert("MerchantID".to_string(), "2000132".to_string());
        answer.insert(
            "MerchantTradeNo".to_string(),
            "ACME00000001SN4f2a1".to_string(),
        );
        answer.insert("TradeStatus".to_string(), "10200095".to_string());
        let mac = check_mac_value(&creds().hash_key, &creds().hash_iv, &answer);
        answer.insert("CheckMacValue".to_string(), mac);
        let body = answer
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("&");

        assert_eq!(
            EcpayAdapter.parse_query_answer(&creds(), &body).unwrap(),
            QueryAnswer::NeverCompleted
        );
    }

    #[test]
    fn parse_query_answer_still_in_progress() {
        let mut answer = BTreeMap::new();
        answer.insert("MerchantID".to_string(), "2000132".to_string());
        answer.insert(
            "MerchantTradeNo".to_string(),
            "ACME00000001SN4f2a1".to_string(),
        );
        answer.insert("TradeStatus".to_string(), "0".to_string());
        let mac = check_mac_value(&creds().hash_key, &creds().hash_iv, &answer);
        answer.insert("CheckMacValue".to_string(), mac);
        let body = answer
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("&");

        assert_eq!(
            EcpayAdapter.parse_query_answer(&creds(), &body).unwrap(),
            QueryAnswer::Unpaid
        );
    }

    #[test]
    fn a_forged_query_answer_is_not_an_answer() {
        let mut answer = BTreeMap::new();
        answer.insert("MerchantID".to_string(), "2000132".to_string());
        answer.insert(
            "MerchantTradeNo".to_string(),
            "ACME00000001SN4f2a1".to_string(),
        );
        answer.insert("TradeStatus".to_string(), "1".to_string());
        let forged = check_mac_value("not-the-real-key", "not-the-real-iv", &answer);
        answer.insert("CheckMacValue".to_string(), forged);
        let body = answer
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("&");

        assert_eq!(
            EcpayAdapter.parse_query_answer(&creds(), &body),
            Err(ProviderError::InvalidSignature)
        );
    }

    // ---- refund ------------------------------------------------------------

    #[test]
    fn build_refund_carries_paygates_own_refund_id_as_the_dedup_key() {
        let r = RefundRequest {
            refund_url: "/provider/ecpay/CreditDetail/DoAction".to_string(),
            provider_merchant_id: "2000132".to_string(),
            provider_trade_no: "ACME00000001SN4f2a1".to_string(),
            provider_charge_id: "2109210000000".to_string(),
            amount: 500,
            refund_id: "01931c4f-0000-7000-8000-00000000001a".to_string(),
        };
        let signed = EcpayAdapter.build_refund(&creds(), &r);
        assert_eq!(signed.form.get("Action"), Some(&"R".to_string()));
        assert_eq!(
            signed.form.get("RefundTradeNo"),
            Some(&"01931c4f-0000-7000-8000-00000000001a".to_string())
        );
        assert_eq!(signed.form.get("TotalAmount"), Some(&"500".to_string()));
        assert!(verify(&creds(), &signed.form).is_ok());
    }

    #[test]
    fn parse_refund_answer_requires_the_exact_body() {
        assert_eq!(
            EcpayAdapter.parse_refund_answer(&creds(), "1|OK").unwrap(),
            RefundAnswer::Succeeded
        );
        assert_eq!(
            EcpayAdapter
                .parse_refund_answer(&creds(), "0|CheckMacValue Error")
                .unwrap(),
            RefundAnswer::Failed("0|CheckMacValue Error".to_string())
        );
    }
}
