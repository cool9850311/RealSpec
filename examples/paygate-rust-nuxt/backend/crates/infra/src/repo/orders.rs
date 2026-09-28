//! `create_order` and `open_attempt` — the two writes behind `POST /payments`
//! (`spec.md`, "One order, three numbers"; `handoff.feature`). Two separate
//! transactions, not one: the order is created once, and a hand-off may be
//! asked for again and again against the same order (a reload, a decline
//! retried), each time opening a fresh attempt with its own
//! `provider_trade_no` — never reusing the last one, because ECPay refuses an
//! order number it has already seen (`spec.md`, "One order, three numbers").

use paygate_domain::{provider_trade_no, Amount, EventType, MerchantId};
use rand::rngs::OsRng;
use uuid::Uuid;

use crate::db::Db;
use crate::error::{Error, Result};

use super::{insert_event, is_unique_violation, lock_payment, map_payment_row, PaymentRow};

/// The merchant-supplied half of a new order, already validated by
/// `paygate_domain::validate_create_order` (a caller's job, not this one's —
/// this function's contract is "insert exactly this row" and it trusts its
/// caller to have refused a malformed one already).
#[derive(Debug, Clone)]
pub struct NewOrder {
    pub merchant_id: MerchantId,
    pub merchant_trade_no: String,
    pub amount: Amount,
    pub currency: String,
    pub item_desc: String,
    pub notify_url: String,
    pub client_back_url: String,
}

/// Insert the order, and its `PaymentCreated` audit row, in one transaction.
/// `Err(Error::MerchantTradeNoTaken)` is `UNIQUE (merchant_id, merchant_trade_no)`
/// firing — the merchant already used this number, whatever became of that
/// order (`spec.md`, "Two kinds of duplicate").
pub async fn create_order(db: &Db, order: NewOrder) -> Result<PaymentRow> {
    let mut tx = db.begin().await?;
    let id = Uuid::now_v7();

    let insert = sqlx::query(
        "INSERT INTO payments \
            (id, merchant_id, merchant_trade_no, amount, currency, status, item_desc, \
             notify_url, client_back_url) \
         VALUES ($1, $2, $3, $4, $5, 'pending', $6, $7, $8)",
    )
    .bind(id)
    .bind(order.merchant_id)
    .bind(&order.merchant_trade_no)
    .bind(order.amount)
    .bind(&order.currency)
    .bind(&order.item_desc)
    .bind(&order.notify_url)
    .bind(&order.client_back_url)
    .execute(&mut *tx)
    .await;

    if let Err(err) = insert {
        if is_unique_violation(&err) {
            return Err(Error::MerchantTradeNoTaken);
        }
        return Err(err.into());
    }

    let payload = serde_json::json!({
        "amount": order.amount,
        "currency": order.currency,
    });
    insert_event(
        &mut tx,
        id,
        order.merchant_id,
        EventType::PaymentCreated,
        payload,
    )
    .await?;

    let row = sqlx::query(
        "SELECT id, merchant_id, merchant_trade_no, amount, currency, status, item_desc, \
                card_brand, card_last4, notify_url, client_back_url, amount_refunded, \
                created_at, updated_at \
           FROM payments WHERE id = $1",
    )
    .bind(id)
    .fetch_one(&mut *tx)
    .await?;
    let payment = map_payment_row(&row)?;

    tx.commit().await?;
    Ok(payment)
}

/// A freshly-opened attempt: the row, and the trade number a hand-off signs.
#[derive(Debug, Clone)]
pub struct OpenedAttempt {
    pub attempt_id: Uuid,
    pub provider_trade_no: String,
}

/// Open a new attempt against an existing, `pending` order: mint a
/// `provider_trade_no` that has never been used at this provider, insert the
/// `payment_attempts` row, and append `PaymentAttemptStarted`
/// (`payload.provider_code`) — all in one transaction, under the payment's own
/// row lock so a caller cannot open an attempt against an order it does not
/// hold (or that has since settled from under it without the caller knowing).
///
/// A collision on `UNIQUE (provider_code, provider_trade_no)` — astronomically
/// unlikely given the five-character random suffix, but the schema's own
/// backstop rather than this function's word for it (`spec.md`, "One order,
/// three numbers") — is retried with a fresh number, bounded, so a true
/// exhaustion of the number space fails loudly rather than looping forever.
pub async fn open_attempt(
    db: &Db,
    merchant_id: MerchantId,
    payment_id: Uuid,
    provider_code: paygate_domain::ProviderCode,
    trade_no_prefix: &str,
) -> Result<OpenedAttempt> {
    const MAX_ATTEMPTS: u32 = 5;
    let provider_code_str = serde_json::to_value(provider_code)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default();

    for _ in 0..MAX_ATTEMPTS {
        let mut tx = db.begin().await?;

        let payment = lock_payment(&mut tx, payment_id, merchant_id)
            .await?
            .ok_or(Error::PaymentNotFound)?;
        if payment.status != paygate_domain::PaymentStatus::Pending {
            // A settled order simply has nothing left to hand off; the
            // caller (api) turns this into whatever the endpoint's own
            // contract requires.
            return Err(Error::PaymentNotPending);
        }

        let attempt_id = Uuid::now_v7();
        let sequence = time_based_sequence();
        let trade_no = provider_trade_no(trade_no_prefix, sequence, &mut OsRng);

        let insert = sqlx::query(
            "INSERT INTO payment_attempts (id, payment_id, provider_code, provider_trade_no, status) \
             VALUES ($1, $2, $3, $4, 'redirected')",
        )
        .bind(attempt_id)
        .bind(payment_id)
        .bind(&provider_code_str)
        .bind(&trade_no)
        .execute(&mut *tx)
        .await;

        if let Err(err) = insert {
            if is_unique_violation(&err) {
                continue; // provider_trade_no collision: try again with a fresh one.
            }
            return Err(err.into());
        }

        let payload = serde_json::json!({ "provider_code": provider_code_str });
        insert_event(
            &mut tx,
            payment_id,
            merchant_id,
            EventType::PaymentAttemptStarted,
            payload,
        )
        .await?;

        tx.commit().await?;
        return Ok(OpenedAttempt {
            attempt_id,
            provider_trade_no: trade_no,
        });
    }

    Err(Error::Database(sqlx::Error::Protocol(
        "exhausted retries minting a unique provider_trade_no".to_string(),
    )))
}

/// A coarse, globally-shared but uncoordinated sequence: seconds since the
/// Unix epoch, folded into eight digits. It exists only to make
/// `provider_trade_no` a little more legible (roughly time-ordered); the
/// five-character random suffix is what `paygate_domain`'s own tests already
/// prove is enough to keep a thousand numbers apart even with this value held
/// FIXED (`trade_no.rs`, "a thousand numbers for one order are all
/// distinct") — so no coordinated counter is needed across replicas, and the
/// real backstop is (and must remain) the schema's own `UNIQUE` constraint.
fn time_based_sequence() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    secs % 100_000_000
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_based_sequence_is_eight_digits_or_fewer() {
        assert!(time_based_sequence() < 100_000_000);
    }
}
