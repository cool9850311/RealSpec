//! `settle_from_query`, `abandon_attempt`, and the plain "nothing changed"
//! recording every reconciler pass needs — the writes behind
//! `spec.md`, "Reconciling what never came back".
//!
//! Unlike the rest of `repo`, these three take an already-open
//! `&mut Transaction` rather than a `&Db` (every repository here takes
//! `&mut Transaction` or `&PgPool`) — on purpose. The
//! SELECTION query (only `redirected`, only older than the window,
//! `SKIP LOCKED`, batch-limited) lives in [`crate::reconciler`], and it must
//! hold that `SKIP LOCKED` row lock for the ENTIRE round trip to the
//! provider, or a second reconciler replica could select and query the same
//! attempt before the first one ever writes anything —
//! `scaling.feature`'s "one query reaches the provider, not two" is
//! precisely the guarantee a lock released between the selection and the
//! write would break. So [`crate::reconciler`] opens the transaction, holds
//! it across the `await` on the provider call, and hands it to whichever of
//! these three functions the answer calls for; only it commits.
//!
//! Every one of these functions writes a `provider_queries` row — an asking
//! is a fact worth keeping however it turns out, and `spec.md`, "Reconciling
//! what never came back" is explicit that even a forged answer or a `403`
//! "counts as an asking", or the next pass would queue up another one and
//! extend the blackout.

use paygate_domain::{Amount, AttemptStatus, EventType, MerchantId, PaymentStatus};
use paygate_provider::CallbackFacts;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::error::{Error, Result};

use super::{
    insert_event, insert_notification, lock_attempt_by_id, lock_payment_by_id,
    merchant_notify_fields, MerchantContext, MerchantNotifyFacts,
};

async fn insert_provider_query(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    attempt_id: Uuid,
    trade_status: Option<&str>,
    raw: &str,
) -> Result<()> {
    sqlx::query("INSERT INTO provider_queries (attempt_id, trade_status, raw) VALUES ($1, $2, $3)")
        .bind(attempt_id)
        .bind(trade_status)
        .bind(raw)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// `TradeStatus = 0` ("still in progress"), a verification failure, or a
/// `403` throttle: nothing about the attempt or the payment changes, but the
/// asking itself is recorded and `queried_at` moves, so the retry window
/// (`RECONCILE_RETRY_MINUTES`) actually holds off the next pass.
///
/// Only ever takes the ONE lock, on `payment_attempts` — never `payments` —
/// which cannot by itself be half of an ABBA cycle. It is safe precisely
/// because it never has to be the first lock this transaction takes: the
/// caller's claim (`reconciler::select_candidates`) already locked this
/// attempt's own `payments` row first, canonical order, before this
/// function is ever reached.
pub async fn record_unresolved_query(
    tx: &mut Transaction<'_, Postgres>,
    attempt_id: Uuid,
    trade_status: Option<&str>,
    raw: &str,
) -> Result<()> {
    lock_attempt_by_id(tx, attempt_id)
        .await?
        .ok_or(Error::PaymentNotFound)?;
    insert_provider_query(tx, attempt_id, trade_status, raw).await?;
    sqlx::query("UPDATE payment_attempts SET queried_at = now() WHERE id = $1")
        .bind(attempt_id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// What `settle_from_query` or `abandon_attempt` did — deliberately its own
/// small vocabulary rather than the full [`paygate_domain::CallbackOutcome`],
/// since the reconciler asks about exactly one attempt and only ever gets a
/// definite `TradeStatus`, never a signature failure at this layer (that is
/// verified by the caller before either of these is reached, the same
/// division of labour as `apply_callback`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconcileOutcome {
    /// The order was still `pending` and this query is what settled it.
    Settled,
    /// This attempt had already resolved (a callback won the race, or a
    /// previous pass already asked and got the same answer) — nothing to do
    /// beyond recording the asking.
    NoOp,
    /// The order had already settled through a DIFFERENT attempt; this one's
    /// money moved too.
    Duplicate,
    /// The query's own amount did not match the order's — refused the same
    /// way a callback's would be, never settled on a guess.
    AmountMismatch,
    /// `abandon_attempt` only: the provider said this attempt was never
    /// completed, and the order was still `pending` to give up on.
    Abandoned,
}

#[derive(Debug, Clone)]
pub struct ReconcileApplication {
    pub outcome: ReconcileOutcome,
    pub payment_id: Uuid,
    pub attempt_id: Uuid,
    pub payment_status: PaymentStatus,
    pub duplicate_amount: Option<Amount>,
}

/// `TradeStatus = 1`: the provider says this attempt paid. Settle late
/// (`PaymentReconciled`, `payload.source = "query"`) if the order is still
/// `pending`; otherwise this is exactly the duplicate-payment case, found by
/// asking instead of by a callback.
#[allow(clippy::too_many_arguments)]
pub async fn settle_from_query(
    tx: &mut Transaction<'_, Postgres>,
    attempt_id: Uuid,
    facts: &CallbackFacts,
    raw_answer: &str,
    merchant: &MerchantContext,
    merchant_hash_key: &str,
    merchant_hash_iv: &str,
) -> Result<ReconcileApplication> {
    // Canonical lock order — `payments` before `payment_attempts` (see the
    // note on `lock_payment_by_id`/`lock_attempt_by_id` in `repo/mod.rs`,
    // and `repo::refunds::create_refund`, which already takes them this
    // way). Locking the attempt first here — as this used to — is the same
    // inversion `repo::callbacks` had, and worse for the reconciler: this
    // function's caller (`reconciler::run_pass`) holds its transaction open
    // across an entire round trip to the provider, once per attempt in the
    // batch, so the window in which the wrong-order lock would be held is
    // seconds, not microseconds. `payment_id` is read WITHOUT a lock first,
    // only so `payments` can be locked before `payment_attempts` at all —
    // sound because an attempt's own `payment_id` never changes once
    // written.
    //
    // In `run_pass`'s own real flow, `reconciler::select_candidates` already
    // claimed both rows, in this same order, before this function is ever
    // called — see its own doc comment for why the CLAIM's own order is what
    // actually matters for that caller, since it takes the attempt's lock
    // before any candidate is ever handed here. The two lock calls below are
    // then re-acquisitions of locks this same transaction already holds
    // (cheap, and never blocking). Taking them in this order here too is
    // what keeps this function itself correct for any OTHER caller that
    // reaches it without going through that claim — this crate's own test
    // suite among them.
    let payment_id: Uuid =
        sqlx::query_scalar("SELECT payment_id FROM payment_attempts WHERE id = $1")
            .bind(attempt_id)
            .fetch_optional(&mut **tx)
            .await?
            .ok_or(Error::PaymentNotFound)?;
    let payment = lock_payment_by_id(tx, payment_id)
        .await?
        .ok_or(Error::PaymentNotFound)?;
    let attempt = lock_attempt_by_id(tx, attempt_id)
        .await?
        .ok_or(Error::PaymentNotFound)?;

    insert_provider_query(tx, attempt_id, Some("1"), raw_answer).await?;

    let outcome = if facts.amount != payment.amount {
        ReconcileOutcome::AmountMismatch
    } else if attempt.status == AttemptStatus::Succeeded {
        ReconcileOutcome::NoOp
    } else if payment.status == PaymentStatus::Pending {
        ReconcileOutcome::Settled
    } else {
        ReconcileOutcome::Duplicate
    };

    let mut duplicate_amount = None;

    match outcome {
        ReconcileOutcome::AmountMismatch | ReconcileOutcome::NoOp => {
            sqlx::query("UPDATE payment_attempts SET queried_at = now() WHERE id = $1")
                .bind(attempt_id)
                .execute(&mut **tx)
                .await?;
        }
        ReconcileOutcome::Settled => {
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
                        auth_code = $6, settled_at = now(), queried_at = now() \
                  WHERE id = $7",
            )
            .bind(facts.rtn_code)
            .bind(&facts.provider_charge_id)
            .bind(&facts.card_brand)
            .bind(&facts.card_last4)
            .bind(&facts.eci)
            .bind(&facts.auth_code)
            .bind(attempt_id)
            .execute(&mut **tx)
            .await?;

            let payload = serde_json::json!({
                "amount": facts.amount,
                "currency": payment.currency,
                "card_brand": facts.card_brand,
                "provider_charge_id": facts.provider_charge_id,
                "reference": payment.merchant_trade_no,
                "source": "query",
            });
            insert_event(
                tx,
                payment.id,
                payment.merchant_id,
                EventType::PaymentReconciled,
                payload,
            )
            .await?;

            let notify_facts = MerchantNotifyFacts {
                provider_merchant_id: merchant.provider_merchant_id.clone(),
                merchant_trade_no: merchant.merchant_trade_no.clone(),
                payment_id: payment.id,
                rtn_code: 1,
                rtn_msg: "paid",
                amount: facts.amount,
                paid_at: facts.paid_at.unwrap_or_else(chrono::Utc::now),
            };
            let mut fields = merchant_notify_fields(&notify_facts);
            let mac = paygate_provider::ecpay::check_mac_value(
                merchant_hash_key,
                merchant_hash_iv,
                &fields,
            );
            fields.insert("CheckMacValue".to_string(), mac);
            let payload = serde_json::to_value(&fields).unwrap_or(serde_json::Value::Null);
            insert_notification(
                tx,
                payment.id,
                merchant.merchant_id,
                &merchant.notify_url,
                payload,
            )
            .await?;
        }
        ReconcileOutcome::Duplicate => {
            sqlx::query(
                "UPDATE payment_attempts SET status = 'succeeded', rtn_code = $1, \
                        provider_charge_id = $2, card_brand = $3, card_last4 = $4, eci = $5, \
                        auth_code = $6, settled_at = now(), queried_at = now() \
                  WHERE id = $7",
            )
            .bind(facts.rtn_code)
            .bind(&facts.provider_charge_id)
            .bind(&facts.card_brand)
            .bind(&facts.card_last4)
            .bind(&facts.eci)
            .bind(&facts.auth_code)
            .bind(attempt_id)
            .execute(&mut **tx)
            .await?;

            let payload = serde_json::json!({
                "amount": facts.amount,
                "currency": payment.currency,
                "attempt_id": attempt_id,
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
            duplicate_amount = Some(facts.amount);
        }
        ReconcileOutcome::Abandoned => {
            unreachable!("settle_from_query's own decision never produces Abandoned")
        }
    }

    let final_status: (String,) = sqlx::query_as("SELECT status FROM payments WHERE id = $1")
        .bind(payment.id)
        .fetch_one(&mut **tx)
        .await?;
    let payment_status: PaymentStatus = super::parse_enum_column("status", &final_status.0)?;

    Ok(ReconcileApplication {
        outcome,
        payment_id: payment.id,
        attempt_id,
        payment_status,
        duplicate_amount,
    })
}

/// `TradeStatus = 10200095`: the provider says the consumer never completed
/// this attempt. The only word that may end an attempt this way
/// (`spec.md`, "Reconciling what never came back": "Only the provider may
/// end an attempt").
pub async fn abandon_attempt(
    tx: &mut Transaction<'_, Postgres>,
    attempt_id: Uuid,
    raw_answer: &str,
) -> Result<ReconcileApplication> {
    // Canonical lock order — see the matching note in `settle_from_query`,
    // above, and `repo/mod.rs`: `payments` before `payment_attempts`,
    // `payment_id` read unlocked first only to know which payment to lock.
    let payment_id: Uuid =
        sqlx::query_scalar("SELECT payment_id FROM payment_attempts WHERE id = $1")
            .bind(attempt_id)
            .fetch_optional(&mut **tx)
            .await?
            .ok_or(Error::PaymentNotFound)?;
    let payment = lock_payment_by_id(tx, payment_id)
        .await?
        .ok_or(Error::PaymentNotFound)?;
    let attempt = lock_attempt_by_id(tx, attempt_id)
        .await?
        .ok_or(Error::PaymentNotFound)?;

    insert_provider_query(tx, attempt_id, Some("10200095"), raw_answer).await?;

    let outcome = if attempt.status == AttemptStatus::Redirected {
        sqlx::query(
            "UPDATE payment_attempts SET status = 'abandoned', queried_at = now() WHERE id = $1",
        )
        .bind(attempt_id)
        .execute(&mut **tx)
        .await?;

        if payment.status == PaymentStatus::Pending {
            let payload = serde_json::json!({
                "attempt_id": attempt_id,
                "reference": payment.merchant_trade_no,
            });
            insert_event(
                tx,
                payment.id,
                payment.merchant_id,
                EventType::PaymentAttemptAbandoned,
                payload,
            )
            .await?;
            ReconcileOutcome::Abandoned
        } else {
            ReconcileOutcome::NoOp
        }
    } else {
        sqlx::query("UPDATE payment_attempts SET queried_at = now() WHERE id = $1")
            .bind(attempt_id)
            .execute(&mut **tx)
            .await?;
        ReconcileOutcome::NoOp
    };

    let final_status: (String,) = sqlx::query_as("SELECT status FROM payments WHERE id = $1")
        .bind(payment.id)
        .fetch_one(&mut **tx)
        .await?;
    let payment_status: PaymentStatus = super::parse_enum_column("status", &final_status.0)?;

    Ok(ReconcileApplication {
        outcome,
        payment_id: payment.id,
        attempt_id,
        payment_status,
        duplicate_amount: None,
    })
}

/// The reconciler's own counterpart to the callback path's
/// `refund_flow::execute(..., Some("duplicate"), ...)`
/// (`crates/api/src/routes/webhooks.rs`): [`settle_from_query`]'s
/// `Duplicate` outcome records the second charge (`PaymentDuplicatePaid`)
/// but reserves nothing to give it back — that is this function's job.
/// Opens the `refunds` row `pending`, reason `duplicate`, against the
/// DUPLICATED attempt itself (never the attempt that actually settled the
/// order), and leaves it there for
/// [`crate::reconciler::resend_pending_refunds`] to deliver, this pass or
/// the next — "sending a refund twice is safe... the provider deduplicates
/// on the refund id paygate chose" (`spec.md`, "Reconciling what never came
/// back").
///
/// Deliberately takes the SAME already-open `tx` [`settle_from_query`] just
/// wrote to, rather than `repo::refunds::create_refund`'s `&Db` (which opens
/// its own connection and transaction): `run_pass` holds this transaction's
/// row locks on `payments`/`payment_attempts` for the whole pass
/// (`scaling.feature`, "one query reaches the provider, not two"), and a
/// second connection trying to lock the very same `payments` row before
/// this one commits would simply block on itself for the rest of the pass.
///
/// Mirrors `repo::refunds::create_refund`'s own `reason = "duplicate"` shape
/// exactly (see its module doc): no bound against `amount - amount_refunded`,
/// and — because this INSERT is the only write this function makes — no
/// touch at all to `payments.amount_refunded` or `payments.status`. That
/// second charge was never the order's own money.
pub async fn create_duplicate_refund(
    tx: &mut Transaction<'_, Postgres>,
    merchant_id: MerchantId,
    payment_id: Uuid,
    attempt_id: Uuid,
    amount: Amount,
) -> Result<Uuid> {
    let refund_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO refunds (id, payment_id, attempt_id, merchant_id, amount, status, reason) \
         VALUES ($1, $2, $3, $4, $5, 'pending', 'duplicate')",
    )
    .bind(refund_id)
    .bind(payment_id)
    .bind(attempt_id)
    .bind(merchant_id)
    .bind(amount)
    .execute(&mut **tx)
    .await?;
    Ok(refund_id)
}
