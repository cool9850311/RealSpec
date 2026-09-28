//! Step implementations, one module per group mirroring `format.yml`'s own
//! section comments. Every regex below is copied verbatim from
//! `spec/bdd/format.yml`; `src/registry.rs`'s tests check that byte-for-byte.

use cucumber::gherkin::Step;

pub mod http;
pub mod infra;
pub mod ops;
pub mod provider;

use std::sync::LazyLock;

use regex::Regex;

/// `\{[a-z][a-zA-Z0-9]+\}`, format.yml's own context-variable pattern.
pub static CONTEXT_VAR_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\{[a-z][a-zA-Z0-9]+\}").unwrap());

/// `(id, pattern)` for every step this crate implements — the API-and-shared
/// surface of `format.yml`, copied verbatim. `src/registry.rs`'s tests check
/// that this is exactly the set of registry steps a `spec/bdd/api/*.feature`
/// scenario actually uses, and that every pattern here matches the
/// registry's own byte for byte.
pub const IMPLEMENTED_STEPS: &[(&str, &str)] = &[
    ("run_migration", r"^run migration$"),
    ("exec_postgresql", r"^in PostgreSQL:$"),
    (
        "http_request",
        r"^(GET|POST|PUT|PATCH|DELETE) (/api/v1/[a-zA-Z0-9/{}:._?=&%-]+):$",
    ),
    (
        "requests_concurrent",
        r"^these things happen at one instant:$",
    ),
    (
        "payment_form_submitted",
        r"^the payment form is submitted to the payment provider(, tampered after signing)?$",
    ),
    (
        "provider_card_entered",
        r"^the customer pays at the payment provider:$",
    ),
    ("response_status", r"^response status is ([1-5][0-9]{2})$"),
    (
        "response_set_status_count",
        r"^exactly ([0-9]+) responses? (?:is|are) ([1-5][0-9]{2})$",
    ),
    ("response_body_contains", r"^response body contains:$"),
    (
        "response_body_does_not_contain",
        r#"^response body does not contain "([^"]+)"$"#,
    ),
    (
        "response_header_contains",
        r#"^response header "([^"]+)" contains "([^"]+)"$"#,
    ),
    (
        "save_response_body_field",
        r#"^save response body field "([a-zA-Z_][a-zA-Z0-9_]*)" as "([a-z][a-zA-Z0-9]+)"$"#,
    ),
    (
        "save_response_cookie",
        r#"^save response cookie "([a-zA-Z0-9_-]+)" as "([a-z][a-zA-Z0-9]+)"$"#,
    ),
    (
        "postgresql_query_returns",
        r"^in PostgreSQL query returns ([0-9]+) rows?:$",
    ),
    ("background_work_settled", r"^background work has settled$"),
    (
        "clickhouse_query_returns",
        r"^in ClickHouse query returns ([0-9]+) rows?:$",
    ),
    (
        "projection_rebuilt",
        r#"^projection "(reports)" is rebuilt from the event log$"#,
    ),
    (
        "payment_provider_received",
        r"^payment provider received ([0-9]+) (checkout|refund) requests?$",
    ),
    (
        "merchant_received",
        r#"^merchant received ([0-9]+) notifications? at "(/demo-merchant/api/[a-z0-9/-]+)"$"#,
    ),
    (
        "provider_delivers_callbacks",
        r#"^payment provider delivers each pending callback ([0-9]+) times?(?:, as (forged|simulated|a wrong amount|return code "([0-9]+)"))?$"#,
    ),
    (
        "callbacks_acknowledged",
        r"^exactly ([0-9]+) callbacks? (?:was|were) (acknowledged|refused)$",
    ),
    (
        "provider_next_query_answer",
        r#"^payment provider answers the next query (?:with trade status "(0|1|10200095)"|as (forged|throttled))$"#,
    ),
    ("reconciler_runs", r"^the reconciler runs$"),
    (
        "service_state",
        r#"^service "(api|relay|ingester|notifier|redis|clickhouse)" is (stopped|started)$"#,
    ),
];

/// Extracts `{action, fields, id}` from a `POST /payments` response body —
/// shared by `http.rs` (which captures it after every successful
/// `POST /api/v1/payments`) and `provider.rs` (which replays the call and
/// re-extracts it).
pub(crate) fn extract_form(body: &serde_json::Value) -> anyhow::Result<crate::world::PendingForm> {
    use serde_json::Value;

    let action = body
        .get("action")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("order response has no string \"action\" field"))?
        .to_string();
    let Some(Value::Object(map)) = body.get("fields") else {
        anyhow::bail!("order response has no \"fields\" object");
    };
    let mut fields = std::collections::BTreeMap::new();
    for (k, v) in map {
        let s = match v {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        fields.insert(k.clone(), s);
    }
    let payment_id = body
        .get("id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    Ok(crate::world::PendingForm {
        payment_id,
        action,
        fields,
    })
}

/// Reports whether `actual` satisfies `expected` under format.yml's partial
/// match rules (`response_body_contains` / `region_contains`): every key of
/// `expected` must exist in `actual` and match; extra keys in `actual` are
/// ignored; arrays are matched element-wise and must have equal length;
/// `"<non-null>"` asserts presence and non-null only.
pub fn match_subset(
    expected: &serde_json::Value,
    actual: &serde_json::Value,
    path: &str,
) -> anyhow::Result<()> {
    use serde_json::Value;

    if let Value::String(s) = expected {
        if s == "<non-null>" {
            anyhow::ensure!(
                !actual.is_null(),
                "{path}: expected a non-null value, got null"
            );
            return Ok(());
        }
    }

    match expected {
        Value::Object(want) => {
            let Value::Object(got) = actual else {
                anyhow::bail!("{path}: expected an object, got {actual}");
            };
            let mut keys: Vec<&String> = want.keys().collect();
            keys.sort();
            for k in keys {
                let Some(child) = got.get(k) else {
                    anyhow::bail!("{path}.{k}: missing from the response");
                };
                match_subset(&want[k], child, &format!("{path}.{k}"))?;
            }
            Ok(())
        }
        Value::Array(want) => {
            let Value::Array(got) = actual else {
                anyhow::bail!("{path}: expected an array, got {actual}");
            };
            anyhow::ensure!(
                got.len() == want.len(),
                "{path}: expected {} element(s), got {}",
                want.len(),
                got.len()
            );
            for (i, w) in want.iter().enumerate() {
                match_subset(w, &got[i], &format!("{path}[{i}]"))?;
            }
            Ok(())
        }
        other => {
            anyhow::ensure!(other == actual, "{path}: expected {other}, got {actual}");
            Ok(())
        }
    }
}

/// The content of a step's docstring, without the media-type marker.
///
/// `format.yml` declares a type on every docstring — ```` """sql ```` or
/// ```` """json ```` — and gherkin hands that marker back as the first line of
/// the content. Passing it through means PostgreSQL is asked to run a statement
/// beginning with the word `sql`, and `serde_json` is asked to parse a document
/// beginning with `json`; the first is a syntax error blamed on the feature file
/// and the second is worse, because the message points at column 1 of something
/// that looks correct.
///
/// Only a first line that is exactly a type this registry declares is removed, so
/// a docstring whose real first line happens to be a bare word is left alone.
pub fn docstring_body(step: &Step) -> Option<&str> {
    let raw = step.docstring()?.as_str();
    let body = raw.strip_prefix('\n').unwrap_or(raw);
    match body.split_once('\n') {
        Some((first, rest)) if matches!(first.trim(), "sql" | "json") => Some(rest),
        _ => Some(body),
    }
}

/// A PostgreSQL error, said usefully.
///
/// `tokio_postgres::Error`'s own `Display` is often just `db error`, which tells
/// a reader nothing about what went wrong in a 40-line seed statement. The server
/// always sends more than that — the message, the SQLSTATE, and usually a detail
/// and a hint — so a failing step prints those instead of making somebody re-run
/// the suite with a logger attached.
pub fn pg_error(e: &tokio_postgres::Error) -> String {
    match e.as_db_error() {
        Some(db) => {
            let mut out = format!("{} [{}]", db.message(), db.code().code());
            if let Some(detail) = db.detail() {
                out.push_str(&format!("\n  detail: {detail}"));
            }
            if let Some(hint) = db.hint() {
                out.push_str(&format!("\n  hint: {hint}"));
            }
            if let Some(column) = db.column() {
                out.push_str(&format!("\n  column: {column}"));
            }
            if let Some(constraint) = db.constraint() {
                out.push_str(&format!("\n  constraint: {constraint}"));
            }
            out
        }
        None => e.to_string(),
    }
}
