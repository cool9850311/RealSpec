//! The wire shapes `demo-merchant.yaml` declares, transcribed field for
//! field — the same relationship `frontend/types/shop.ts` has to this same
//! file, on the other side of the wire.

use paygate_domain::AttemptStatus;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::state::OrderStatus;

#[derive(Debug, Deserialize)]
pub struct CreateShopOrderRequest {
    pub amount: String,
}

#[derive(Debug, Serialize)]
pub struct CreateShopOrderResponse {
    pub merchant_trade_no: String,
    pub pay_url: String,
}

#[derive(Debug, Serialize)]
pub struct ShopPaymentForm {
    pub action: String,
    pub fields: BTreeMap<String, String>,
}

#[derive(Debug, Serialize)]
pub struct ShopOrder {
    pub merchant_trade_no: String,
    pub status: OrderStatus,
    pub amount: i64,
    pub trade_no: Option<String>,
    /// The addition documented in `demo-merchant.yaml`: absent (never
    /// `null`) once the order is no longer `awaiting_payment`, or when
    /// paygate could not be asked, or when it had nothing to say about a
    /// `last_attempt` at all.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_attempt_status: Option<AttemptStatus>,
}

/// ECPay's own shape, plus the signature — `demo-merchant.yaml`'s
/// `Notification` schema. Deserialized from the URL-decoded form body as a
/// plain map rather than a typed struct: every `notify-*` handler needs the
/// RAW field set (to recompute `CheckMacValue` over it, and to log it
/// verbatim to `__deliveries`), not a picked subset.
pub type NotificationForm = BTreeMap<String, String>;
