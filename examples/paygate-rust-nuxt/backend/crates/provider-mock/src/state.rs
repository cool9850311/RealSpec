//! Everything the mock remembers, in memory, for the one scenario it serves
//! (spec.md, "The payment provider mock": "started per scenario, serving both
//! shapes from one process").
//!
//! Three kinds of state:
//!   - one [`Attempt`] per `provider_trade_no` the cashier has seen, carrying
//!     what the card decided and whether a callback for it is still owed;
//!   - the callback queue itself: an insertion-ordered list of pending
//!     `provider_trade_no`s, and the full delivery log both control routes
//!     read from;
//!   - the query/refund control surfaces: the next armed `QueryTradeInfo`
//!     answer, its throttle window, and the refund dedup table keyed by
//!     paygate's own `RefundTradeNo`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Instant;

use paygate_domain::ProviderCode;
use serde::Serialize;

use crate::config::Config;

pub fn provider_str(code: ProviderCode) -> &'static str {
    match code {
        ProviderCode::Ecpay => "ecpay",
        ProviderCode::Newebpay => "newebpay",
    }
}

/// What the card typed at the provider decided, fixed at card-submission
/// time and replayed verbatim whenever the queued callback is released
/// (spec.md, "The payment provider mock").
#[derive(Debug, Clone)]
pub struct CardOutcome {
    pub rtn_code: i32,
    pub failure_code: Option<String>,
    pub card6: String,
    pub card4: String,
    pub eci: String,
    pub auth_code: Option<String>,
}

/// One hand-off the mock's cashier has accepted. Lives from the checkout
/// POST until the process exits; a callback for it may be released, refused,
/// and released again any number of times.
#[derive(Debug, Clone)]
pub struct Attempt {
    pub provider: ProviderCode,
    pub provider_merchant_id: String,
    pub provider_trade_no: String,
    pub amount: i64,
    pub return_url: String,
    pub client_back_url: String,
    /// The exact fields the cashier verified this trade number against, so a
    /// byte-identical resubmission (a double-click, a browser replay) is
    /// recognised as "the same order arriving again" rather than a reused
    /// trade number (`payment-provider.yaml`, `ecpayCashier`).
    pub original_form: std::collections::BTreeMap<String, String>,
    pub charge_id: String,
    pub card: Option<CardOutcome>,
    /// Whether a callback for this attempt is currently owed. Cleared the
    /// instant a delivery of it is acknowledged; set again never (an
    /// attempt's outcome, once decided at card entry, does not change).
    pub pending: bool,
}

/// A wire-shaped snapshot of one callback, in the vocabulary
/// `payment-provider.yaml`'s `QueuedCallback` schema stores it in: every
/// value already a string, the way ECPay's own form carries it.
#[derive(Debug, Clone, Serialize)]
pub struct CallbackRecord {
    pub provider: &'static str,
    pub provider_trade_no: String,
    pub event_id: String,
    pub rtn_code: String,
    pub rtn_msg: String,
    pub simulate_paid: String,
    pub trade_amt: String,
    pub card4no: Option<String>,
    pub card6no: Option<String>,
    pub eci: Option<String>,
    pub auth_code: Option<String>,
    pub failure_code: Option<String>,
}

/// [`CallbackRecord`] plus what happened when it was actually sent
/// (`DeliveredCallback` in the schema).
#[derive(Debug, Clone, Serialize)]
pub struct DeliveredRecord {
    #[serde(flatten)]
    pub callback: CallbackRecord,
    /// `None` when the delivery could not be made at all because the target
    /// process was gone: there is no status to report, and the field is
    /// required, so it is present and null
    /// (`spec/openapi/payment-provider.yaml`, `DeliveredCallback`:
    /// "`null` when the delivery could not be made at all"). Not `0`, which
    /// would be a lie in a field whose other values are HTTP statuses.
    pub status: Option<i64>,
    pub body: String,
    pub accepted: bool,
}

/// One request the mock answered, whatever it was: a charge at the cashier
/// or a refund call (`RecordedRequest` in the schema).
#[derive(Debug, Clone, Serialize)]
pub struct RecordedRequest {
    pub provider: &'static str,
    pub provider_trade_no: String,
    pub amount: Option<i64>,
    pub refund_trade_no: Option<String>,
    pub signature_verified: bool,
    pub deduplicated: Option<bool>,
}

/// One `QueryTradeInfo` (or NewebPay's equivalent) the mock answered.
#[derive(Debug, Clone, Serialize)]
pub struct QueryLogEntry {
    pub provider_trade_no: String,
    pub signature_verified: bool,
    pub answered: String,
}

/// What `POST /__control/query` armed for the next `QueryTradeInfo` call.
#[derive(Debug, Clone)]
pub enum ArmedQuery {
    TradeStatus(String),
    Forged,
    Throttled,
}

/// A settled refund answer, kept so a retry of the same `RefundTradeNo`
/// (paygate's own dedup key) gets the exact same answer back instead of
/// being processed twice (spec.md, "Refunds").
#[derive(Debug, Clone)]
pub struct RefundOutcome {
    pub status: u16,
    pub body: String,
}

pub struct AppState {
    pub config: Config,
    pub http: reqwest::Client,

    pub attempts: Mutex<HashMap<String, Attempt>>,
    /// Insertion-ordered `provider_trade_no`s with a callback still owed —
    /// release walks this in order (spec.md: "all copies of the first
    /// callback are parked at a gate and released together... then the next
    /// callback's").
    pub pending_order: Mutex<Vec<String>>,
    pub delivered_log: Mutex<Vec<DeliveredRecord>>,

    pub charge_counter: AtomicU64,

    pub armed_query: Mutex<Option<ArmedQuery>>,
    pub query_throttled_until: Mutex<Option<Instant>>,
    pub query_log: Mutex<Vec<QueryLogEntry>>,

    pub charge_requests: Mutex<Vec<RecordedRequest>>,
    pub refund_requests: Mutex<Vec<RecordedRequest>>,
    pub refund_settled: Mutex<HashMap<String, RefundOutcome>>,
}

impl AppState {
    pub fn new(config: Config) -> Self {
        AppState {
            config,
            http: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .expect("building the mock's outbound http client"),
            attempts: Mutex::new(HashMap::new()),
            pending_order: Mutex::new(Vec::new()),
            delivered_log: Mutex::new(Vec::new()),
            charge_counter: AtomicU64::new(1),
            armed_query: Mutex::new(None),
            query_throttled_until: Mutex::new(None),
            query_log: Mutex::new(Vec::new()),
            charge_requests: Mutex::new(Vec::new()),
            refund_requests: Mutex::new(Vec::new()),
            refund_settled: Mutex::new(HashMap::new()),
        }
    }

    /// A plausible provider charge id (`TradeNo`), unique within this
    /// process — the mock invents no bank behind it, so shape rather than
    /// authenticity is all that matters.
    pub fn next_charge_id(&self, provider: ProviderCode) -> String {
        let n = self.charge_counter.fetch_add(1, Ordering::Relaxed);
        let prefix = match provider {
            ProviderCode::Ecpay => "21",
            ProviderCode::Newebpay => "22",
        };
        format!("{prefix}{n:012}")
    }
}
