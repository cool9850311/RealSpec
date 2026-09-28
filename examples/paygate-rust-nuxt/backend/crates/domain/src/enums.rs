//! The closed vocabularies the schema stores as text. Every enum here
//! serializes to exactly the string the column holds — that is asserted by
//! the round-trip tests at the bottom of this module — so `infra` can read a
//! row's text column straight into one of these with `serde_json` (or
//! `serde_plain`-style `to_string`/`FromStr`, which we also provide) without
//! a separate mapping table anywhere.
//!
//! One exception to "snake_case": [`EventType`]. The audit table stores the
//! exact PascalCase spelling of the Rust variant (`'PaymentAttemptStarted'`,
//! not `'payment_attempt_started'`) — every `payment_events.event_type =`
//! literal in `spec/bdd/api/*.feature` is written that way. So `EventType`
//! is deliberately left without a `rename_all`, which is what makes serde's
//! default (the identifier itself) the correct wire form.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// `payments.status`. Only three states exist — no `processing`, no
/// `failed` — because paygate cannot know whether a customer is mid-payment;
/// that fact lives on the attempt, not the order (spec.md, "Events").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaymentStatus {
    Pending,
    Succeeded,
    Refunded,
}

/// `payment_attempts.status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptStatus {
    Redirected,
    Succeeded,
    Failed,
    Abandoned,
}

/// `refunds.status`. A refund is final the instant the provider answers, so
/// there is no `failed` here either — a refund that did not succeed simply
/// stays `pending` until the reconciler sends it again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefundStatus {
    Pending,
    Succeeded,
}

/// `providers.code` / `merchants.provider_code` / `payment_attempts.provider_code`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderCode {
    Ecpay,
    Newebpay,
}

/// `provider_events.outcome` — one row per thing that arrived, whatever it
/// turned out to be (spec.md, "Data model").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallbackOutcome {
    Applied,
    NoOp,
    Conflict,
    Duplicate,
    AmountMismatch,
    UnknownCode,
}

/// `payment_events.event_type`. See [`EventType::is_projected`] for which of
/// these reach the ClickHouse report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EventType {
    PaymentCreated,
    PaymentAttemptStarted,
    PaymentAttemptFailed,
    PaymentSucceeded,
    PaymentAttemptAbandoned,
    PaymentReconciled,
    PaymentDuplicatePaid,
    ProviderCallTimedOut,
    PaymentRefunded,
}

impl EventType {
    /// Matches spec.md's Events table exactly. An intention (`PaymentCreated`),
    /// a fact about routing rather than money (`PaymentAttemptStarted`), the
    /// absence of an outcome (`PaymentAttemptAbandoned`) and a call that
    /// changed nothing (`ProviderCallTimedOut`) are not projected; every
    /// event that is itself a reportable outcome is.
    pub fn is_projected(&self) -> bool {
        matches!(
            self,
            EventType::PaymentAttemptFailed
                | EventType::PaymentSucceeded
                | EventType::PaymentReconciled
                | EventType::PaymentDuplicatePaid
                | EventType::PaymentRefunded
        )
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            EventType::PaymentCreated => "PaymentCreated",
            EventType::PaymentAttemptStarted => "PaymentAttemptStarted",
            EventType::PaymentAttemptFailed => "PaymentAttemptFailed",
            EventType::PaymentSucceeded => "PaymentSucceeded",
            EventType::PaymentAttemptAbandoned => "PaymentAttemptAbandoned",
            EventType::PaymentReconciled => "PaymentReconciled",
            EventType::PaymentDuplicatePaid => "PaymentDuplicatePaid",
            EventType::ProviderCallTimedOut => "ProviderCallTimedOut",
            EventType::PaymentRefunded => "PaymentRefunded",
        }
    }
}

impl fmt::Display for EventType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for EventType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "PaymentCreated" => EventType::PaymentCreated,
            "PaymentAttemptStarted" => EventType::PaymentAttemptStarted,
            "PaymentAttemptFailed" => EventType::PaymentAttemptFailed,
            "PaymentSucceeded" => EventType::PaymentSucceeded,
            "PaymentAttemptAbandoned" => EventType::PaymentAttemptAbandoned,
            "PaymentReconciled" => EventType::PaymentReconciled,
            "PaymentDuplicatePaid" => EventType::PaymentDuplicatePaid,
            "ProviderCallTimedOut" => EventType::ProviderCallTimedOut,
            "PaymentRefunded" => EventType::PaymentRefunded,
            other => return Err(format!("unknown event_type {other:?}")),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn to_json_str<T: Serialize>(v: &T) -> String {
        serde_json::to_value(v)
            .unwrap()
            .as_str()
            .unwrap()
            .to_string()
    }

    #[test]
    fn payment_status_serializes_to_the_schema_strings() {
        assert_eq!(to_json_str(&PaymentStatus::Pending), "pending");
        assert_eq!(to_json_str(&PaymentStatus::Succeeded), "succeeded");
        assert_eq!(to_json_str(&PaymentStatus::Refunded), "refunded");
    }

    #[test]
    fn attempt_status_serializes_to_the_schema_strings() {
        assert_eq!(to_json_str(&AttemptStatus::Redirected), "redirected");
        assert_eq!(to_json_str(&AttemptStatus::Succeeded), "succeeded");
        assert_eq!(to_json_str(&AttemptStatus::Failed), "failed");
        assert_eq!(to_json_str(&AttemptStatus::Abandoned), "abandoned");
    }

    #[test]
    fn refund_status_serializes_to_the_schema_strings() {
        assert_eq!(to_json_str(&RefundStatus::Pending), "pending");
        assert_eq!(to_json_str(&RefundStatus::Succeeded), "succeeded");
    }

    #[test]
    fn provider_code_serializes_to_the_schema_strings() {
        assert_eq!(to_json_str(&ProviderCode::Ecpay), "ecpay");
        assert_eq!(to_json_str(&ProviderCode::Newebpay), "newebpay");
    }

    #[test]
    fn callback_outcome_serializes_to_the_schema_strings() {
        assert_eq!(to_json_str(&CallbackOutcome::Applied), "applied");
        assert_eq!(to_json_str(&CallbackOutcome::NoOp), "no_op");
        assert_eq!(to_json_str(&CallbackOutcome::Conflict), "conflict");
        assert_eq!(to_json_str(&CallbackOutcome::Duplicate), "duplicate");
        assert_eq!(
            to_json_str(&CallbackOutcome::AmountMismatch),
            "amount_mismatch"
        );
        assert_eq!(to_json_str(&CallbackOutcome::UnknownCode), "unknown_code");
    }

    #[test]
    fn event_type_round_trips_through_the_pascal_case_the_schema_stores() {
        let all = [
            EventType::PaymentCreated,
            EventType::PaymentAttemptStarted,
            EventType::PaymentAttemptFailed,
            EventType::PaymentSucceeded,
            EventType::PaymentAttemptAbandoned,
            EventType::PaymentReconciled,
            EventType::PaymentDuplicatePaid,
            EventType::ProviderCallTimedOut,
            EventType::PaymentRefunded,
        ];
        for e in all {
            assert_eq!(to_json_str(&e), e.as_str());
            assert_eq!(e.as_str().parse::<EventType>().unwrap(), e);
        }
    }

    // spec.md's Events table, verbatim.
    #[test]
    fn projection_matches_the_events_table() {
        assert!(!EventType::PaymentCreated.is_projected());
        assert!(!EventType::PaymentAttemptStarted.is_projected());
        assert!(EventType::PaymentAttemptFailed.is_projected());
        assert!(EventType::PaymentSucceeded.is_projected());
        assert!(!EventType::PaymentAttemptAbandoned.is_projected());
        assert!(EventType::PaymentReconciled.is_projected());
        assert!(EventType::PaymentDuplicatePaid.is_projected());
        assert!(!EventType::ProviderCallTimedOut.is_projected());
        assert!(EventType::PaymentRefunded.is_projected());
    }
}
