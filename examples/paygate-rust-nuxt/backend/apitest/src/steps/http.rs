//! API: HTTP Request, and API: Response Assertions.

use std::{collections::HashMap, sync::LazyLock, time::Duration};

use cucumber::gherkin::Step;
use cucumber::{given, then, when};
use serde::Deserialize;
use serde_json::Value;

use crate::world::{DeliveredCallback, RecordedResponse, World};

/// The harness's own client: no cookie jar, no redirects — format.yml,
/// `http_request`: "the next request sends exactly the headers its own
/// docstring writes".
pub(crate) static HTTP: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .cookie_store(false)
        .timeout(Duration::from_secs(30))
        .build()
        .expect("building the harness HTTP client")
});

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    #[serde(default)]
    headers: HashMap<String, String>,
    #[serde(default)]
    body: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FreeEnvelope {
    method: String,
    path: String,
    #[serde(default)]
    headers: HashMap<String, String>,
    #[serde(default)]
    body: Option<Value>,
}

/// `None`/`null`/`{}` all mean "send no body" (format.yml, `http_request`).
fn effective_body(body: Option<Value>) -> Option<Value> {
    match body {
        None => None,
        Some(Value::Null) => None,
        Some(Value::Object(m)) if m.is_empty() => None,
        Some(other) => Some(other),
    }
}

pub(crate) async fn send_one(
    world: &World,
    replica: usize,
    method: &str,
    path: &str,
    headers: &HashMap<String, String>,
    body: Option<Value>,
) -> anyhow::Result<RecordedResponse> {
    let stack = world.stack();
    let n = stack.api.len().max(1);
    let base = stack.api.base_url(replica % n);
    let url = format!("{base}{path}");
    let m = reqwest::Method::from_bytes(method.as_bytes())
        .map_err(|e| anyhow::anyhow!("invalid method {method:?}: {e}"))?;

    let mut req = HTTP.request(m, &url);
    if let Some(b) = &body {
        req = req
            .header("Content-Type", "application/json")
            .body(serde_json::to_vec(b)?);
    }
    for (k, v) in headers {
        req = req.header(k.as_str(), v.as_str());
    }

    let resp = req
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("{method} {path} failed: {e}"))?;
    let status = resp.status().as_u16();
    let headers_out = resp.headers().clone();
    let body_bytes = resp.bytes().await?.to_vec();
    Ok(RecordedResponse {
        status,
        headers: headers_out,
        body: body_bytes,
    })
}

fn parse_envelope(text: &str) -> anyhow::Result<Envelope> {
    serde_json::from_str(text).map_err(|e| {
        anyhow::anyhow!(
            "request docstring must be a JSON object with only \"headers\" and \"body\": {e}"
        )
    })
}

/// `<METHOD> <path>:` — records ONE response and clears any recorded set.
#[given(regex = r"^(GET|POST|PUT|PATCH|DELETE) (/api/v1/[a-zA-Z0-9/{}:._?=&%-]+):$")]
#[when(regex = r"^(GET|POST|PUT|PATCH|DELETE) (/api/v1/[a-zA-Z0-9/{}:._?=&%-]+):$")]
#[then(regex = r"^(GET|POST|PUT|PATCH|DELETE) (/api/v1/[a-zA-Z0-9/{}:._?=&%-]+):$")]
async fn http_request(world: &mut World, method: String, path: String, step: &Step) {
    http_request_impl(world, method, path, step)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
}

async fn http_request_impl(
    world: &mut World,
    method: String,
    path: String,
    step: &Step,
) -> anyhow::Result<()> {
    let path = world.resolve(&path)?;
    let content = world.resolve(
        super::docstring_body(step)
            .ok_or_else(|| anyhow::anyhow!("`{method} {path}:` needs a docstring"))?,
    )?;
    let envelope = parse_envelope(&content)?;

    let idx = world.single_request_counter;
    world.single_request_counter += 1;

    let sent_body = effective_body(envelope.body);
    let resp = send_one(
        world,
        idx,
        &method,
        &path,
        &envelope.headers,
        sent_body.clone(),
    )
    .await?;

    // A successful `POST /api/v1/payments` hands back a form. Capture it —
    // and the request that produced it — for `payment_form_submitted`,
    // whether this is the scenario's first order or a second hand-off for one
    // already created (format.yml, `payment_form_submitted`: "the most recent
    // response, which is the answer to POST /payments").
    if method == "POST" && path == "/api/v1/payments" && resp.status / 100 == 2 {
        if let Ok(json) = serde_json::from_slice::<Value>(&resp.body) {
            if let Ok(form) = crate::steps::extract_form(&json) {
                world.pending_form = Some(form);
                if let Some(body) = sent_body {
                    world.last_order_request = Some(crate::world::OrderRequestEnvelope {
                        headers: envelope.headers.clone(),
                        body,
                    });
                }
            }
        }
    }

    world.last_resp = Some(resp);
    world.response_set = None;
    Ok(())
}

/// What one actor of a race produced: its answers, in the order it produced
/// them, and — for a callback release — the deliveries themselves.
#[derive(Default)]
struct ActorOutcome {
    responses: Vec<RecordedResponse>,
    callbacks: Option<Vec<DeliveredCallback>>,
}

/// One actor of `these things happen at one instant:`, resolved to owned data
/// BEFORE the gate opens. Nothing here borrows the World: an actor has to be
/// able to run while every other actor runs, and the World is one object.
enum Actor {
    /// A caller of this scenario's service — an `http_request` envelope.
    Request {
        envelope: FreeEnvelope,
        base: String,
    },
    /// `payment provider delivers each pending callback …`: the provider
    /// posting a signed callback, which no step of this registry can imitate.
    Callbacks { url: String, payload: Value },
    /// `the reconciler runs`: one pass on every reconciler instance.
    Reconciler { urls: Vec<String> },
}

/// The steps a race can name, by the registry's own sentence. Every one is
/// `When` with no docstring, as `format.yml`'s `step_ref` requires — but the
/// registry says what may be NAMED, and this says what this harness can
/// currently RUN concurrently: a step that mutates the scenario's own state
/// mid-race (the hand-off's pending form, a service being stopped) is refused
/// by name rather than raced wrongly. `registry.rs` pins this list.
pub const RACEABLE_STEPS: &[&str] = &[
    "the reconciler runs",
    "payment provider delivers each pending callback <n> time(s)[, as <variant>]",
];

static DELIVER_CALLBACKS_RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(
        r#"^payment provider delivers each pending callback ([0-9]+) times?(?:, as (forged|simulated|a wrong amount|return code "([0-9]+)"))?$"#,
    )
    .expect("the callback-delivery pattern is the registry's own")
});

/// One `{"step": "<sentence>"}` element, understood but not yet resolved
/// against the stack — split out so the sentences this harness accepts can be
/// tested without containers.
#[derive(Debug, PartialEq, Eq)]
enum NamedActor {
    Reconciler,
    Callbacks {
        times: u32,
        variant: String,
        return_code: String,
    },
}

fn parse_named(sentence: &str) -> anyhow::Result<NamedActor> {
    if sentence == "the reconciler runs" {
        return Ok(NamedActor::Reconciler);
    }
    if let Some(caps) = DELIVER_CALLBACKS_RE.captures(sentence) {
        return Ok(NamedActor::Callbacks {
            times: caps[1].parse()?,
            variant: caps
                .get(2)
                .map(|m| m.as_str())
                .unwrap_or_default()
                .to_string(),
            return_code: caps
                .get(3)
                .map(|m| m.as_str())
                .unwrap_or_default()
                .to_string(),
        });
    }
    anyhow::bail!(
        "`{sentence}` cannot take part in a race here. This harness can race: {}",
        RACEABLE_STEPS.join("; ")
    )
}

/// Resolves one named element into an actor, against this scenario's stack.
fn named_actor(world: &World, sentence: &str) -> anyhow::Result<Actor> {
    match parse_named(sentence)? {
        NamedActor::Reconciler => Ok(Actor::Reconciler {
            urls: world.stack().reconciler_run_urls(),
        }),
        NamedActor::Callbacks {
            times,
            variant,
            return_code,
        } => Ok(Actor::Callbacks {
            url: super::provider::callback_release_url(&world.stack().provider_mock_base_url()),
            payload: super::provider::callback_release_payload(times, &variant, &return_code)?,
        }),
    }
}

/// `these things happen at one instant:` — the registry's one way to arrange a
/// race. An element is an HTTP caller of this scenario's service, or the
/// sentence of a step this registry already declares: the two actors that
/// matter and are NOT callers of this service are a provider's signed callback
/// and a worker's own pass, and neither could otherwise be shown to hold while
/// the merchant API is being used at the same instant. Records a SET — the
/// union of what every actor answered, in element order — and clears the
/// single response.
#[given(regex = r"^these things happen at one instant:$")]
#[when(regex = r"^these things happen at one instant:$")]
#[then(regex = r"^these things happen at one instant:$")]
async fn requests_concurrent(world: &mut World, step: &Step) {
    requests_concurrent_impl(world, step)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
}

async fn requests_concurrent_impl(world: &mut World, step: &Step) -> anyhow::Result<()> {
    let content = world.resolve(super::docstring_body(step).ok_or_else(|| {
        anyhow::anyhow!("`these things happen at one instant:` needs a docstring")
    })?)?;
    let elements: Vec<Value> = serde_json::from_str(&content)
        .map_err(|e| anyhow::anyhow!("docstring must be a JSON array of actors: {e}"))?;
    anyhow::ensure!(
        elements.len() >= 2,
        "{} actor(s) listed; a race needs at least 2",
        elements.len()
    );

    let stack_api_len = world.stack().api.len().max(1);
    let mut actors = Vec::with_capacity(elements.len());
    let mut request_index = 0usize;
    for (i, element) in elements.into_iter().enumerate() {
        // An element whose ONLY key is `step` names a step of this registry;
        // anything else is an envelope, so a body field called `step` is still
        // a body (`format.yml`, `step_ref`).
        let named = element
            .as_object()
            .filter(|o| o.len() == 1)
            .and_then(|o| o.get("step"))
            .and_then(|v| v.as_str())
            .map(str::to_string);
        match named {
            Some(sentence) => actors.push(named_actor(world, &sentence)?),
            None => {
                let envelope: FreeEnvelope = serde_json::from_value(element).map_err(|e| {
                    anyhow::anyhow!(
                        "actor {}: not a {{method,path,headers,body}} envelope, and not a \
                         {{\"step\": \"…\"}} name either: {e}",
                        i + 1
                    )
                })?;
                // The k-th single REQUEST visits replica ((k-1) mod N) + 1, as
                // it does outside a race; a named step is not a caller of this
                // service and takes no turn in that rotation.
                let base = world.stack().api.base_url(request_index % stack_api_len);
                request_index += 1;
                actors.push(Actor::Request { envelope, base });
            }
        }
    }

    let n = actors.len();
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(n));
    let mut tasks = Vec::with_capacity(n);
    for (i, actor) in actors.into_iter().enumerate() {
        let barrier = barrier.clone();
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            match actor {
                Actor::Request { envelope, base } => {
                    let body = effective_body(envelope.body);
                    let url = format!("{base}{}", envelope.path);
                    let m =
                        reqwest::Method::from_bytes(envelope.method.as_bytes()).map_err(|e| {
                            anyhow::anyhow!("invalid method {:?}: {e}", envelope.method)
                        })?;
                    let mut req = HTTP.request(m, &url);
                    if let Some(b) = &body {
                        req = req
                            .header("Content-Type", "application/json")
                            .body(serde_json::to_vec(b)?);
                    }
                    for (k, v) in &envelope.headers {
                        req = req.header(k.as_str(), v.as_str());
                    }
                    let resp = req.send().await.map_err(|e| {
                        anyhow::anyhow!(
                            "actor {}: {} {} failed: {e}",
                            i + 1,
                            envelope.method,
                            envelope.path
                        )
                    })?;
                    let status = resp.status().as_u16();
                    let headers_out = resp.headers().clone();
                    let body_bytes = resp.bytes().await?.to_vec();
                    Ok::<_, anyhow::Error>(ActorOutcome {
                        responses: vec![RecordedResponse {
                            status,
                            headers: headers_out,
                            body: body_bytes,
                        }],
                        callbacks: None,
                    })
                }
                Actor::Callbacks { url, payload } => {
                    let (recorded, set) = super::provider::release_callbacks(&url, &payload)
                        .await
                        .map_err(|e| anyhow::anyhow!("actor {}: {e}", i + 1))?;
                    Ok(ActorOutcome {
                        responses: set,
                        callbacks: Some(recorded),
                    })
                }
                Actor::Reconciler { urls } => {
                    // Every instance, as the step itself does; `SKIP LOCKED`
                    // means only one of them claims a given row.
                    let mut any_ok = false;
                    let mut errors = Vec::new();
                    for url in &urls {
                        match HTTP.post(url).send().await {
                            Ok(resp) if resp.status().is_success() => any_ok = true,
                            Ok(resp) => errors.push(format!("/run answered {}", resp.status())),
                            Err(e) => errors.push(format!("/run failed: {e}")),
                        }
                    }
                    anyhow::ensure!(
                        any_ok,
                        "actor {}: no reconciler instance completed a pass: {}",
                        i + 1,
                        errors.join("; ")
                    );
                    // A pass answers nobody: it contributes no response.
                    Ok(ActorOutcome::default())
                }
            }
        }));
    }

    let mut responses = Vec::with_capacity(n);
    let mut callbacks: Option<Vec<DeliveredCallback>> = None;
    for (i, t) in tasks.into_iter().enumerate() {
        let outcome = t
            .await
            .map_err(|e| anyhow::anyhow!("actor {}: task panicked: {e}", i + 1))??;
        responses.extend(outcome.responses);
        if let Some(delivered) = outcome.callbacks {
            callbacks.get_or_insert_with(Vec::new).extend(delivered);
        }
    }

    world.response_set = Some(responses);
    if callbacks.is_some() {
        world.delivered_callbacks = callbacks;
    }
    world.last_resp = None;
    Ok(())
}

/// `response status is <code>`.
#[given(regex = r"^response status is ([1-5][0-9]{2})$")]
#[when(regex = r"^response status is ([1-5][0-9]{2})$")]
#[then(regex = r"^response status is ([1-5][0-9]{2})$")]
async fn response_status(world: &mut World, expected: u16) {
    let resp = world.require_response().unwrap_or_else(|e| panic!("{e}"));
    if resp.status != expected {
        panic!(
            "expected HTTP {expected}, got {}\nBody: {}",
            resp.status,
            resp.body_str()
        );
    }
}

/// `exactly <n> responses are <status>` — EXACT.
#[given(regex = r"^exactly ([0-9]+) responses? (?:is|are) ([1-5][0-9]{2})$")]
#[when(regex = r"^exactly ([0-9]+) responses? (?:is|are) ([1-5][0-9]{2})$")]
#[then(regex = r"^exactly ([0-9]+) responses? (?:is|are) ([1-5][0-9]{2})$")]
async fn response_set_status_count(world: &mut World, expected: usize, status: u16) {
    let Some(set) = &world.response_set else {
        panic!("no concurrent responses recorded: no `is called concurrently:` step has run yet");
    };
    let count = set.iter().filter(|r| r.status == status).count();
    if count != expected {
        panic!(
            "expected exactly {expected} of the {} responses to be HTTP {status}, got {count}\n\
             Actual distribution:\n{}",
            set.len(),
            distribution(set)
        );
    }
}

fn distribution(set: &[RecordedResponse]) -> String {
    use std::collections::BTreeMap;
    let mut counts: BTreeMap<u16, usize> = BTreeMap::new();
    let mut sample: BTreeMap<u16, String> = BTreeMap::new();
    for r in set {
        *counts.entry(r.status).or_default() += 1;
        sample.entry(r.status).or_insert_with(|| r.body_str());
    }
    counts
        .into_iter()
        .map(|(status, n)| format!("  {n} x HTTP {status}, e.g. {}", sample[&status]))
        .collect::<Vec<_>>()
        .join("\n")
}

/// `response body contains:` — partial match.
#[given(regex = r"^response body contains:$")]
#[when(regex = r"^response body contains:$")]
#[then(regex = r"^response body contains:$")]
async fn response_body_contains(world: &mut World, step: &Step) {
    response_body_contains_impl(world, step).unwrap_or_else(|e| panic!("{e}"));
}

fn response_body_contains_impl(world: &mut World, step: &Step) -> anyhow::Result<()> {
    world.require_response()?;
    let doc = super::docstring_body(step)
        .ok_or_else(|| anyhow::anyhow!("`response body contains:` needs a docstring"))?;
    let content = world.resolve(doc)?;
    let expected: Value = serde_json::from_str(&content)
        .map_err(|e| anyhow::anyhow!("expected JSON is not valid: {e}"))?;
    let resp = world.require_response()?;
    let actual: Value = serde_json::from_slice(&resp.body).map_err(|e| {
        anyhow::anyhow!(
            "response body is not valid JSON: {e}\nBody: {}",
            resp.body_str()
        )
    })?;
    crate::steps::match_subset(&expected, &actual, "$")
        .map_err(|e| anyhow::anyhow!("{e}\nActual body:\n{}", pretty(&actual)))
}

fn pretty(v: &Value) -> String {
    serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string())
}

/// `response body does not contain "<text>"`.
#[given(regex = r#"^response body does not contain "([^"]+)"$"#)]
#[when(regex = r#"^response body does not contain "([^"]+)"$"#)]
#[then(regex = r#"^response body does not contain "([^"]+)"$"#)]
async fn response_body_does_not_contain(world: &mut World, text: String) {
    let resp = world.require_response().unwrap_or_else(|e| panic!("{e}"));
    let body = resp.body_str();
    if let Some(idx) = body.find(&text) {
        let start = idx.saturating_sub(40);
        let end = (idx + text.len() + 40).min(body.len());
        panic!(
            "response body contains {text:?} but must not: ...{}...",
            &body[start..end]
        );
    }
}

/// `response header "<name>" contains "<substring>"`.
#[given(regex = r#"^response header "([^"]+)" contains "([^"]+)"$"#)]
#[when(regex = r#"^response header "([^"]+)" contains "([^"]+)"$"#)]
#[then(regex = r#"^response header "([^"]+)" contains "([^"]+)"$"#)]
async fn response_header_contains(world: &mut World, header: String, substring: String) {
    let resp = world.require_response().unwrap_or_else(|e| panic!("{e}"));
    let values: Vec<&str> = resp
        .headers
        .get_all(header.as_str())
        .iter()
        .filter_map(|v| v.to_str().ok())
        .collect();
    if values.is_empty() {
        let present: Vec<&str> = resp.headers.keys().map(|k| k.as_str()).collect();
        panic!(
            "response has no {header:?} header; headers present: {}",
            present.join(", ")
        );
    }
    if !values.iter().any(|v| v.contains(&substring)) {
        panic!("no {header:?} header contains {substring:?}; values: {values:?}");
    }
}

fn render_value(value: &Value) -> anyhow::Result<String> {
    match value {
        Value::String(s) => Ok(s.clone()),
        Value::Bool(b) => Ok(b.to_string()),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(i.to_string())
            } else {
                Ok(n.to_string())
            }
        }
        other => Ok(serde_json::to_string(other)?),
    }
}

/// `save response body field "<field>" as "<varName>"`.
#[given(
    regex = r#"^save response body field "([a-zA-Z_][a-zA-Z0-9_]*)" as "([a-z][a-zA-Z0-9]+)"$"#
)]
#[when(regex = r#"^save response body field "([a-zA-Z_][a-zA-Z0-9_]*)" as "([a-z][a-zA-Z0-9]+)"$"#)]
#[then(regex = r#"^save response body field "([a-zA-Z_][a-zA-Z0-9_]*)" as "([a-z][a-zA-Z0-9]+)"$"#)]
async fn save_response_body_field(world: &mut World, field: String, var_name: String) {
    save_response_body_field_impl(world, field, var_name).unwrap_or_else(|e| panic!("{e}"));
}

fn save_response_body_field_impl(
    world: &mut World,
    field: String,
    var_name: String,
) -> anyhow::Result<()> {
    let resp = world.require_response()?;
    let body: Value = serde_json::from_slice(&resp.body).map_err(|e| {
        anyhow::anyhow!(
            "response body is not valid JSON: {e}\nBody: {}",
            resp.body_str()
        )
    })?;
    let Value::Object(map) = &body else {
        anyhow::bail!(
            "response body is not a JSON object\nBody: {}",
            pretty(&body)
        );
    };
    let Some(value) = map.get(&field) else {
        anyhow::bail!(
            "response body has no field {field:?}\nBody: {}",
            pretty(&body)
        );
    };
    anyhow::ensure!(!value.is_null(), "response body field {field:?} is null");
    let rendered = render_value(value)?;
    world.vars.insert(var_name, rendered);
    Ok(())
}

/// `save response cookie "<name>" as "<varName>"`.
#[given(regex = r#"^save response cookie "([a-zA-Z0-9_-]+)" as "([a-z][a-zA-Z0-9]+)"$"#)]
#[when(regex = r#"^save response cookie "([a-zA-Z0-9_-]+)" as "([a-z][a-zA-Z0-9]+)"$"#)]
#[then(regex = r#"^save response cookie "([a-zA-Z0-9_-]+)" as "([a-z][a-zA-Z0-9]+)"$"#)]
async fn save_response_cookie(world: &mut World, name: String, var_name: String) {
    save_response_cookie_impl(world, name, var_name).unwrap_or_else(|e| panic!("{e}"));
}

fn save_response_cookie_impl(
    world: &mut World,
    name: String,
    var_name: String,
) -> anyhow::Result<()> {
    let resp = world.require_response()?;
    let all: Vec<&str> = resp
        .headers
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .collect();
    for raw in &all {
        let first = raw.split(';').next().unwrap_or("");
        let Some((k, v)) = first.split_once('=') else {
            continue;
        };
        if k.trim() != name {
            continue;
        }
        anyhow::ensure!(
            !v.is_empty(),
            "response set cookie {name:?} to the empty string, which clears it rather than granting one"
        );
        world.vars.insert(var_name, v.to_string());
        return Ok(());
    }
    anyhow::bail!("response set no {name:?} cookie (Set-Cookie: {all:?})");
}

#[cfg(test)]
pub mod race_tests {
    use super::*;

    /// The sentences a scenario may name, one per shape this harness can run.
    /// `registry.rs` checks each one against `format.yml` itself.
    pub const RACEABLE_SAMPLES: &[&str] = &[
        "the reconciler runs",
        "payment provider delivers each pending callback 1 time",
        "payment provider delivers each pending callback 2 times, as forged",
        r#"payment provider delivers each pending callback 1 time, as return code "10300066""#,
    ];

    #[test]
    fn every_sample_sentence_is_understood() {
        for sentence in RACEABLE_SAMPLES {
            parse_named(sentence)
                .unwrap_or_else(|e| panic!("a sentence this harness advertises is refused: {e}"));
        }
    }

    #[test]
    fn a_callback_release_keeps_its_count_and_variant() {
        assert_eq!(
            parse_named("payment provider delivers each pending callback 2 times, as forged")
                .unwrap(),
            NamedActor::Callbacks {
                times: 2,
                variant: "forged".to_string(),
                return_code: String::new(),
            }
        );
    }

    /// A step the registry allows to be named but that this harness cannot run
    /// concurrently is refused BY NAME, with what it can run — never raced
    /// wrongly and never silently skipped.
    #[test]
    fn a_step_this_harness_cannot_race_is_refused_by_name() {
        let err = parse_named(r#"service "api" is stopped"#)
            .unwrap_err()
            .to_string();
        assert!(err.contains("cannot take part in a race"), "{err}");
        assert!(err.contains("the reconciler runs"), "{err}");
    }
}
