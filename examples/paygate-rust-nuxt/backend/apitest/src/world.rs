//! The [`World`]: everything one scenario carries between steps.
//!
//! One `World` is constructed per scenario by the `cucumber` runner
//! (`Default::default()`); the actual stack of containers is not started
//! until the `before` hook in `tests/api.rs` runs, because only there do we
//! know the scenario's tags (`@stack:scaled`, `@serial`).

use std::collections::{BTreeMap, HashMap};

use serde_json::Value;

use crate::stack::Stack;

/// One HTTP response the harness recorded, kept as headers + body rather than
/// a live `reqwest::Response` so it can be read more than once.
#[derive(Debug, Clone)]
pub struct RecordedResponse {
    pub status: u16,
    pub headers: reqwest::header::HeaderMap,
    pub body: Vec<u8>,
}

impl RecordedResponse {
    pub fn body_str(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// The form the merchant's page has in hand: the `action` and `fields` of the
/// most recent `POST /payments` response, and the request that produced it
/// (kept so `payment_form_submitted` can replay it — with a fresh
/// `Idempotency-Key` — the way a merchant's page asking again for an unpaid
/// order would).
#[derive(Debug, Clone)]
pub struct PendingForm {
    pub payment_id: Option<String>,
    pub action: String,
    pub fields: BTreeMap<String, String>,
}

/// The exact envelope (headers + body) of the most recent explicit
/// `POST /api/v1/payments` call, kept so a second, unpaid-order call for the
/// same order can be replayed with a new `Idempotency-Key`.
#[derive(Debug, Clone)]
pub struct OrderRequestEnvelope {
    pub headers: HashMap<String, String>,
    pub body: Value,
}

/// One delivery `provider_delivers_callbacks` recorded, and
/// `callbacks_acknowledged` / `response_set_status_count` read back.
#[derive(Debug, Clone)]
pub struct DeliveredCallback {
    /// `None` when the delivery could not be made at all (the api was down);
    /// recorded as refused, and does not fail the step.
    pub status: Option<u16>,
    pub body: Option<String>,
    pub accepted: bool,
}

/// Which half of the process-wide serial lock this scenario is holding.
/// `@serial` scenarios (and every `@stack:scaled` one, which carries
/// `@serial`) hold the write half so they run alone; every other scenario
/// holds the read half so any number of them may run together.
#[derive(Default)]
pub enum SerialGuard {
    #[default]
    None,
    Read(tokio::sync::OwnedRwLockReadGuard<()>),
    Write(tokio::sync::OwnedRwLockWriteGuard<()>),
}

impl std::fmt::Debug for SerialGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            SerialGuard::None => "None",
            SerialGuard::Read(_) => "Read",
            SerialGuard::Write(_) => "Write",
        };
        write!(f, "SerialGuard::{s}")
    }
}

#[derive(Debug, Default, cucumber::World)]
pub struct World {
    /// `None` until the `before` hook starts the scenario's stack.
    pub stack: Option<Stack>,

    /// `{varName}` substitution bag.
    pub vars: HashMap<String, String>,

    /// The most recent SINGLE response, and the most recent SET — mutually
    /// exclusive, per format.yml.
    pub last_resp: Option<RecordedResponse>,
    pub response_set: Option<Vec<RecordedResponse>>,

    /// State for `payment_form_submitted` / `provider_card_entered`.
    pub pending_form: Option<PendingForm>,
    pub last_order_request: Option<OrderRequestEnvelope>,
    pub at_cashier: bool,
    /// The `provider_trade_no` of the attempt most recently handed to the
    /// provider — set by `payment_form_submitted`, read by
    /// `provider_card_entered`.
    pub current_provider_trade_no: Option<String>,

    /// The set `provider_delivers_callbacks` recorded, read by
    /// `callbacks_acknowledged`.
    pub delivered_callbacks: Option<Vec<DeliveredCallback>>,

    /// Round-robin counters, reset per scenario: one for `http_request`
    /// (`(k-1) mod N`), independent of the one used by the concurrent steps
    /// and by callback delivery (each of those counts from 0 inside its own
    /// step call).
    pub single_request_counter: usize,

    pub serial_guard: SerialGuard,
}

impl World {
    pub fn stack(&self) -> &Stack {
        self.stack
            .as_ref()
            .expect("stack not started: the `before` hook did not run or failed")
    }

    pub fn stack_mut(&mut self) -> &mut Stack {
        self.stack
            .as_mut()
            .expect("stack not started: the `before` hook did not run or failed")
    }

    /// Substitutes every `{varName}` in `text` from the scenario's context
    /// bag. Leaves anything not matching `{[a-z][a-zA-Z0-9]+}` untouched, and
    /// fails loudly (rather than passing a literal `{foo}` on to PostgreSQL or
    /// an HTTP call) when a token survives substitution.
    pub fn resolve(&self, text: &str) -> anyhow::Result<String> {
        let mut out = text.to_string();
        for (k, v) in &self.vars {
            out = out.replace(&format!("{{{k}}}"), v);
        }
        if let Some(m) = crate::steps::CONTEXT_VAR_RE.find(&out) {
            let mut known: Vec<&str> = self.vars.keys().map(String::as_str).collect();
            known.sort_unstable();
            anyhow::bail!(
                "unknown context variable {} (saved so far: {})",
                m.as_str(),
                if known.is_empty() {
                    "none".to_string()
                } else {
                    known.join(", ")
                }
            );
        }
        Ok(out)
    }

    /// Guard every single-response assertion opens with.
    pub fn require_response(&self) -> anyhow::Result<&RecordedResponse> {
        if let Some(r) = &self.last_resp {
            return Ok(r);
        }
        if let Some(set) = &self.response_set {
            anyhow::bail!(
                "the last HTTP step was concurrent and recorded {} responses, not one; \
                 assert them with `exactly <n> responses are <status>`",
                set.len()
            );
        }
        anyhow::bail!("no response recorded: no HTTP step has run yet")
    }
}
