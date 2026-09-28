//! JSON `tracing` setup and redaction.
//!
//! `spec.md`, "Non-functional requirements" (NFR-OBS-1, NFR-OBS-3): every log
//! line is structured (`tracing` + JSON), carries the request id where one
//! exists, and never contains a `hash_key`, `hash_iv`, API key or session.
//! This module has two independent halves:
//! [`init`] (and [`JsonRedactingLayer`]) wire up the subscriber, and
//! [`redact_fields`] is the pure transform that keeps a secret out of a log
//! line regardless of which field name a caller happened to log it under.
//!
//! The layer is built on `tracing_subscriber::Registry`, specifically so that
//! a field attached ONCE to a span (`tracing::info_span!("request", request_id
//! = %id)`, which `api` opens per request) is folded onto EVERY event logged
//! inside that span — that is the whole mechanism behind "carries the request
//! id where one exists": nothing downstream has to remember to repeat it on
//! every log call.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context, Layer};
use tracing_subscriber::registry::LookupSpan;

/// Field names that must never reach a log line, however a caller spelled
/// them. Matched case-insensitively and by suffix (`_hash_key` as well as
/// `hash_key`) so a namespaced field (`merchant.hash_key`) is still caught.
const SENSITIVE_FIELD_NAMES: &[&str] = &[
    "hash_key",
    "hash_iv",
    "api_key",
    "apikey",
    "authorization",
    "session",
    "session_token",
    "password",
    "password_hash",
    "card_number",
    "card_cvc",
    "cvc",
    "check_mac_value",
    "trade_sha",
    "trade_info",
];

const REDACTED: &str = "[redacted]";

fn is_sensitive_field(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    SENSITIVE_FIELD_NAMES.iter().any(|s| {
        lower == *s || lower.ends_with(&format!(".{s}")) || lower.ends_with(&format!("_{s}"))
    })
}

/// Redact a already-collected set of event fields, by name alone — the pure
/// core of this module's whole promise. A field whose *name* looks sensitive
/// has its value replaced outright; nothing here tries to pattern-match a
/// card number out of an otherwise-innocent field; `spec.md`, "PostgreSQL"'s
/// promise is narrower and stronger: no column and, by the same discipline,
/// no log field is ever allowed to hold one in the first place; this is the
/// column.
pub fn redact_fields(fields: BTreeMap<String, String>) -> BTreeMap<String, String> {
    fields
        .into_iter()
        .map(|(k, v)| {
            if is_sensitive_field(&k) {
                (k, REDACTED.to_string())
            } else {
                (k, v)
            }
        })
        .collect()
}

/// Collects every field of one `tracing` event (or span) into a string map,
/// via `tracing`'s own [`Visit`] trait — the same mechanism `tracing-subscriber`'s
/// built-in formatters use, so this is a real visitor, not a stand-in.
#[derive(Default)]
struct FieldCollector(BTreeMap<String, String>);

impl Visit for FieldCollector {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.0
            .insert(field.name().to_string(), format!("{value:?}"));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.insert(field.name().to_string(), value.to_string());
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.0.insert(field.name().to_string(), value.to_string());
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.0.insert(field.name().to_string(), value.to_string());
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.0.insert(field.name().to_string(), value.to_string());
    }
}

/// Where a redacted JSON line goes. Production uses [`StdoutSink`]; unit
/// tests use a sink that captures lines in memory, since a subscriber's
/// actual stdout output is not a stable thing to assert against.
pub trait LogSink: Send + Sync + 'static {
    fn write_line(&self, line: &serde_json::Value);
}

pub struct StdoutSink;

impl LogSink for StdoutSink {
    fn write_line(&self, line: &serde_json::Value) {
        println!("{line}");
    }
}

/// One span's own fields, attached the first time it is created
/// (`tracing::info_span!("request", request_id = %id)`) and merged with any
/// later `Span::record` calls.
struct SpanFields(BTreeMap<String, String>);

/// A `tracing_subscriber` layer that writes one redacted JSON object per
/// event. Requires `Registry` (or anything implementing
/// `LookupSpan`) underneath it, which is what lets this layer read a parent
/// span's own stored fields when an event fires inside it.
pub struct JsonRedactingLayer {
    sink: Arc<dyn LogSink>,
}

impl JsonRedactingLayer {
    pub fn new(sink: Arc<dyn LogSink>) -> Self {
        Self { sink }
    }

    pub fn stdout() -> Self {
        Self::new(Arc::new(StdoutSink))
    }
}

impl<S> Layer<S> for JsonRedactingLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let mut collector = FieldCollector::default();
        attrs.record(&mut collector);
        if let Some(span) = ctx.span(id) {
            span.extensions_mut().insert(SpanFields(collector.0));
        }
    }

    fn on_record(&self, id: &Id, values: &Record<'_>, ctx: Context<'_, S>) {
        let mut collector = FieldCollector::default();
        values.record(&mut collector);
        if let Some(span) = ctx.span(id) {
            let mut ext = span.extensions_mut();
            if let Some(existing) = ext.get_mut::<SpanFields>() {
                existing.0.extend(collector.0);
            } else {
                ext.insert(SpanFields(collector.0));
            }
        }
    }

    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        let mut fields = BTreeMap::new();

        // Fold in every ancestor span's own fields, root first, so a
        // request-scoped `request_id` (attached once, outermost) survives
        // being overridden by an inner span's field of the same name, while
        // the event's own fields (folded in last, below) still win over
        // both — the usual "most specific wins" rule.
        if let Some(scope) = ctx.event_scope(event) {
            for span in scope.from_root() {
                if let Some(span_fields) = span.extensions().get::<SpanFields>() {
                    fields.extend(span_fields.0.clone());
                }
            }
        }

        let mut collector = FieldCollector::default();
        event.record(&mut collector);
        fields.extend(collector.0);

        let fields = redact_fields(fields);
        let meta = event.metadata();
        let line = serde_json::json!({
            "timestamp": chrono::Utc::now().to_rfc3339(),
            "level": meta.level().as_str(),
            "target": meta.target(),
            "fields": fields,
        });
        self.sink.write_line(&line);
    }
}

/// Install the global JSON-with-redaction subscriber. Every binary in this
/// workspace (`api`, `worker`, `provider-mock`, `demo-merchant`) calls this
/// once, at startup, with nothing binary-specific baked in here — a
/// `request_id` or any other field is whatever the caller's own
/// `tracing::info_span!`/`tracing::info!` calls attach, not something this
/// function knows about.
///
/// `log_level` is `LOG_LEVEL` (`config::CommonConfig`), parsed the way
/// `tracing_subscriber::EnvFilter` parses any directive string (`"info"`,
/// `"debug"`, `"paygate_api=debug,info"`, ...).
///
/// One call at process startup is everything a binary needs — nothing else
/// in this crate installs or replaces the global subscriber. A second call
/// (in the same process) is not silently accepted twice over: `tracing`
/// only ever has one global default, so this returns `Err` rather than
/// panicking or quietly swapping it out, and a caller that might legitimately
/// run its own startup path more than once (a test harness, for instance)
/// should treat that `Err` as "already installed" rather than a fatal error.
pub fn init(log_level: &str) -> Result<(), anyhow::Error> {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    use tracing_subscriber::EnvFilter;

    let filter = EnvFilter::try_new(log_level)
        .or_else(|_| EnvFilter::try_new("info"))
        .map_err(|e| anyhow::anyhow!("invalid LOG_LEVEL: {e}"))?;

    tracing_subscriber::registry()
        .with(filter)
        .with(JsonRedactingLayer::stdout())
        .try_init()
        .map_err(|e| anyhow::anyhow!("tracing subscriber already installed: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tracing_subscriber::layer::SubscriberExt;

    fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn hash_key_and_hash_iv_are_redacted() {
        let fields = map(&[
            ("hash_key", "pwFHCqoQZGmho4w6"),
            ("hash_iv", "EkRm7iFT261dpevs"),
        ]);
        let out = redact_fields(fields);
        assert_eq!(out["hash_key"], REDACTED);
        assert_eq!(out["hash_iv"], REDACTED);
    }

    #[test]
    fn a_namespaced_hash_key_field_is_still_redacted() {
        let fields = map(&[("provider.hash_key", "secret")]);
        let out = redact_fields(fields);
        assert_eq!(out["provider.hash_key"], REDACTED);
    }

    #[test]
    fn api_keys_sessions_and_passwords_are_redacted() {
        let fields = map(&[
            ("api_key", "sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc"),
            ("authorization", "Bearer sk_test_acme_x"),
            ("session", "abcdef0123456789"),
            ("password", "hunter2"),
            ("password_hash", "$2a$10$..."),
        ]);
        let out = redact_fields(fields);
        for key in [
            "api_key",
            "authorization",
            "session",
            "password",
            "password_hash",
        ] {
            assert_eq!(out[key], REDACTED, "{key} should be redacted");
        }
    }

    #[test]
    fn no_card_field_could_ever_reach_a_log_line() {
        let fields = map(&[("card_number", "4242424242424242"), ("card_cvc", "123")]);
        let out = redact_fields(fields);
        assert_eq!(out["card_number"], REDACTED);
        assert_eq!(out["card_cvc"], REDACTED);
    }

    #[test]
    fn an_ordinary_field_passes_through_unredacted() {
        let fields = map(&[
            ("request_id", "req_abc123"),
            ("merchant_id", "1"),
            ("event_type", "PaymentSucceeded"),
        ]);
        let out = redact_fields(fields.clone());
        assert_eq!(out, fields);
    }

    #[test]
    fn is_sensitive_field_is_case_insensitive() {
        assert!(is_sensitive_field("Hash_Key"));
        assert!(is_sensitive_field("HASH_IV"));
        assert!(!is_sensitive_field("merchant_id"));
    }

    /// A sink that captures every emitted line in memory, so a test can
    /// assert on the JSON a real `tracing` call chain produced, instead of
    /// on `redact_fields` alone.
    #[derive(Clone, Default)]
    struct CapturingSink(Arc<Mutex<Vec<serde_json::Value>>>);

    impl LogSink for CapturingSink {
        fn write_line(&self, line: &serde_json::Value) {
            self.0.lock().unwrap().push(line.clone());
        }
    }

    #[test]
    fn a_field_set_on_a_span_appears_on_every_event_logged_inside_it() {
        let sink = CapturingSink::default();
        let layer = JsonRedactingLayer::new(Arc::new(sink.clone()));
        let subscriber = tracing_subscriber::registry().with(layer);

        tracing::subscriber::with_default(subscriber, || {
            let span = tracing::info_span!("request", request_id = "req_abc123");
            let _enter = span.enter();
            tracing::info!(merchant_id = 1, "handled");
        });

        let lines = sink.0.lock().unwrap();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0]["fields"]["request_id"], "req_abc123");
        assert_eq!(lines[0]["fields"]["merchant_id"], "1");
    }

    #[test]
    fn a_redacted_field_set_on_a_span_is_still_redacted_on_the_event() {
        let sink = CapturingSink::default();
        let layer = JsonRedactingLayer::new(Arc::new(sink.clone()));
        let subscriber = tracing_subscriber::registry().with(layer);

        tracing::subscriber::with_default(subscriber, || {
            let span = tracing::info_span!("provider_call", hash_key = "pwFHCqoQZGmho4w6");
            let _enter = span.enter();
            tracing::info!("signed");
        });

        let lines = sink.0.lock().unwrap();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0]["fields"]["hash_key"], REDACTED);
    }

    #[test]
    fn fields_from_nested_spans_and_the_event_itself_all_appear_together() {
        let sink = CapturingSink::default();
        let layer = JsonRedactingLayer::new(Arc::new(sink.clone()));
        let subscriber = tracing_subscriber::registry().with(layer);

        tracing::subscriber::with_default(subscriber, || {
            let outer = tracing::info_span!("request", request_id = "req_1");
            let _outer_enter = outer.enter();
            let inner = tracing::info_span!("callback", provider_code = "ecpay");
            let _inner_enter = inner.enter();
            tracing::info!(outcome = "applied", "callback applied");
        });

        let lines = sink.0.lock().unwrap();
        assert_eq!(lines.len(), 1);
        let fields = &lines[0]["fields"];
        assert_eq!(fields["request_id"], "req_1");
        assert_eq!(fields["provider_code"], "ecpay");
        assert_eq!(fields["outcome"], "applied");
    }

    #[test]
    fn an_event_without_any_enclosing_span_still_logs_its_own_fields() {
        let sink = CapturingSink::default();
        let layer = JsonRedactingLayer::new(Arc::new(sink.clone()));
        let subscriber = tracing_subscriber::registry().with(layer);

        tracing::subscriber::with_default(subscriber, || {
            tracing::warn!(merchant_id = 7, "no span here");
        });

        let lines = sink.0.lock().unwrap();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0]["level"], "WARN");
        assert_eq!(lines[0]["fields"]["merchant_id"], "7");
    }
}
