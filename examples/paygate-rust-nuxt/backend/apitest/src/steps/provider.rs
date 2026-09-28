//! API: What the merchant does with the form, the provider's back channel,
//! and asking the provider instead of guessing.

use std::collections::BTreeMap;

use cucumber::gherkin::Step;
use cucumber::{given, then, when};
use serde::Deserialize;
use serde_json::Value;

use super::extract_form;
use super::http::{send_one, HTTP};
use crate::world::{DeliveredCallback, OrderRequestEnvelope, PendingForm, RecordedResponse, World};

/// Replays the most recent explicit `POST /api/v1/payments` call with a fresh
/// `Idempotency-Key`, exactly what a merchant's page asking again for an
/// unpaid order does (`spec/openapi/openapi.yaml`, `createOrder`: "Calling it
/// again for an unpaid order with the same parameters gets the same order and
/// a NEW provider_trade_no"). Used by `payment_form_submitted` when it is
/// called a second time in a scenario with no fresh `POST /payments` between
/// (see `duplicate_payment.feature`'s Background: one create call, two forms
/// submitted).
async fn refresh_form(world: &mut World) -> anyhow::Result<PendingForm> {
    let Some(prev) = world.last_order_request.clone() else {
        anyhow::bail!(
            "the payment form is submitted to the payment provider: the last response carried no form"
        );
    };
    let mut headers = prev.headers.clone();
    headers.insert(
        "Idempotency-Key".to_string(),
        format!("auto-retry-{}", uuid::Uuid::new_v4().simple()),
    );

    let idx = world.single_request_counter;
    world.single_request_counter += 1;
    let resp = send_one(
        world,
        idx,
        "POST",
        "/api/v1/payments",
        &headers,
        Some(prev.body.clone()),
    )
    .await?;
    anyhow::ensure!(
        resp.status / 100 == 2,
        "asking again for the order's form failed: HTTP {}\nBody: {}",
        resp.status,
        resp.body_str()
    );
    let body: Value = serde_json::from_slice(&resp.body)
        .map_err(|e| anyhow::anyhow!("order response is not valid JSON: {e}"))?;
    let form = extract_form(&body)?;
    world.last_order_request = Some(OrderRequestEnvelope {
        headers,
        body: prev.body,
    });
    Ok(form)
}

/// Changes `TotalAmount` (ECPay) after signing, or corrupts the encrypted
/// `TradeInfo` blob (NewebPay) — the harness has no crypto, so it cannot
/// surgically retarget the amount packed inside `TradeInfo`, but flipping a
/// character inside the ciphertext is exactly as fatal to the signature the
/// mock recomputes, which is the observable contract this variant exists to
/// exercise: paygate signed it, tampering after the fact makes the provider
/// refuse it.
fn tamper_fields(fields: &mut BTreeMap<String, String>) -> anyhow::Result<()> {
    if let Some(amount) = fields.get("TotalAmount").cloned() {
        let n: i64 = amount.parse().unwrap_or(0);
        fields.insert("TotalAmount".to_string(), (n + 1).to_string());
        return Ok(());
    }
    if let Some(info) = fields.get("TradeInfo").cloned() {
        let mut chars: Vec<char> = info.chars().collect();
        anyhow::ensure!(
            !chars.is_empty(),
            "TradeInfo field is empty; cannot tamper with it"
        );
        let mid = chars.len() / 2;
        chars[mid] = if chars[mid] == '0' { '1' } else { '0' };
        fields.insert("TradeInfo".to_string(), chars.into_iter().collect());
        return Ok(());
    }
    anyhow::bail!("the form has neither TotalAmount nor TradeInfo to tamper with");
}

/// `the payment form is submitted to the payment provider(, tampered after signing)?`
#[given(
    regex = r"^the payment form is submitted to the payment provider(, tampered after signing)?$"
)]
#[when(
    regex = r"^the payment form is submitted to the payment provider(, tampered after signing)?$"
)]
#[then(
    regex = r"^the payment form is submitted to the payment provider(, tampered after signing)?$"
)]
async fn payment_form_submitted(world: &mut World, variant: String) {
    payment_form_submitted_impl(world, !variant.is_empty())
        .await
        .unwrap_or_else(|e| panic!("{e}"));
}

async fn payment_form_submitted_impl(world: &mut World, tampered: bool) -> anyhow::Result<()> {
    let mut form = match world.pending_form.take() {
        Some(f) => f,
        None => refresh_form(world).await?,
    };
    if tampered {
        tamper_fields(&mut form.fields)?;
    }

    let body = serde_urlencoded::to_string(&form.fields)?;
    let base = world.stack().provider_mock_base_url();
    let url = format!("{base}{}", form.action);
    let resp = HTTP
        .post(&url)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("POST {url} failed: {e}"))?;
    let status = resp.status().as_u16();
    let headers = resp.headers().clone();
    let bytes = resp.bytes().await?.to_vec();

    world.at_cashier = status == 200;
    // The number the card will be typed against comes from the cashier page the
    // form just opened — its own hidden `providerTradeNo` field, which is exactly
    // what a browser would submit back.
    //
    // It used to be read from PostgreSQL as "the most recently started attempt",
    // and that is wrong whenever an attempt is opened between the hand-off and
    // the card: `scaling.feature`'s six concurrent creates do precisely that, so
    // the card was typed against a number whose cashier session had never been
    // opened and the mock answered `400 Unknown provider trade number`. Reading
    // the page keeps the two halves of one customer's visit tied together, and
    // works for NewebPay too, whose number is inside an encrypted blob the
    // harness deliberately cannot read.
    world.current_provider_trade_no = if world.at_cashier {
        provider_trade_no_from_cashier(&String::from_utf8_lossy(&bytes))
    } else {
        None
    };

    world.last_resp = Some(RecordedResponse {
        status,
        headers,
        body: bytes,
    });
    world.response_set = None;
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CardEntry {
    card_number: String,
    card_expiry: String,
    card_cvc: String,
}

/// `the customer pays at the payment provider:`
#[given(regex = r"^the customer pays at the payment provider:$")]
#[when(regex = r"^the customer pays at the payment provider:$")]
#[then(regex = r"^the customer pays at the payment provider:$")]
async fn provider_card_entered(world: &mut World, step: &Step) {
    provider_card_entered_impl(world, step)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
}

async fn provider_card_entered_impl(world: &mut World, step: &Step) -> anyhow::Result<()> {
    anyhow::ensure!(
        world.at_cashier,
        "the customer pays at the payment provider: the previous step did not reach a cashier"
    );
    let provider_trade_no = world.current_provider_trade_no.clone().ok_or_else(|| {
        anyhow::anyhow!("no provider_trade_no is on record for the current attempt")
    })?;

    let doc = super::docstring_body(step).ok_or_else(|| {
        anyhow::anyhow!("`the customer pays at the payment provider:` needs a docstring")
    })?;
    let content = world.resolve(doc)?;
    let card: CardEntry = serde_json::from_str(&content).map_err(|e| {
        anyhow::anyhow!(
            "card docstring must be a JSON object with card_number/card_expiry/card_cvc: {e}"
        )
    })?;

    let mut fields = BTreeMap::new();
    fields.insert("providerTradeNo".to_string(), provider_trade_no);
    fields.insert("card_number".to_string(), card.card_number);
    fields.insert("card_expiry".to_string(), card.card_expiry);
    fields.insert("card_cvc".to_string(), card.card_cvc);
    let body = serde_urlencoded::to_string(&fields)?;

    let base = world.stack().provider_mock_base_url();
    let url = format!("{base}/provider/ecpay/Cashier/Card");
    let resp = HTTP
        .post(&url)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("POST {url} failed: {e}"))?;
    let status = resp.status().as_u16();
    let headers = resp.headers().clone();
    let bytes = resp.bytes().await?.to_vec();

    world.at_cashier = false;
    world.last_resp = Some(RecordedResponse {
        status,
        headers,
        body: bytes,
    });
    world.response_set = None;
    Ok(())
}

/// `payment provider delivers each pending callback <n> times?(, as ...)?`
#[given(
    regex = r#"^payment provider delivers each pending callback ([0-9]+) times?(?:, as (forged|simulated|a wrong amount|return code "([0-9]+)"))?$"#
)]
#[when(
    regex = r#"^payment provider delivers each pending callback ([0-9]+) times?(?:, as (forged|simulated|a wrong amount|return code "([0-9]+)"))?$"#
)]
#[then(
    regex = r#"^payment provider delivers each pending callback ([0-9]+) times?(?:, as (forged|simulated|a wrong amount|return code "([0-9]+)"))?$"#
)]
async fn provider_delivers_callbacks(
    world: &mut World,
    times: u32,
    variant: String,
    return_code: String,
) {
    provider_delivers_callbacks_impl(world, times, variant, return_code)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
}

/// The mock's release payload for one delivery variant — shared by the step
/// itself and by `these things happen at one instant:`, which releases the same
/// callbacks from inside a race. One code path, so a variant can never mean two
/// different things depending on which step asked for it.
pub fn callback_release_payload(
    times: u32,
    variant: &str,
    return_code: &str,
) -> anyhow::Result<Value> {
    let (as_value, rtn_code): (&str, Option<String>) = if variant.is_empty() {
        ("queued", None)
    } else if variant == "forged" {
        ("forged", None)
    } else if variant == "simulated" {
        ("simulated", None)
    } else if variant == "a wrong amount" {
        ("wrong_amount", None)
    } else if variant.starts_with("return code") {
        ("return_code", Some(return_code.to_string()))
    } else {
        anyhow::bail!("unrecognised callback delivery variant {variant:?}");
    };

    let mut payload = serde_json::Map::new();
    payload.insert("times".to_string(), Value::from(times));
    payload.insert("as".to_string(), Value::String(as_value.to_string()));
    if let Some(code) = rtn_code {
        payload.insert("rtn_code".to_string(), Value::String(code));
    }
    Ok(Value::Object(payload))
}

/// The URL the release is posted to.
pub fn callback_release_url(mock_base: &str) -> String {
    format!("{mock_base}/provider/__control/callbacks")
}

/// Releases the mock's queued callbacks and returns what paygate answered:
/// the deliveries themselves (read by `exactly <n> callbacks were …`) and the
/// same answers as responses (read by the response-set steps).
pub async fn release_callbacks(
    url: &str,
    payload: &Value,
) -> anyhow::Result<(Vec<DeliveredCallback>, Vec<RecordedResponse>)> {
    let resp = HTTP
        .post(url)
        .json(payload)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("POST {url} failed: {e}"))?;
    anyhow::ensure!(
        resp.status().is_success(),
        "the provider mock refused to release callbacks: HTTP {}",
        resp.status()
    );
    let json: Value = resp.json().await.map_err(|e| {
        anyhow::anyhow!("the provider mock's callback-release answer is not JSON: {e}")
    })?;
    let deliveries = json
        .get("deliveries")
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow::anyhow!("the provider mock's answer has no \"deliveries\" array"))?;
    anyhow::ensure!(
        !deliveries.is_empty(),
        "the provider's queue was empty: nothing to deliver"
    );

    let mut recorded = Vec::with_capacity(deliveries.len());
    let mut set = Vec::with_capacity(deliveries.len());
    for d in deliveries {
        let status = d.get("status").and_then(|v| v.as_u64()).map(|v| v as u16);
        let body = d
            .get("body")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let accepted = d.get("accepted").and_then(|v| v.as_bool()).unwrap_or(false);
        recorded.push(DeliveredCallback {
            status,
            body: Some(body.clone()),
            accepted,
        });
        set.push(RecordedResponse {
            status: status.unwrap_or(0),
            headers: reqwest::header::HeaderMap::new(),
            body: body.into_bytes(),
        });
    }
    Ok((recorded, set))
}

async fn provider_delivers_callbacks_impl(
    world: &mut World,
    times: u32,
    variant: String,
    return_code: String,
) -> anyhow::Result<()> {
    let payload = callback_release_payload(times, &variant, &return_code)?;
    let url = callback_release_url(&world.stack().provider_mock_base_url());
    let (recorded, set) = release_callbacks(&url, &payload).await?;
    world.delivered_callbacks = Some(recorded);
    world.response_set = Some(set);
    world.last_resp = None;
    Ok(())
}

/// `exactly <n> callbacks? (was|were) (acknowledged|refused)`
#[given(regex = r"^exactly ([0-9]+) callbacks? (?:was|were) (acknowledged|refused)$")]
#[when(regex = r"^exactly ([0-9]+) callbacks? (?:was|were) (acknowledged|refused)$")]
#[then(regex = r"^exactly ([0-9]+) callbacks? (?:was|were) (acknowledged|refused)$")]
async fn callbacks_acknowledged(world: &mut World, expected: usize, outcome: String) {
    let Some(delivered) = &world.delivered_callbacks else {
        panic!("no callback delivery recorded: `payment provider delivers ...` has not run yet");
    };
    let want_accepted = outcome == "acknowledged";
    let count = delivered
        .iter()
        .filter(|d| d.accepted == want_accepted)
        .count();
    if count != expected {
        panic!(
            "expected exactly {expected} callback(s) {outcome}, got {count} of {} deliveries",
            delivered.len()
        );
    }
}

/// `payment provider answers the next query (with trade status "..."|as forged|throttled)`
#[given(
    regex = r#"^payment provider answers the next query (?:with trade status "(0|1|10200095)"|as (forged|throttled))$"#
)]
#[when(
    regex = r#"^payment provider answers the next query (?:with trade status "(0|1|10200095)"|as (forged|throttled))$"#
)]
#[then(
    regex = r#"^payment provider answers the next query (?:with trade status "(0|1|10200095)"|as (forged|throttled))$"#
)]
async fn provider_next_query_answer(world: &mut World, trade_status: String, variant: String) {
    provider_next_query_answer_impl(world, trade_status, variant)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
}

async fn provider_next_query_answer_impl(
    world: &mut World,
    trade_status: String,
    variant: String,
) -> anyhow::Result<()> {
    let mut payload = serde_json::Map::new();
    if !trade_status.is_empty() {
        payload.insert("trade_status".to_string(), Value::String(trade_status));
    } else if !variant.is_empty() {
        payload.insert("as".to_string(), Value::String(variant));
    } else {
        anyhow::bail!("neither a trade status nor a variant was captured from the step text");
    }

    let base = world.stack().provider_mock_base_url();
    let url = format!("{base}/provider/__control/query");
    let resp = HTTP
        .post(&url)
        .json(&Value::Object(payload))
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("POST {url} failed: {e}"))?;
    anyhow::ensure!(
        resp.status().is_success(),
        "arming the next query answer failed: HTTP {}",
        resp.status()
    );
    Ok(())
}

/// `payment provider received <n> (checkout|refund) requests?`
#[given(regex = r"^payment provider received ([0-9]+) (checkout|refund) requests?$")]
#[when(regex = r"^payment provider received ([0-9]+) (checkout|refund) requests?$")]
#[then(regex = r"^payment provider received ([0-9]+) (checkout|refund) requests?$")]
async fn payment_provider_received(world: &mut World, expected: usize, kind: String) {
    payment_provider_received_impl(world, expected, kind)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
}

async fn payment_provider_received_impl(
    world: &mut World,
    expected: usize,
    kind: String,
) -> anyhow::Result<()> {
    let base = world.stack().provider_mock_base_url();
    let url = format!("{base}/provider/__control/requests");
    let resp = HTTP
        .get(&url)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("GET {url} failed: {e}"))?;
    let json: Value = resp
        .json()
        .await
        .map_err(|e| anyhow::anyhow!("the provider mock's request log is not JSON: {e}"))?;
    let key = if kind == "checkout" {
        "charges"
    } else {
        "refunds"
    };
    let count = json
        .get(key)
        .and_then(|v| v.as_array())
        .map(Vec::len)
        .unwrap_or(0);
    anyhow::ensure!(
        count == expected,
        "expected exactly {expected} {kind} request(s), got {count}"
    );
    Ok(())
}

/// `merchant received <n> notifications? at "<path>"`
#[given(
    regex = r#"^merchant received ([0-9]+) notifications? at "(/demo-merchant/api/[a-z0-9/-]+)"$"#
)]
#[when(
    regex = r#"^merchant received ([0-9]+) notifications? at "(/demo-merchant/api/[a-z0-9/-]+)"$"#
)]
#[then(
    regex = r#"^merchant received ([0-9]+) notifications? at "(/demo-merchant/api/[a-z0-9/-]+)"$"#
)]
async fn merchant_received(world: &mut World, expected: usize, path: String) {
    merchant_received_impl(world, expected, path)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
}

async fn merchant_received_impl(
    world: &mut World,
    expected: usize,
    path: String,
) -> anyhow::Result<()> {
    let base = world.stack().demo_merchant_base_url();
    let url = format!("{base}/demo-merchant/api/__deliveries");
    let resp = HTTP
        .get(&url)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("GET {url} failed: {e}"))?;
    let json: Value = resp
        .json()
        .await
        .map_err(|e| anyhow::anyhow!("the merchant's delivery log is not JSON: {e}"))?;
    let count = json
        .get(&path)
        .and_then(|v| v.as_array())
        .map(Vec::len)
        .unwrap_or(0);
    anyhow::ensure!(
        count == expected,
        "expected exactly {expected} notification(s) at {path:?}, got {count}"
    );
    Ok(())
}

/// The `providerTradeNo` the cashier page asks the browser to send back.
fn provider_trade_no_from_cashier(html: &str) -> Option<String> {
    let marker = "name=\"providerTradeNo\"";
    let after = html.split_once(marker)?.1;
    let after = after.split_once("value=\"")?.1;
    let (value, _) = after.split_once('"')?;
    (!value.is_empty()).then(|| value.to_string())
}

#[cfg(test)]
mod cashier_tests {
    use super::provider_trade_no_from_cashier;

    #[test]
    fn reads_the_hidden_field_the_cashier_renders() {
        let html = r#"<form method="post" action="/provider/ecpay/Cashier/Card">
  <input type="hidden" name="providerTradeNo" value="PG00000042SNab12c">
  <input id="card_number" name="card_number" type="text">
</form>"#;
        assert_eq!(
            provider_trade_no_from_cashier(html).as_deref(),
            Some("PG00000042SNab12c")
        );
    }

    #[test]
    fn a_page_that_is_not_a_cashier_yields_nothing() {
        assert_eq!(
            provider_trade_no_from_cashier("<h1>Payment error</h1>"),
            None
        );
        assert_eq!(
            provider_trade_no_from_cashier(r#"<input name="providerTradeNo" value="">"#),
            None
        );
    }
}
