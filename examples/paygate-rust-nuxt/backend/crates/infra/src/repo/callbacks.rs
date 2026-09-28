//! `apply_callback` — the one transaction `spec.md`'s "Two tables, one
//! transaction" diagram is drawn for. By the time this function is called,
//! the caller (the `api` crate) has already verified the callback's
//! signature with `paygate_provider::adapter(code).parse_callback` — this
//! function only ever sees [`CallbackFacts`] that verified, and its own job
//! is entirely the state machine: decide the six [`CallbackOutcome`]
//! variants, and write whatever each one implies, in one transaction, under
//! the payment's and the attempt's own row locks.
//!
//! The six outcomes, and why each is infra's call rather than
//! `paygate_domain::apply`'s: `apply` sees only a payment's status, amount
//! and amount-refunded (`state.rs`'s own doc comment) — never which attempt a
//! command came from — so it cannot by itself tell a race-loser (`no_op`)
//! apart from a genuine second payment (`duplicate`) or a contradicting
//! decline (`conflict`). This function is the one place that holds both the
//! attempt's identity and its row lock at once, which is what makes the
//! distinction possible at all.

use paygate_domain::{
    Amount, AttemptStatus, CallbackOutcome, EventType, PaymentStatus, ProviderCode,
};
use paygate_provider::{classify_rtn_code, CallbackFacts, RtnClass};
use serde_json::Value;
use uuid::Uuid;

use crate::db::Db;
use crate::error::{Error, Result};

use super::{
    insert_event, insert_notification, lock_attempt_by_provider_trade_no, lock_payment_by_id,
    merchant_notify_fields, AttemptRow, MerchantContext, MerchantNotifyFacts, PaymentRow,
};

/// What applying one callback did.
#[derive(Debug, Clone)]
pub struct CallbackApplication {
    pub outcome: CallbackOutcome,
    pub payment_id: Uuid,
    pub attempt_id: Uuid,
    /// The payment's status once this transaction committed.
    pub payment_status: PaymentStatus,
    /// Set only when `outcome == Duplicate`: this attempt really was paid a
    /// second time, for this amount. The caller decides, from
    /// `merchants.duplicate_auto_refund`, whether to follow up with
    /// [`crate::repo::create_refund`] against this same `attempt_id` — the
    /// duplicate is recorded here, in this transaction, before any refund is
    /// ever attempted (`spec.md`, "When the customer pays twice").
    pub duplicate_amount: Option<Amount>,
}

/// paygate answers a callback's back channel with exactly `1|OK` — but only
/// when it should, per `spec.md`, "Being told by the provider": every
/// outcome except `amount_mismatch` is acknowledged (an amount mismatch is
/// "refused, not reconciled away", so the provider keeps holding it and
/// tries again). A signature that did not verify is not this function's
/// concern at all — the caller never reaches `apply_callback` with one.
pub fn should_acknowledge(outcome: CallbackOutcome) -> bool {
    outcome != CallbackOutcome::AmountMismatch
}

/// The internal decision, richer than [`CallbackOutcome`] alone: two of the
/// outcomes (`Applied`, for a settlement or a decline; `NoOp`, for a
/// simulated notification or a same-attempt replay) need to know which case
/// they are before anything is written.
enum Decision {
    AmountMismatch,
    /// `SimulatePaid=1` — identical to a real notification otherwise, and
    /// settles nothing (`spec.md`, "Being told by the provider").
    Simulated,
    /// The same attempt reporting paid (or declined) again, after it had
    /// already reached a final state — a callback racing a reconciliation
    /// that already won, or plain at-least-once redelivery under a fresh
    /// `provider_event_id`.
    ReplayOfResolvedAttempt,
    Settled,
    FailedAttempt {
        failure_code: String,
    },
    Duplicate,
    Conflict,
    UnknownCode,
}

impl Decision {
    fn outcome(&self) -> CallbackOutcome {
        match self {
            Decision::AmountMismatch => CallbackOutcome::AmountMismatch,
            Decision::Simulated | Decision::ReplayOfResolvedAttempt => CallbackOutcome::NoOp,
            Decision::Settled | Decision::FailedAttempt { .. } => CallbackOutcome::Applied,
            Decision::Duplicate => CallbackOutcome::Duplicate,
            Decision::Conflict => CallbackOutcome::Conflict,
            Decision::UnknownCode => CallbackOutcome::UnknownCode,
        }
    }
}

/// Pure: given the payment's and the attempt's locked state, and the
/// verified facts, decide what this callback means. No I/O, so this is what
/// `apply_callback`'s own unit tests exercise directly.
fn decide(payment: &PaymentRow, attempt: &AttemptRow, facts: &CallbackFacts) -> Decision {
    if facts.amount != payment.amount {
        return Decision::AmountMismatch;
    }
    if facts.simulate_paid {
        return Decision::Simulated;
    }
    match classify_rtn_code(facts.rtn_code) {
        RtnClass::Paid => {
            if attempt.status == AttemptStatus::Succeeded {
                // The same attempt, reporting paid a second time — the
                // callback that lost a race with the reconciler's own query,
                // or plain at-least-once redelivery.
                Decision::ReplayOfResolvedAttempt
            } else if payment.status == PaymentStatus::Pending {
                Decision::Settled
            } else {
                // The order already settled through a DIFFERENT attempt (or
                // was refunded since); this attempt's own money still moved.
                Decision::Duplicate
            }
        }
        RtnClass::Failed(code) => {
            if attempt.status == AttemptStatus::Failed {
                Decision::ReplayOfResolvedAttempt
            } else if attempt.status == AttemptStatus::Redirected
                && payment.status == PaymentStatus::Pending
            {
                Decision::FailedAttempt { failure_code: code }
            } else {
                // Either the order settled through a different attempt
                // already, or this attempt itself already resolved another
                // way (succeeded, abandoned) — a decline that arrives after
                // either changes nothing here.
                Decision::Conflict
            }
        }
        RtnClass::PendingConfirmation | RtnClass::Unknown => Decision::UnknownCode,
    }
}

fn facts_to_raw(facts: &CallbackFacts) -> Value {
    serde_json::json!({
        "provider_trade_no": facts.provider_trade_no,
        "provider_event_id": facts.provider_event_id,
        "provider_charge_id": facts.provider_charge_id,
        "rtn_code": facts.rtn_code,
        "rtn_msg": facts.rtn_msg,
        "amount": facts.amount,
        "simulate_paid": facts.simulate_paid,
        "failure_code": facts.failure_code,
        "card_brand": facts.card_brand,
        "card_last4": facts.card_last4,
        "eci": facts.eci,
        "auth_code": facts.auth_code,
        "paid_at": facts.paid_at,
    })
}

fn provider_code_str(code: ProviderCode) -> &'static str {
    match code {
        ProviderCode::Ecpay => "ecpay",
        ProviderCode::Newebpay => "newebpay",
    }
}

fn outcome_str(outcome: CallbackOutcome) -> &'static str {
    match outcome {
        CallbackOutcome::Applied => "applied",
        CallbackOutcome::NoOp => "no_op",
        CallbackOutcome::Conflict => "conflict",
        CallbackOutcome::Duplicate => "duplicate",
        CallbackOutcome::AmountMismatch => "amount_mismatch",
        CallbackOutcome::UnknownCode => "unknown_code",
    }
}

/// Whether a stored `provider_events.outcome` already reflects a business
/// fact that must never happen twice: `Applied` settled the payment or
/// closed the attempt as failed, `Duplicate` recorded a second charge as
/// really paid. Both moved money or closed a state transition, so a
/// redelivery under the same `(provider_code, event_id)` must replay that
/// verbatim rather than decide again — re-running `decide` a second time
/// could otherwise re-settle an already-succeeded payment (a lost lock is
/// the only thing standing between "replay" and "double-apply") or turn a
/// recorded duplicate into something else if the world moved on since.
///
/// Every other outcome — `NoOp`, `Conflict`, `AmountMismatch`,
/// `UnknownCode` — left the payment's and the attempt's state exactly where
/// it already was: nothing was settled, nothing was closed, nothing but the
/// audit row itself was written. Nothing is lost by deciding those again
/// from the attempt's and payment's CURRENT state on redelivery, and it is
/// what a refused `amount_mismatch` needs: `spec.md`, "Being told by the
/// provider" — a refused delivery "stays queued and can be released again",
/// which only means something if the corrected redelivery is actually
/// re-evaluated instead of replaying the same refusal forever.
fn outcome_already_committed(outcome: CallbackOutcome) -> bool {
    matches!(
        outcome,
        CallbackOutcome::Applied | CallbackOutcome::Duplicate
    )
}

/// Apply one verified provider callback. See the module doc for the shape of
/// the decision; this function's own job is turning that decision into rows.
pub async fn apply_callback(
    db: &Db,
    provider_code: ProviderCode,
    facts: &CallbackFacts,
    merchant: &MerchantContext,
    merchant_hash_key: &str,
    merchant_hash_iv: &str,
) -> Result<CallbackApplication> {
    let provider_code = provider_code_str(provider_code);
    let mut tx = db.begin().await?;

    // Lock order: `payments` before `payment_attempts`, always — see the
    // invariant on `repo::lock_payment`/`repo::lock_attempt_by_id` in
    // `repo/mod.rs`. This function used to lock the attempt FIRST (it only
    // has the attempt's provider identity to start from, not a payment id),
    // then the payment its `payment_id` named — the reverse of the order
    // `repo::refunds::create_refund` takes the same two locks in, which is
    // an ABBA deadlock waiting for a concurrent refund and callback on the
    // same payment (verified by
    // `concurrent_refund_and_callback_on_the_same_payment_never_deadlock` in
    // `tests/postgres_repo.rs`).
    //
    // The fix: resolve `payment_id` WITHOUT taking any lock first, then take
    // the two locks in the canonical order. This is sound because an
    // attempt's `payment_id` is set once, at `INSERT`, and never updated
    // again by any code in this crate — so an unlocked read of it now and a
    // locked re-read of the whole attempt row a moment later are reading the
    // same fact, just at two different times; nothing can have changed
    // `payment_id` in between for the lock to miss.
    let payment_id: Uuid = sqlx::query_scalar(
        "SELECT payment_id FROM payment_attempts WHERE provider_code = $1 AND provider_trade_no = $2",
    )
    .bind(provider_code)
    .bind(&facts.provider_trade_no)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(Error::PaymentNotFound)?;

    let payment = lock_payment_by_id(&mut tx, payment_id)
        .await?
        .ok_or(Error::PaymentNotFound)?;
    let attempt =
        lock_attempt_by_provider_trade_no(&mut tx, provider_code, &facts.provider_trade_no)
            .await?
            .ok_or(Error::PaymentNotFound)?;

    // Re-delivery of a callback: `provider_events`'s own primary key
    // (`provider_code`, `event_id`) is the deduplication (`spec.md`, "Being
    // told by the provider"). Every re-delivery is acknowledged (unless it
    // is refused all over again), but whether it has any NEW effect depends
    // on what the first delivery already committed — see
    // `outcome_already_committed`. An outcome that committed nothing is
    // re-decided from the attempt's and payment's current state rather than
    // replayed, which is what lets a refused `amount_mismatch` be corrected
    // by a later, honest delivery of the same event.
    let existing: Option<(String,)> = sqlx::query_as(
        "SELECT outcome FROM provider_events WHERE provider_code = $1 AND event_id = $2",
    )
    .bind(provider_code)
    .bind(&facts.provider_event_id)
    .fetch_optional(&mut *tx)
    .await?;
    let existing_outcome = existing
        .as_ref()
        .map(|(s,)| super::parse_enum_column::<CallbackOutcome>("outcome", s))
        .transpose()?;
    if let Some(outcome) = existing_outcome {
        if outcome_already_committed(outcome) {
            tx.commit().await?;
            return Ok(CallbackApplication {
                outcome,
                payment_id: payment.id,
                attempt_id: attempt.id,
                payment_status: payment.status,
                duplicate_amount: None,
            });
        }
    }

    let decision = decide(&payment, &attempt, facts);
    let outcome = decision.outcome();

    // The row is keyed on `(provider_code, event_id)` alone, so a
    // re-evaluated redelivery cannot INSERT a second row for the same event
    // — it UPDATEs the one that is already there, in place, with whatever
    // this delivery's facts newly decided. `payment_id` and `attempt_id`
    // never change across redeliveries of one event (the event id is the
    // attempt's own charge id), so only the outcome-dependent columns move.
    if existing_outcome.is_some() {
        sqlx::query(
            "UPDATE provider_events SET rtn_code = $1, outcome = $2, raw = $3, received_at = now() \
              WHERE provider_code = $4 AND event_id = $5",
        )
        .bind(facts.rtn_code)
        .bind(outcome_str(outcome))
        .bind(facts_to_raw(facts))
        .bind(provider_code)
        .bind(&facts.provider_event_id)
        .execute(&mut *tx)
        .await?;
    } else {
        sqlx::query(
            "INSERT INTO provider_events \
                (provider_code, event_id, payment_id, attempt_id, rtn_code, outcome, raw) \
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(provider_code)
        .bind(&facts.provider_event_id)
        .bind(payment.id)
        .bind(attempt.id)
        .bind(facts.rtn_code)
        .bind(outcome_str(outcome))
        .bind(facts_to_raw(facts))
        .execute(&mut *tx)
        .await?;
    }

    let mut duplicate_amount = None;

    match &decision {
        Decision::AmountMismatch | Decision::ReplayOfResolvedAttempt => {
            // Nothing else is written: an amount mismatch is refused
            // outright, and a replay of an already-resolved attempt has
            // nothing left to do.
        }
        Decision::Simulated => {
            // `SimulatePaid=1` is recorded on the attempt it names — even
            // when `RtnCode` underneath it would have failed the attempt
            // (`spec.md`, "Being told by the provider": "identical to a
            // real notification otherwise ... settles nothing") — but
            // settles nothing else: no payment status change, no
            // `PaymentSucceeded`/`PaymentAttemptFailed` event, no
            // notification. This is independent of `RtnClass`, which is why
            // it lives in its own arm rather than being folded into
            // `Settled`/`FailedAttempt`.
            mark_simulated(&mut tx, attempt.id).await?;
        }
        Decision::Settled => {
            settle(
                &mut tx,
                &payment,
                &attempt,
                facts,
                merchant,
                merchant_hash_key,
                merchant_hash_iv,
            )
            .await?;
        }
        Decision::FailedAttempt { failure_code } => {
            fail_attempt(&mut tx, &payment, &attempt, facts, failure_code).await?;
        }
        Decision::Duplicate => {
            record_duplicate(&mut tx, &payment, &attempt, facts).await?;
            duplicate_amount = Some(facts.amount);
        }
        Decision::Conflict => {
            // The contradicted attempt is left exactly as the hand-off left
            // it (`spec.md`, "Being told by the provider").
        }
        Decision::UnknownCode => {
            if attempt.status == AttemptStatus::Redirected {
                sqlx::query("UPDATE payment_attempts SET rtn_code = $1 WHERE id = $2")
                    .bind(facts.rtn_code)
                    .bind(attempt.id)
                    .execute(&mut *tx)
                    .await?;
            }
        }
    }

    let final_status: (String,) = sqlx::query_as("SELECT status FROM payments WHERE id = $1")
        .bind(payment.id)
        .fetch_one(&mut *tx)
        .await?;
    let payment_status: PaymentStatus = super::parse_enum_column("status", &final_status.0)?;

    tx.commit().await?;

    Ok(CallbackApplication {
        outcome,
        payment_id: payment.id,
        attempt_id: attempt.id,
        payment_status,
        duplicate_amount,
    })
}

async fn settle(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    payment: &PaymentRow,
    attempt: &AttemptRow,
    facts: &CallbackFacts,
    merchant: &MerchantContext,
    merchant_hash_key: &str,
    merchant_hash_iv: &str,
) -> Result<()> {
    // The conditional update: `spec.md`'s "Concurrency" table names this
    // exact `WHERE status = 'pending'` as what makes "an order is settled at
    // most once" true. Its `rows_affected()` is the boolean `spec.md`,
    // "Layers" calls "state changed" rather than success — here it is
    // always expected to be 1, because this function only runs once
    // `decide` has already read `payment.status == Pending` under the same
    // lock this UPDATE runs inside of; a `0` would mean that invariant broke
    // and is treated as a hard error rather than silently ignored.
    let result = sqlx::query(
        "UPDATE payments SET status = 'succeeded', card_brand = $1, card_last4 = $2, \
                updated_at = now() \
          WHERE id = $3 AND status = 'pending'",
    )
    .bind(&facts.card_brand)
    .bind(&facts.card_last4)
    .bind(payment.id)
    .execute(&mut **tx)
    .await?;
    if result.rows_affected() != 1 {
        return Err(Error::Database(sqlx::Error::RowNotFound));
    }

    sqlx::query(
        "UPDATE payment_attempts SET status = 'succeeded', rtn_code = $1, \
                provider_charge_id = $2, card_brand = $3, card_last4 = $4, eci = $5, \
                auth_code = $6, settled_at = now() \
          WHERE id = $7",
    )
    .bind(facts.rtn_code)
    .bind(&facts.provider_charge_id)
    .bind(&facts.card_brand)
    .bind(&facts.card_last4)
    .bind(&facts.eci)
    .bind(&facts.auth_code)
    .bind(attempt.id)
    .execute(&mut **tx)
    .await?;

    let payload = serde_json::json!({
        "amount": facts.amount,
        "currency": payment.currency,
        "card_brand": facts.card_brand,
        "provider_charge_id": facts.provider_charge_id,
        "reference": payment.merchant_trade_no,
        "source": "callback",
    });
    insert_event(
        tx,
        payment.id,
        payment.merchant_id,
        EventType::PaymentSucceeded,
        payload,
    )
    .await?;

    write_paid_notification(
        tx,
        payment,
        merchant,
        facts,
        merchant_hash_key,
        merchant_hash_iv,
    )
    .await?;
    Ok(())
}

/// Record that `SimulatePaid=1` named this attempt. The column exists
/// precisely so that a simulated notification is distinguishable, later,
/// from a real one that happened to arrive for the same attempt — no other
/// table or column carries that fact.
async fn mark_simulated(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    attempt_id: Uuid,
) -> Result<()> {
    sqlx::query("UPDATE payment_attempts SET simulate_paid = true WHERE id = $1")
        .bind(attempt_id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn fail_attempt(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    payment: &PaymentRow,
    attempt: &AttemptRow,
    facts: &CallbackFacts,
    failure_code: &str,
) -> Result<()> {
    let failure_code = facts
        .failure_code
        .clone()
        .unwrap_or_else(|| failure_code.to_string());
    sqlx::query(
        "UPDATE payment_attempts SET status = 'failed', rtn_code = $1, failure_code = $2 \
          WHERE id = $3",
    )
    .bind(facts.rtn_code)
    .bind(&failure_code)
    .bind(attempt.id)
    .execute(&mut **tx)
    .await?;

    let payload = serde_json::json!({
        "amount": facts.amount,
        "currency": payment.currency,
        "failure_code": failure_code,
        "reference": payment.merchant_trade_no,
        "source": "callback",
    });
    insert_event(
        tx,
        payment.id,
        payment.merchant_id,
        EventType::PaymentAttemptFailed,
        payload,
    )
    .await?;
    Ok(())
}

async fn record_duplicate(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    payment: &PaymentRow,
    attempt: &AttemptRow,
    facts: &CallbackFacts,
) -> Result<()> {
    // This attempt really was paid, so its own row reflects that — but the
    // ORDER is untouched: `amount_refunded` stays exactly what it was,
    // because that second charge was never this order's money (`spec.md`,
    // "When the customer pays twice").
    sqlx::query(
        "UPDATE payment_attempts SET status = 'succeeded', rtn_code = $1, \
                provider_charge_id = $2, card_brand = $3, card_last4 = $4, eci = $5, \
                auth_code = $6, settled_at = now() \
          WHERE id = $7",
    )
    .bind(facts.rtn_code)
    .bind(&facts.provider_charge_id)
    .bind(&facts.card_brand)
    .bind(&facts.card_last4)
    .bind(&facts.eci)
    .bind(&facts.auth_code)
    .bind(attempt.id)
    .execute(&mut **tx)
    .await?;

    let payload = serde_json::json!({
        "amount": facts.amount,
        "currency": payment.currency,
        "attempt_id": attempt.id,
        "reference": payment.merchant_trade_no,
    });
    insert_event(
        tx,
        payment.id,
        payment.merchant_id,
        EventType::PaymentDuplicatePaid,
        payload,
    )
    .await?;
    Ok(())
}

async fn write_paid_notification(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    payment: &PaymentRow,
    merchant: &MerchantContext,
    facts: &CallbackFacts,
    merchant_hash_key: &str,
    merchant_hash_iv: &str,
) -> Result<()> {
    let paid_at = facts.paid_at.unwrap_or_else(chrono::Utc::now);
    let notify_facts = MerchantNotifyFacts {
        provider_merchant_id: merchant.provider_merchant_id.clone(),
        merchant_trade_no: merchant.merchant_trade_no.clone(),
        payment_id: payment.id,
        rtn_code: 1,
        rtn_msg: "paid",
        amount: facts.amount,
        paid_at,
    };
    let mut fields = merchant_notify_fields(&notify_facts);
    let mac =
        paygate_provider::ecpay::check_mac_value(merchant_hash_key, merchant_hash_iv, &fields);
    fields.insert("CheckMacValue".to_string(), mac);
    let payload = serde_json::to_value(&fields).unwrap_or(Value::Null);

    insert_notification(
        tx,
        payment.id,
        merchant.merchant_id,
        &merchant.notify_url,
        payload,
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn payment(status: PaymentStatus, amount: Amount) -> PaymentRow {
        PaymentRow {
            id: Uuid::nil(),
            merchant_id: 1,
            merchant_trade_no: "ACME-1".to_string(),
            amount,
            currency: "USD".to_string(),
            status,
            item_desc: "Beans".to_string(),
            card_brand: None,
            card_last4: None,
            notify_url: "/demo-merchant/api/notify".to_string(),
            client_back_url: "/shop/result".to_string(),
            amount_refunded: 0,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn attempt(status: AttemptStatus) -> AttemptRow {
        AttemptRow {
            id: Uuid::from_u128(1),
            payment_id: Uuid::nil(),
            provider_code: "ecpay".to_string(),
            provider_trade_no: "ACME00000001SNabcde".to_string(),
            status,
            rtn_code: None,
            simulate_paid: false,
            failure_code: None,
            provider_charge_id: None,
            card_brand: None,
            card_last4: None,
            eci: None,
            auth_code: None,
            started_at: Utc::now(),
            queried_at: None,
            settled_at: None,
        }
    }

    fn facts(rtn_code: i32, amount: Amount, simulate_paid: bool) -> CallbackFacts {
        CallbackFacts {
            provider_trade_no: "ACME00000001SNabcde".to_string(),
            provider_event_id: "evt-1".to_string(),
            provider_charge_id: "ch-1".to_string(),
            rtn_code,
            rtn_msg: String::new(),
            amount,
            simulate_paid,
            failure_code: None,
            card_brand: Some("visa".to_string()),
            card_last4: Some("4242".to_string()),
            eci: None,
            auth_code: None,
            paid_at: Some(Utc::now()),
        }
    }

    #[test]
    fn a_matching_paid_callback_against_a_pending_order_settles_it() {
        let p = payment(PaymentStatus::Pending, 1000);
        let a = attempt(AttemptStatus::Redirected);
        let d = decide(&p, &a, &facts(1, 1000, false));
        assert_eq!(d.outcome(), CallbackOutcome::Applied);
        assert!(matches!(d, Decision::Settled));
    }

    #[test]
    fn an_amount_mismatch_is_refused_however_the_payment_stands() {
        let p = payment(PaymentStatus::Pending, 1000);
        let a = attempt(AttemptStatus::Redirected);
        let d = decide(&p, &a, &facts(1, 999, false));
        assert_eq!(d.outcome(), CallbackOutcome::AmountMismatch);
    }

    #[test]
    fn a_simulated_payment_settles_nothing_even_if_it_would_have_been_paid() {
        let p = payment(PaymentStatus::Pending, 1000);
        let a = attempt(AttemptStatus::Redirected);
        let d = decide(&p, &a, &facts(1, 1000, true));
        assert_eq!(d.outcome(), CallbackOutcome::NoOp);
        assert!(matches!(d, Decision::Simulated));
    }

    #[test]
    fn simulate_paid_overrides_even_a_genuine_decline() {
        let p = payment(PaymentStatus::Pending, 1000);
        let a = attempt(AttemptStatus::Redirected);
        let d = decide(&p, &a, &facts(10_100_058, 1000, true));
        assert_eq!(d.outcome(), CallbackOutcome::NoOp);
        assert!(matches!(d, Decision::Simulated));
    }

    #[test]
    fn a_decline_against_a_pending_order_fails_the_attempt() {
        let p = payment(PaymentStatus::Pending, 1000);
        let a = attempt(AttemptStatus::Redirected);
        let d = decide(&p, &a, &facts(10_100_058, 1000, false));
        assert_eq!(d.outcome(), CallbackOutcome::Applied);
        assert!(matches!(d, Decision::FailedAttempt { .. }));
    }

    #[test]
    fn an_unknown_return_code_is_recorded_and_acted_on_in_no_way() {
        let p = payment(PaymentStatus::Pending, 1000);
        let a = attempt(AttemptStatus::Redirected);
        let d = decide(&p, &a, &facts(10_300_066, 1000, false));
        assert_eq!(d.outcome(), CallbackOutcome::UnknownCode);
        let d = decide(&p, &a, &facts(999_999, 1000, false));
        assert_eq!(d.outcome(), CallbackOutcome::UnknownCode);
    }

    #[test]
    fn a_paid_callback_for_a_different_attempt_of_a_settled_order_is_duplicate() {
        let p = payment(PaymentStatus::Succeeded, 1000);
        let a = attempt(AttemptStatus::Redirected);
        let d = decide(&p, &a, &facts(1, 1000, false));
        assert_eq!(d.outcome(), CallbackOutcome::Duplicate);
    }

    #[test]
    fn a_decline_against_an_already_settled_order_is_a_conflict_not_a_failure() {
        let p = payment(PaymentStatus::Succeeded, 1000);
        let a = attempt(AttemptStatus::Redirected);
        let d = decide(&p, &a, &facts(10_100_058, 1000, false));
        assert_eq!(d.outcome(), CallbackOutcome::Conflict);
    }

    #[test]
    fn a_paid_callback_replaying_for_an_already_succeeded_attempt_is_a_no_op() {
        // The exact race the reconciler creates: the query settled this very
        // attempt already, and now the original callback turns up too.
        let p = payment(PaymentStatus::Succeeded, 1000);
        let a = attempt(AttemptStatus::Succeeded);
        let d = decide(&p, &a, &facts(1, 1000, false));
        assert_eq!(d.outcome(), CallbackOutcome::NoOp);
        assert!(matches!(d, Decision::ReplayOfResolvedAttempt));
    }

    #[test]
    fn a_decline_replaying_for_an_already_failed_attempt_is_a_no_op() {
        let p = payment(PaymentStatus::Pending, 1000);
        let a = attempt(AttemptStatus::Failed);
        let d = decide(&p, &a, &facts(10_100_058, 1000, false));
        assert_eq!(d.outcome(), CallbackOutcome::NoOp);
    }

    #[test]
    fn should_acknowledge_is_false_only_for_amount_mismatch() {
        assert!(!should_acknowledge(CallbackOutcome::AmountMismatch));
        assert!(should_acknowledge(CallbackOutcome::Applied));
        assert!(should_acknowledge(CallbackOutcome::NoOp));
        assert!(should_acknowledge(CallbackOutcome::Conflict));
        assert!(should_acknowledge(CallbackOutcome::Duplicate));
        assert!(should_acknowledge(CallbackOutcome::UnknownCode));
    }
}
