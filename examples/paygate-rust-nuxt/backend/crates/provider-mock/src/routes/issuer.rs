//! The issuing bank's 3-D Secure page — a third origin, offering both
//! outcomes so a browser can take either path with the same card
//! (spec.md, "The payment provider mock"). Releasing the queue and
//! redirecting the browser happen in that order here, matching
//! `payment-provider.yaml`'s own note that in life the order is not
//! guaranteed and paygate must not rely on it.

use std::sync::Arc;

use axum::extract::{Form, Path, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Redirect, Response};
use serde::Deserialize;

use crate::callbacks::{self, Variant};
use crate::state::AppState;
use crate::util::format_dollars;
use crate::{cards, html};

pub async fn issuer_page(
    State(state): State<Arc<AppState>>,
    Path(provider_trade_no): Path<String>,
) -> Response {
    let attempts = state.attempts.lock().expect("attempts lock");
    let Some(attempt) = attempts.get(&provider_trade_no) else {
        return (StatusCode::NOT_FOUND, "unknown provider trade number").into_response();
    };
    Html(html::issuer_page(
        &provider_trade_no,
        &format_dollars(attempt.amount),
    ))
    .into_response()
}

#[derive(Debug, Deserialize)]
pub struct AuthenticateForm {
    outcome: String,
}

pub async fn authenticate(
    State(state): State<Arc<AppState>>,
    Path(provider_trade_no): Path<String>,
    Form(form): Form<AuthenticateForm>,
) -> Response {
    let client_back_url = {
        let mut attempts = state.attempts.lock().expect("attempts lock");
        let Some(attempt) = attempts.get_mut(&provider_trade_no) else {
            return (StatusCode::NOT_FOUND, "unknown provider trade number").into_response();
        };
        // Failing the challenge overrides whatever the card would have said
        // (e2e `shop.feature`, "A customer who fails authentication can try
        // again"): the same card can take either path, and only the
        // authenticate path lets the card's own outcome through.
        if form.outcome == "fail" {
            if let Some(card) = &attempt.card {
                attempt.card = Some(cards::three_ds_failed(card));
            }
        }
        attempt.client_back_url.clone()
    };

    // The back channel first, the redirect after — releasing exactly one
    // callback, this attempt's own.
    callbacks::release(&state, Some(&provider_trade_no), 1, Variant::Honest).await;

    Redirect::to(&client_back_url).into_response()
}
