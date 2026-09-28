//! In-process state. This service keeps no database of its own — `Cargo.toml`
//! pins no SQL dependency for its manifest: it is a fixture that stands up
//! fresh with every stack, and `__deliveries` is explicitly "counted from
//! process start" rather than from any persisted log.

use chrono::{DateTime, Utc};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};
use tokio::sync::RwLock;

use crate::config::Config;
use crate::gateway::PaygateClient;

/// `demo-merchant.yaml`'s `Order.status`, the merchant's OWN belief — updated
/// only by a `notify-*` callback, never by the browser's return
/// (`shopGetOrder`'s description).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderStatus {
    AwaitingPayment,
    Paid,
    Refunded,
}

/// One order this merchant created through its own `/orders`. Everything
/// paygate needs to mint another form is kept here so `GET .../form` can ask
/// again with the exact same parameters (`shopGetPaymentForm`: "the same
/// merchant_trade_no ... and the same parameters").
#[derive(Debug, Clone)]
pub struct Order {
    pub merchant_trade_no: String,
    pub payment_id: String,
    pub amount_minor: i64,
    pub currency: String,
    pub item_desc: String,
    pub notify_url: String,
    pub client_back_url: String,
    pub status: OrderStatus,
    pub action: String,
    pub fields: BTreeMap<String, String>,
    /// Whether the form paygate signed has already been handed to the shop's
    /// own page once. `POST /orders` opens the FIRST attempt and stores its
    /// form here unclaimed (`false`); the first `GET .../form` simply hands
    /// that one out and flips this to `true`; every ask after that mints a
    /// genuinely new one (`demo-merchant.yaml`, `shopGetPaymentForm`: "Asking
    /// AGAIN mints a new one"). One page render must cost exactly one
    /// attempt at paygate — `shop.feature`'s attempt-count assertions ("two
    /// renders, two attempts") are exactly this invariant.
    pub form_issued: bool,
}

/// One row of the merchant's own delivery log — the addition documented in
/// `demo-merchant.yaml` as `GET /__deliveries`.
#[derive(Debug, Clone, Serialize)]
pub struct Delivery {
    pub received_at: DateTime<Utc>,
    pub body: BTreeMap<String, String>,
    pub answered: String,
}

pub struct AppState {
    pub config: Config,
    pub gateway: PaygateClient,
    pub orders: RwLock<HashMap<String, Order>>,
    pub deliveries: RwLock<HashMap<String, Vec<Delivery>>>,
    /// `/notify-flaky`'s own memory of how many times it has been asked about
    /// a given `MerchantTradeNo` — fails the first two, then answers `1|OK`.
    pub flaky_attempts: RwLock<HashMap<String, u32>>,
}

impl AppState {
    pub fn new(config: Config) -> Self {
        let gateway = PaygateClient::new(config.gateway_url.clone(), config.api_key.clone());
        AppState {
            config,
            gateway,
            orders: RwLock::new(HashMap::new()),
            deliveries: RwLock::new(HashMap::new()),
            flaky_attempts: RwLock::new(HashMap::new()),
        }
    }
}

/// Records one delivery to the merchant's own log, under the path that
/// received it. `path` is always one of the seven `notify*` paths this
/// service serves — a closed, tiny set — so a plain `String` key costs
/// nothing worth avoiding.
pub async fn record_delivery(
    state: &AppState,
    path: &str,
    body: BTreeMap<String, String>,
    answered: impl Into<String>,
) {
    let mut deliveries = state.deliveries.write().await;
    deliveries
        .entry(path.to_string())
        .or_default()
        .push(Delivery {
            received_at: Utc::now(),
            body,
            answered: answered.into(),
        });
}
