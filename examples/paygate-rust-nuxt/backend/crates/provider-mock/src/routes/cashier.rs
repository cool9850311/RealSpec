//! The two hops a card takes at the provider: the cashier
//! (`ecpayCashier`/`newebpayCashier`) and the card form it renders
//! (`ecpaySubmitCard`, plus a NewebPay-namespaced alias — see this crate's
//! build report). Both verify first; the card is looked at only once the
//! signature has (spec.md, "Authentication").

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::extract::{Form, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Redirect, Response};
use paygate_domain::ProviderCode;
use serde::Deserialize;

use crate::state::{provider_str, AppState, Attempt, RecordedRequest};
use crate::{cards, html, wire};

async fn handle_cashier(
    state: Arc<AppState>,
    provider: ProviderCode,
    outer_form: BTreeMap<String, String>,
    card_endpoint: &'static str,
) -> Response {
    let verified_fields: Option<BTreeMap<String, String>> = match provider {
        ProviderCode::Ecpay => {
            if wire::ecpay_verify(&state.config.ecpay, &outer_form) {
                Some(outer_form.clone())
            } else {
                None
            }
        }
        ProviderCode::Newebpay => wire::newebpay_verify(&state.config.newebpay, &outer_form),
    };

    let signature_verified = verified_fields.is_some();
    // ECPay's own fields are clear text, so a bad signature still leaves
    // `MerchantTradeNo`/`TotalAmount` readable for the request log — only
    // NewebPay's `TradeInfo` is genuinely opaque once it fails to verify.
    let unverified_readable = match provider {
        ProviderCode::Ecpay => Some(&outer_form),
        ProviderCode::Newebpay => None,
    };
    let readable = verified_fields.as_ref().or(unverified_readable);
    let provider_trade_no = readable
        .and_then(|f| f.get("MerchantTradeNo").cloned())
        .unwrap_or_default();
    let amount = readable
        .and_then(|f| f.get("TotalAmount"))
        .and_then(|s| s.parse::<i64>().ok());

    state
        .charge_requests
        .lock()
        .expect("charge_requests lock")
        .push(RecordedRequest {
            provider: provider_str(provider),
            provider_trade_no: provider_trade_no.clone(),
            amount,
            refund_trade_no: None,
            signature_verified,
            deduplicated: None,
        });

    let Some(fields) = verified_fields else {
        return (
            StatusCode::BAD_REQUEST,
            Html(html::cashier_error_page("CheckMacValue Error")),
        )
            .into_response();
    };

    if provider_trade_no.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Html(html::cashier_error_page("MerchantTradeNo Error")),
        )
            .into_response();
    }

    let merchant_id = fields.get("MerchantID").cloned().unwrap_or_default();
    let amount = fields
        .get("TotalAmount")
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(0);
    let return_url = fields.get("ReturnURL").cloned().unwrap_or_default();
    let client_back_url = fields.get("ClientBackURL").cloned().unwrap_or_default();

    {
        let mut attempts = state.attempts.lock().expect("attempts lock");
        if let Some(existing) = attempts.get(&provider_trade_no) {
            // ECPay refuses a MerchantTradeNo it has seen before, UNLESS this
            // is byte-for-byte the same order arriving again (a double-click,
            // a browser replay) — payment-provider.yaml, `ecpayCashier`.
            return if existing.original_form == fields {
                Html(html::cashier_page(
                    card_endpoint,
                    &provider_trade_no,
                    &existing.client_back_url,
                ))
                .into_response()
            } else {
                (
                    StatusCode::BAD_REQUEST,
                    Html(html::cashier_error_page("MerchantTradeNo Error")),
                )
                    .into_response()
            };
        }

        let charge_id = state.next_charge_id(provider);
        attempts.insert(
            provider_trade_no.clone(),
            Attempt {
                provider,
                provider_merchant_id: merchant_id,
                provider_trade_no: provider_trade_no.clone(),
                amount,
                return_url,
                client_back_url: client_back_url.clone(),
                original_form: fields,
                charge_id,
                card: None,
                pending: false,
            },
        );
    }

    Html(html::cashier_page(
        card_endpoint,
        &provider_trade_no,
        &client_back_url,
    ))
    .into_response()
}

pub async fn ecpay_cashier(
    State(state): State<Arc<AppState>>,
    Form(form): Form<BTreeMap<String, String>>,
) -> Response {
    handle_cashier(
        state,
        ProviderCode::Ecpay,
        form,
        "/provider/ecpay/Cashier/Card",
    )
    .await
}

pub async fn newebpay_cashier(
    State(state): State<Arc<AppState>>,
    Form(form): Form<BTreeMap<String, String>>,
) -> Response {
    handle_cashier(
        state,
        ProviderCode::Newebpay,
        form,
        "/provider/newebpay/Cashier/Card",
    )
    .await
}

#[derive(Debug, Deserialize)]
pub struct CardForm {
    #[serde(rename = "providerTradeNo")]
    provider_trade_no: String,
    card_number: String,
    card_expiry: String,
    card_cvc: String,
}

/// The only place in this repository a card number exists
/// (payment-provider.yaml, `ecpaySubmitCard`). Shared by both cashiers'
/// forms — the outcome only ever depends on the card and the attempt it
/// names, never on which provider's page it was typed into.
pub async fn submit_card(
    State(state): State<Arc<AppState>>,
    Form(form): Form<CardForm>,
) -> Response {
    let trade_no = form.provider_trade_no.clone();
    let outcome = cards::decide(&form.card_number, &form.card_expiry, &form.card_cvc);

    let mut attempts = state.attempts.lock().expect("attempts lock");
    let Some(attempt) = attempts.get_mut(&trade_no) else {
        return (
            StatusCode::BAD_REQUEST,
            Html(html::cashier_error_page("Unknown provider trade number")),
        )
            .into_response();
    };

    match outcome {
        Err(message) => (
            StatusCode::BAD_REQUEST,
            Html(html::cashier_error_page(message)),
        )
            .into_response(),
        Ok(card) => {
            attempt.card = Some(card);
            attempt.pending = true;
            drop(attempts);
            {
                let mut order = state.pending_order.lock().expect("pending_order lock");
                if !order.iter().any(|t| t == &trade_no) {
                    order.push(trade_no.clone());
                }
            }
            Redirect::to(&format!("/provider/issuer/3ds/{trade_no}")).into_response()
        }
    }
}
