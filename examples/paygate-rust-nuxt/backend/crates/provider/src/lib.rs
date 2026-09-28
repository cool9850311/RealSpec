//! `paygate-provider` — the ECPay and NewebPay codecs. Everything a hand-off,
//! a callback, a query or a refund needs to be built, signed, verified and
//! parsed, with no network call anywhere in this crate: what to POST and
//! where is a [`SignedRequest`] or a [`HandOff`], and making the call is
//! `infra`'s job.
//!
//! Two adapters, one trait ([`ProviderAdapter`]), because — spec.md,
//! "Choosing a provider" — what paygate does with either provider is
//! identical: open an attempt, sign, hand off, verify the callback, settle
//! once. The difference between them lives entirely in [`ecpay`] and
//! [`newebpay`]'s signing/wrapping, not in how they are used.

mod common;
pub mod ecpay;
mod error;
pub mod newebpay;

pub use common::{
    classify_rtn_code, should_settle, CallbackFacts, HandOff, HandOffRequest, PlatformCredentials,
    QueryAnswer, QueryRequest, RefundAnswer, RefundRequest, RtnClass, SignedRequest,
};
pub use error::ProviderError;

use paygate_domain::ProviderCode;
use std::collections::BTreeMap;

/// Everything a provider integration must do, from paygate's side of the
/// protocol. One implementation per provider; `infra` never matches on
/// [`ProviderCode`] itself — it asks [`adapter`] for the right one and calls
/// through the trait, which is what keeps the two providers' quirks out of
/// every other crate.
pub trait ProviderAdapter: Send + Sync {
    fn code(&self) -> ProviderCode;

    /// Build and SIGN the hand-off form. No network call.
    fn hand_off(
        &self,
        creds: &PlatformCredentials,
        req: &HandOffRequest,
    ) -> Result<HandOff, ProviderError>;

    /// Verify and parse a back-channel callback body (already url-decoded
    /// into a map). A body that does not verify is refused — `Err`, not a
    /// best-effort guess — so the caller answers with anything but
    /// [`ProviderAdapter::ack_body`] and changes nothing.
    fn parse_callback(
        &self,
        creds: &PlatformCredentials,
        form: &BTreeMap<String, String>,
    ) -> Result<CallbackFacts, ProviderError>;

    /// The exact body paygate must answer a callback with: `"1|OK"` for both
    /// providers (spec.md, "Choosing a provider").
    fn ack_body(&self) -> &'static str;

    /// Build a signed request asking what became of an attempt.
    fn build_query(&self, creds: &PlatformCredentials, q: &QueryRequest) -> SignedRequest;

    /// Verify and parse `QueryTradeInfo`'s (or NewebPay's equivalent)
    /// answer — a signed `key=value&…` string, not JSON.
    fn parse_query_answer(
        &self,
        creds: &PlatformCredentials,
        body: &str,
    ) -> Result<QueryAnswer, ProviderError>;

    /// Build a signed refund request, carrying paygate's own refund id as
    /// the provider's own deduplication key.
    fn build_refund(&self, creds: &PlatformCredentials, r: &RefundRequest) -> SignedRequest;

    /// Parse a refund call's answer.
    fn parse_refund_answer(
        &self,
        creds: &PlatformCredentials,
        body: &str,
    ) -> Result<RefundAnswer, ProviderError>;
}

static ECPAY: ecpay::EcpayAdapter = ecpay::EcpayAdapter;
static NEWEBPAY: newebpay::NewebpayAdapter = newebpay::NewebpayAdapter;

/// The adapter for a given provider code. A `&'static dyn` rather than a
/// fresh allocation each time, since an adapter carries no state of its own —
/// every credential is passed in per call.
pub fn adapter(code: ProviderCode) -> &'static dyn ProviderAdapter {
    match code {
        ProviderCode::Ecpay => &ECPAY,
        ProviderCode::Newebpay => &NEWEBPAY,
    }
}
