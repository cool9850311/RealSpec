//! The one thing this crate calls over the network by itself: paygate's own
//! `POST /api/v1/payments` and `GET /api/v1/payments/{paymentId}`
//! (`spec/openapi/openapi.yaml`), authenticated with this merchant's own
//! secret API key.
//!
//! Every failure here — a network error, a timeout, or paygate answering
//! anything but the expected status — collapses to one [`GatewayError`].
//! `spec/openapi/demo-merchant.yaml`'s own words for `POST /orders` are
//! "paygate could not be reached or refused the order" — `502
//! GATEWAY_UNAVAILABLE` — so the caller does not need to tell a refusal from
//! an outage: both mean the merchant cannot get a form right now.

use paygate_domain::{AttemptStatus, PaymentStatus};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::Duration;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GatewayError;

#[derive(Debug, Serialize)]
struct CreatePaymentRequest<'a> {
    merchant_trade_no: &'a str,
    amount: i64,
    currency: &'a str,
    item_desc: &'a str,
    notify_url: &'a str,
    client_back_url: &'a str,
}

/// The fields this crate reads out of paygate's `Payment` /
/// `CreatedOrder` schema. Anything else in the body is ignored by serde's
/// default (non-`deny_unknown_fields`) behaviour, which is what lets one
/// struct answer for both `POST /payments` (which also carries `provider`,
/// `action`, `fields`) and `GET /payments/{id}` (which does not).
#[derive(Debug, Clone, Deserialize)]
pub struct PaygatePayment {
    pub id: String,
    pub status: PaymentStatus,
    #[serde(default)]
    pub last_attempt: Option<PaygateLastAttempt>,
    #[serde(default)]
    pub action: Option<String>,
    #[serde(default)]
    pub fields: Option<BTreeMap<String, String>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PaygateLastAttempt {
    pub status: AttemptStatus,
}

/// What `POST /orders` and `GET /orders/{no}/form` both ask paygate for: an
/// order with a signed hand-off form attached.
pub struct CreatePaymentParams<'a> {
    pub merchant_trade_no: &'a str,
    pub amount_minor: i64,
    pub currency: &'a str,
    pub item_desc: &'a str,
    pub notify_url: &'a str,
    pub client_back_url: &'a str,
}

pub struct PaygateClient {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
}

impl PaygateClient {
    pub fn new(base_url: String, api_key: String) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        PaygateClient {
            http,
            base_url,
            api_key,
        }
    }

    /// `POST /api/v1/payments` with a FRESH `Idempotency-Key`, so this always
    /// opens a new attempt rather than replaying a previous answer
    /// (`openapi.yaml`, `createOrder`: "A retry carrying the same
    /// Idempotency-Key is replayed instead ... and opens nothing").
    pub async fn create_payment(
        &self,
        params: &CreatePaymentParams<'_>,
    ) -> Result<PaygatePayment, GatewayError> {
        let body = CreatePaymentRequest {
            merchant_trade_no: params.merchant_trade_no,
            amount: params.amount_minor,
            currency: params.currency,
            item_desc: params.item_desc,
            notify_url: params.notify_url,
            client_back_url: params.client_back_url,
        };

        let response = self
            .http
            .post(format!("{}/api/v1/payments", self.base_url))
            .bearer_auth(&self.api_key)
            .header("Idempotency-Key", Uuid::new_v4().to_string())
            .json(&body)
            .send()
            .await
            .map_err(|_| GatewayError)?;

        if response.status() != reqwest::StatusCode::CREATED {
            return Err(GatewayError);
        }

        response
            .json::<PaygatePayment>()
            .await
            .map_err(|_| GatewayError)
    }

    /// `GET /api/v1/payments/{paymentId}` — the merchant's own recovery path
    /// for `last_attempt`, since paygate tells it nothing when an attempt
    /// merely fails (`spec.md`, "A failed attempt leaves the order payable
    /// and tells the merchant nothing").
    pub async fn get_payment(&self, payment_id: &str) -> Result<PaygatePayment, GatewayError> {
        let response = self
            .http
            .get(format!("{}/api/v1/payments/{payment_id}", self.base_url))
            .bearer_auth(&self.api_key)
            .send()
            .await
            .map_err(|_| GatewayError)?;

        if response.status() != reqwest::StatusCode::OK {
            return Err(GatewayError);
        }

        response
            .json::<PaygatePayment>()
            .await
            .map_err(|_| GatewayError)
    }
}

impl From<PaymentStatus> for crate::state::OrderStatus {
    fn from(status: PaymentStatus) -> Self {
        match status {
            PaymentStatus::Pending => crate::state::OrderStatus::AwaitingPayment,
            PaymentStatus::Succeeded => crate::state::OrderStatus::Paid,
            PaymentStatus::Refunded => crate::state::OrderStatus::Refunded,
        }
    }
}
