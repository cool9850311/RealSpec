//! The types shared by both adapters, and the field vocabulary they agree
//! on internally.
//!
//! **A deliberate design choice, spelled out because nothing in
//! `spec/openapi/payment-provider.yaml` fixes it**: NewebPay's `TradeInfo` is
//! opaque outside this crate (it is ciphertext on the wire), so nothing pins
//! the *names* of the fields packed inside it the way ECPay's cleartext form
//! is pinned by `EcpayCheckoutForm`. Rather than invent a second, parallel
//! vocabulary, both adapters build and read the exact same field names —
//! `MerchantID`, `MerchantTradeNo`, `MerchantTradeDate`, `TotalAmount`,
//! `TradeDesc`, `ItemName`, `ReturnURL`, `ClientBackURL` for the hand-off, and
//! `RtnCode`, `RtnMsg`, `TradeAmt`, `TradeNo`, `PaymentDate`, `SimulatePaid`,
//! `card4no`, `card6no`, `eci`, `auth_code`, `failure_code`, `event_id` for
//! the callback/query answer. The two adapters differ only in **how** that
//! vocabulary is wrapped — clear text signed with `CheckMacValue`, or
//! encrypted and signed with `TradeSha` — which is exactly the point spec.md
//! makes: "What paygate does with both is identical... which is the point of
//! having two." Anything that builds a provider-mock or a demo-merchant
//! against this crate should read the field names from here, not reverse
//! either adapter.
//!
//! **A second, additive field beyond `CallbackFacts`'s minimal wire shape**:
//! a `failure_code: Option<String>` field. `classify_rtn_code`'s own
//! documented mapping only has two known failure codes
//! (`10100058`, `10200163`), but `handoff.feature`'s declined-card scenarios
//! need three distinct, semantic failure reasons
//! (`card_declined` / `insufficient_funds` / `three_ds_failed`) stored in
//! `payment_attempts.failure_code`. `payment-provider.yaml`'s own
//! `QueuedCallback` schema already carries a dedicated `failure_code` field
//! for exactly this, independent of `rtn_code` — so this crate passes it
//! through verbatim when the wire body carries one, and callers fall back to
//! `RtnClass::Failed`'s code-as-string only when it does not.

use chrono::{DateTime, Utc};
use paygate_domain::Amount;
use std::collections::BTreeMap;

/// paygate's own platform credentials at one provider — never a merchant's.
/// See spec.md, "Platform, not merchant of record".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlatformCredentials {
    pub platform_id: String,
    pub hash_key: String,
    pub hash_iv: String,
}

/// The signed form the merchant's page must submit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandOff {
    pub action: String,
    pub fields: BTreeMap<String, String>,
}

/// Everything an adapter needs to build and sign a hand-off. `cashier_url`
/// is `providers.cashier_url`, echoed back unchanged as [`HandOff::action`]
/// — paygate never rewrites it (`spec.md`, "Environment variables":
/// `providers.cashier_url` is a path made absolute against
/// `PROVIDER_BASE_URL`, not rewritten).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandOffRequest {
    pub cashier_url: String,
    pub provider_merchant_id: String,
    pub provider_trade_no: String,
    pub trade_date: DateTime<Utc>,
    pub amount: Amount,
    pub item_desc: String,
    pub return_url: String,
    pub client_back_url: String,
}

/// Everything an adapter needs to ask a provider what became of an attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryRequest {
    pub query_url: String,
    pub provider_merchant_id: String,
    pub provider_trade_no: String,
}

/// Everything an adapter needs to ask a provider for a refund.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefundRequest {
    pub refund_url: String,
    pub provider_merchant_id: String,
    pub provider_trade_no: String,
    pub provider_charge_id: String,
    pub amount: Amount,
    /// paygate's own refund id — the number the provider deduplicates a
    /// retry on (spec.md, "Refunds").
    pub refund_id: String,
}

/// A request ready to be made absolute against `PROVIDER_BASE_URL` and POSTed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedRequest {
    pub url_path: String,
    pub form: BTreeMap<String, String>,
}

/// What a refund call answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefundAnswer {
    Succeeded,
    /// The provider's own reason text.
    Failed(String),
}

/// The facts of a settlement, verified and parsed out of a callback or a
/// query's `Paid` answer. Never a card number — the provider gives back a
/// brand and four digits, nothing else (spec.md, "The card never enters this
/// system").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallbackFacts {
    pub provider_trade_no: String,
    pub provider_event_id: String,
    pub provider_charge_id: String,
    pub rtn_code: i32,
    pub rtn_msg: String,
    pub amount: Amount,
    pub simulate_paid: bool,
    pub failure_code: Option<String>,
    pub card_brand: Option<String>,
    pub card_last4: Option<String>,
    pub eci: Option<String>,
    pub auth_code: Option<String>,
    pub paid_at: Option<DateTime<Utc>>,
}

/// What `classify_rtn_code` makes of an `RtnCode`. The set of codes is open
/// (spec.md, "Being told by the provider") — an unrecognised one is
/// `Unknown`, never an error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RtnClass {
    Paid,
    Failed(String),
    PendingConfirmation,
    Unknown,
}

/// `1` paid; `10100058` / `10200163` failed; `10300066` pending confirmation
/// (do not ship); anything else unknown. `provider_events` has no
/// `pending_confirmation` outcome of its own — a caller records that the
/// same way as `Unknown`, `'unknown_code'`, acted on in no way
/// (`webhooks.feature`, "A return code this gateway has never seen").
pub fn classify_rtn_code(code: i32) -> RtnClass {
    match code {
        1 => RtnClass::Paid,
        10_100_058 | 10_200_163 => RtnClass::Failed(code.to_string()),
        10_300_066 => RtnClass::PendingConfirmation,
        _ => RtnClass::Unknown,
    }
}

/// What asking `QueryTradeInfo` (or its NewebPay equivalent) came back with.
// This shape is pinned exactly (`Paid { facts: CallbackFacts }`, not
// `Box<CallbackFacts>`) since `crates/infra/src/reconciler.rs` is written
// against it; the size difference clippy flags is one `QueryAnswer` per
// query answer, not a hot per-request allocation, so the lint is suppressed
// rather than the signature changed.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryAnswer {
    /// `TradeStatus = 0`: created, still unpaid.
    Unpaid,
    /// `TradeStatus = 1`: paid, late.
    Paid { facts: CallbackFacts },
    /// `TradeStatus = 10200095`: the consumer never completed it.
    NeverCompleted,
}

// ---- the shared field vocabulary ------------------------------------------

/// The fields both adapters agree the hand-off carries, before either one
/// wraps them (see the module doc for why the vocabulary is shared).
pub(crate) fn core_hand_off_fields(req: &HandOffRequest) -> BTreeMap<String, String> {
    let mut fields = BTreeMap::new();
    fields.insert("MerchantID".to_string(), req.provider_merchant_id.clone());
    fields.insert("MerchantTradeNo".to_string(), req.provider_trade_no.clone());
    fields.insert(
        "MerchantTradeDate".to_string(),
        format_provider_datetime(req.trade_date),
    );
    fields.insert("TotalAmount".to_string(), req.amount.to_string());
    fields.insert("TradeDesc".to_string(), req.item_desc.clone());
    fields.insert("ItemName".to_string(), req.item_desc.clone());
    fields.insert("ReturnURL".to_string(), req.return_url.clone());
    fields.insert("ClientBackURL".to_string(), req.client_back_url.clone());
    fields
}

fn parse_amount(
    form: &BTreeMap<String, String>,
    key: &'static str,
) -> Result<Amount, crate::ProviderError> {
    match form.get(key) {
        Some(raw) => raw
            .parse::<Amount>()
            .map_err(|_| crate::ProviderError::MalformedField(key, raw.clone())),
        None => Ok(0),
    }
}

/// Classifies a card's brand from the leading digits ECPay/NewebPay give
/// back (`card6no`, or the first digits of `card4no`'s companion BIN).
/// paygate never sees the card itself — only these leading digits, from the
/// provider's own callback (spec.md, "The card never enters this system").
pub(crate) fn classify_card_brand(bin: &str) -> Option<String> {
    let bin = bin.trim();
    if bin.is_empty() {
        return None;
    }
    if bin.starts_with('4') {
        return Some("visa".to_string());
    }
    if let Some(two) = bin.get(0..2).and_then(|s| s.parse::<u32>().ok()) {
        if (51..=55).contains(&two) {
            return Some("mastercard".to_string());
        }
    }
    if bin.starts_with("34") || bin.starts_with("37") {
        return Some("amex".to_string());
    }
    if bin.starts_with("6011") || bin.starts_with("65") {
        return Some("discover".to_string());
    }
    None
}

fn non_empty(form: &BTreeMap<String, String>, key: &str) -> Option<String> {
    form.get(key)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Turns a wire-level field map — ECPay's cleartext form, or NewebPay's
/// decrypted `TradeInfo` — into [`CallbackFacts`]. Shared by both adapters'
/// `parse_callback` and the `Paid` branch of `parse_query_answer`, because
/// (by design — see the module doc) they read the same field names.
///
/// `rtn_code_if_absent` covers `QueryTradeInfo`'s answer, which carries
/// `TradeStatus` rather than `RtnCode`: the caller already knows the
/// classification from `TradeStatus` and supplies the equivalent `RtnCode`
/// (`1`, for the `Paid` branch) rather than requiring the field twice over.
pub(crate) fn extract_callback_facts(
    form: &BTreeMap<String, String>,
    rtn_code_if_absent: i32,
) -> Result<CallbackFacts, crate::ProviderError> {
    let provider_trade_no = form
        .get("MerchantTradeNo")
        .cloned()
        .ok_or(crate::ProviderError::MissingField("MerchantTradeNo"))?;
    let amount = parse_amount(form, "TradeAmt")?;
    let rtn_code = match form.get("RtnCode") {
        Some(raw) => raw
            .parse::<i32>()
            .map_err(|_| crate::ProviderError::MalformedField("RtnCode", raw.clone()))?,
        None => rtn_code_if_absent,
    };
    let rtn_msg = form.get("RtnMsg").cloned().unwrap_or_default();
    let provider_charge_id = form.get("TradeNo").cloned().unwrap_or_default();
    let provider_event_id =
        non_empty(form, "event_id").unwrap_or_else(|| provider_charge_id.clone());
    let simulate_paid = form.get("SimulatePaid").map(|s| s == "1").unwrap_or(false);
    let failure_code = non_empty(form, "failure_code");
    let card_last4 = non_empty(form, "card4no");
    let card_brand = non_empty(form, "card6no").and_then(|bin| classify_card_brand(&bin));
    let eci = non_empty(form, "eci");
    let auth_code = non_empty(form, "auth_code");
    let paid_at = form
        .get("PaymentDate")
        .and_then(|s| parse_provider_datetime(s));

    Ok(CallbackFacts {
        provider_trade_no,
        provider_event_id,
        provider_charge_id,
        rtn_code,
        rtn_msg,
        amount,
        simulate_paid,
        failure_code,
        card_brand,
        card_last4,
        eci,
        auth_code,
        paid_at,
    })
}

/// ECPay's (and, by the shared vocabulary above, NewebPay's) own date shape:
/// `2026/09/21 14:03:11`, in Taiwan local time.
pub(crate) fn parse_provider_datetime(s: &str) -> Option<DateTime<Utc>> {
    use chrono::TimeZone;
    let naive = chrono::NaiveDateTime::parse_from_str(s, "%Y/%m/%d %H:%M:%S").ok()?;
    chrono_tz::Asia::Taipei
        .from_local_datetime(&naive)
        .single()
        .map(|dt| dt.with_timezone(&Utc))
}

pub(crate) fn format_provider_datetime(dt: DateTime<Utc>) -> String {
    dt.format("%Y/%m/%d %H:%M:%S").to_string()
}

/// The `SimulatePaid` guard, as a pure function. ECPay's back office can
/// send a notification that is identical to a real one except for
/// `SimulatePaid=1`, and it is set independently of `RtnCode` — so this
/// checks both, in order, rather than trusting `RtnCode` alone: a callback
/// only settles a payment when it both classifies as paid AND was not
/// simulated. `webhooks.feature`'s "SimulatePaid overrides even a genuine
/// decline" scenario is the reason for checking `simulate_paid` at all
/// (a callback that would have been a `Failed` anyway proves nothing about
/// the guard); this function is what a caller runs before ever asking
/// `apply` for a [`paygate_domain::Command::Settle`].
pub fn should_settle(facts: &CallbackFacts) -> bool {
    !facts.simulate_paid && classify_rtn_code(facts.rtn_code) == RtnClass::Paid
}

/// A plain `key=value&key=value…` body — `QueryTradeInfo`'s answer shape,
/// not URL-decoded (ECPay's own example shows a literal space inside
/// `PaymentDate`, so this is not `application/x-www-form-urlencoded`).
pub(crate) fn parse_key_value_body(body: &str) -> BTreeMap<String, String> {
    body.split('&')
        .filter_map(|pair| pair.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// Constant-time, case-insensitive comparison of two signatures. Refuses a
/// tampered, truncated or re-ordered body without leaking the answer through
/// timing (`spec.md`, "Authentication").
pub(crate) fn signatures_match(expected: &str, provided: &str) -> bool {
    use subtle::ConstantTimeEq;
    let expected = expected.to_ascii_uppercase();
    let provided = provided.to_ascii_uppercase();
    if expected.len() != provided.len() {
        return false;
    }
    expected.as_bytes().ct_eq(provided.as_bytes()).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brand_classification_from_the_test_cards_bin() {
        assert_eq!(classify_card_brand("424242"), Some("visa".to_string()));
        assert_eq!(classify_card_brand("400000"), Some("visa".to_string()));
        assert_eq!(
            classify_card_brand("510000"),
            Some("mastercard".to_string())
        );
        assert_eq!(classify_card_brand("340000"), Some("amex".to_string()));
        assert_eq!(classify_card_brand("601100"), Some("discover".to_string()));
        assert_eq!(classify_card_brand(""), None);
        assert_eq!(classify_card_brand("999999"), None);
    }

    #[test]
    fn signatures_match_is_case_insensitive_but_exact() {
        assert!(signatures_match("abc123", "ABC123"));
        assert!(!signatures_match("abc123", "abc124"));
        assert!(!signatures_match("abc123", "abc12"));
        assert!(!signatures_match("abc123", "abc1234"));
    }

    #[test]
    fn classify_rtn_code_matches_the_documented_taxonomy() {
        assert_eq!(classify_rtn_code(1), RtnClass::Paid);
        assert_eq!(
            classify_rtn_code(10_100_058),
            RtnClass::Failed("10100058".to_string())
        );
        assert_eq!(
            classify_rtn_code(10_200_163),
            RtnClass::Failed("10200163".to_string())
        );
        assert_eq!(classify_rtn_code(10_300_066), RtnClass::PendingConfirmation);
        assert_eq!(classify_rtn_code(0), RtnClass::Unknown);
        assert_eq!(classify_rtn_code(-1), RtnClass::Unknown);
        assert_eq!(classify_rtn_code(999_999), RtnClass::Unknown);
    }

    #[test]
    fn extract_callback_facts_reads_the_shared_vocabulary() {
        let mut form = BTreeMap::new();
        form.insert(
            "MerchantTradeNo".to_string(),
            "ACME00000001SN4f2a1".to_string(),
        );
        form.insert("TradeNo".to_string(), "2109210000000".to_string());
        form.insert("RtnCode".to_string(), "1".to_string());
        form.insert("RtnMsg".to_string(), "Succeeded".to_string());
        form.insert("TradeAmt".to_string(), "2500".to_string());
        form.insert("SimulatePaid".to_string(), "0".to_string());
        form.insert("card4no".to_string(), "4242".to_string());
        form.insert("card6no".to_string(), "424242".to_string());
        form.insert("eci".to_string(), "05".to_string());
        form.insert("auth_code".to_string(), "777888".to_string());
        form.insert("PaymentDate".to_string(), "2026/09/21 14:03:11".to_string());

        let facts = extract_callback_facts(&form, 1).unwrap();
        assert_eq!(facts.provider_trade_no, "ACME00000001SN4f2a1");
        assert_eq!(facts.provider_charge_id, "2109210000000");
        assert_eq!(facts.provider_event_id, "2109210000000");
        assert_eq!(facts.rtn_code, 1);
        assert_eq!(facts.amount, 2500);
        assert!(!facts.simulate_paid);
        assert_eq!(facts.card_brand.as_deref(), Some("visa"));
        assert_eq!(facts.card_last4.as_deref(), Some("4242"));
        assert_eq!(facts.eci.as_deref(), Some("05"));
        assert_eq!(facts.auth_code.as_deref(), Some("777888"));
        assert!(facts.paid_at.is_some());
    }

    #[test]
    fn extract_callback_facts_requires_merchant_trade_no() {
        let form = BTreeMap::new();
        assert_eq!(
            extract_callback_facts(&form, 1),
            Err(crate::ProviderError::MissingField("MerchantTradeNo"))
        );
    }

    fn facts(rtn_code: i32, simulate_paid: bool) -> CallbackFacts {
        CallbackFacts {
            provider_trade_no: "ACME00000001SN4f2a1".to_string(),
            provider_event_id: "evt-1".to_string(),
            provider_charge_id: "charge-1".to_string(),
            rtn_code,
            rtn_msg: String::new(),
            amount: 1250,
            simulate_paid,
            failure_code: None,
            card_brand: None,
            card_last4: None,
            eci: None,
            auth_code: None,
            paid_at: None,
        }
    }

    #[test]
    fn should_settle_is_true_only_for_a_genuine_paid_callback() {
        assert!(should_settle(&facts(1, false)));
    }

    #[test]
    fn should_settle_guards_against_simulate_paid_even_on_an_otherwise_paid_code() {
        assert!(!should_settle(&facts(1, true)));
    }

    #[test]
    fn should_settle_is_false_for_a_decline_whether_or_not_it_is_simulated() {
        assert!(!should_settle(&facts(10_100_058, false)));
        assert!(!should_settle(&facts(10_100_058, true)));
    }

    #[test]
    fn should_settle_is_false_for_an_unknown_or_pending_code() {
        assert!(!should_settle(&facts(10_300_066, false)));
        assert!(!should_settle(&facts(999_999, false)));
    }

    #[test]
    fn parse_key_value_body_splits_on_ampersand_and_first_equals() {
        let form = parse_key_value_body(
            "MerchantID=2000132&TradeStatus=1&PaymentDate=2026/09/21 14:03:11",
        );
        assert_eq!(form.get("MerchantID"), Some(&"2000132".to_string()));
        assert_eq!(form.get("TradeStatus"), Some(&"1".to_string()));
        assert_eq!(
            form.get("PaymentDate"),
            Some(&"2026/09/21 14:03:11".to_string())
        );
    }
}
