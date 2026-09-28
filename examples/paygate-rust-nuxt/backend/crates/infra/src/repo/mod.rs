//! The write paths. Every command that changes a payment goes through one of
//! [`orders`], [`callbacks`], [`reconcile`], [`refunds`] or [`idempotency`],
//! and each one that changes state does `SELECT ... FOR UPDATE` on the
//! `payments` row and decides against what it read under that lock, in ONE
//! transaction that also writes the `payment_attempts` row, the append-only
//! `payment_events` row (with its `seq`), and — when there is an outcome to
//! report — the `notifications` row (`spec.md`, "Two tables, one
//! transaction").
//!
//! This module holds the shared row types and the small pieces of plumbing
//! every one of those transactions needs (locking a payment, computing the
//! next `payment_events.seq`, inserting the audit row, inserting the
//! notification row) so the five submodules read as the business rule they
//! implement, not as repeated SQL boilerplate.

mod callbacks;
mod idempotency;
mod orders;
mod reconcile;
mod refunds;

pub use callbacks::*;
pub use idempotency::*;
pub use orders::*;
pub use reconcile::*;
pub use refunds::*;

use chrono::{DateTime, Utc};
use paygate_domain::{Amount, AttemptStatus, MerchantId, PaymentStatus};
use serde::de::DeserializeOwned;
use serde_json::Value;
use sqlx::postgres::Postgres;
use sqlx::{Row, Transaction};
use uuid::Uuid;

use crate::error::{Error, Result};

/// The full `payments` row, mapped by hand (rather than `#[derive(FromRow)]`)
/// because its `status` column is a `TEXT` the domain crate's enums have to
/// be parsed out of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaymentRow {
    pub id: Uuid,
    pub merchant_id: MerchantId,
    pub merchant_trade_no: String,
    pub amount: Amount,
    pub currency: String,
    pub status: PaymentStatus,
    pub item_desc: String,
    pub card_brand: Option<String>,
    pub card_last4: Option<String>,
    pub notify_url: String,
    pub client_back_url: String,
    pub amount_refunded: Amount,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// The full `payment_attempts` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttemptRow {
    pub id: Uuid,
    pub payment_id: Uuid,
    pub provider_code: String,
    pub provider_trade_no: String,
    pub status: AttemptStatus,
    pub rtn_code: Option<i32>,
    pub simulate_paid: bool,
    pub failure_code: Option<String>,
    pub provider_charge_id: Option<String>,
    pub card_brand: Option<String>,
    pub card_last4: Option<String>,
    pub eci: Option<String>,
    pub auth_code: Option<String>,
    pub started_at: DateTime<Utc>,
    pub queried_at: Option<DateTime<Utc>>,
    pub settled_at: Option<DateTime<Utc>>,
}

/// A domain enum stored as `TEXT` deserializes through the same
/// `#[serde(rename_all = "snake_case")]` implementation the schema's literal
/// strings were checked against in `paygate-domain`'s own tests — so reading
/// a row's text column back into one of these enums (`crates/domain/src/enums.rs`'s
/// own doc comment) is exactly this: wrap the text in a JSON string and let
/// `serde` do the matching it already promises.
pub(crate) fn parse_enum_column<T: DeserializeOwned>(column: &'static str, s: &str) -> Result<T> {
    serde_json::from_value(Value::String(s.to_string())).map_err(|_| {
        Error::Database(sqlx::Error::ColumnDecode {
            index: column.to_string(),
            source: format!("{s:?} is not a valid value for {column}").into(),
        })
    })
}

pub(crate) fn map_payment_row(row: &sqlx::postgres::PgRow) -> Result<PaymentRow> {
    let status: String = row.try_get("status")?;
    Ok(PaymentRow {
        id: row.try_get("id")?,
        merchant_id: row.try_get("merchant_id")?,
        merchant_trade_no: row.try_get("merchant_trade_no")?,
        amount: row.try_get("amount")?,
        currency: row.try_get("currency")?,
        status: parse_enum_column("status", &status)?,
        item_desc: row.try_get("item_desc")?,
        card_brand: row.try_get("card_brand")?,
        card_last4: row.try_get("card_last4")?,
        notify_url: row.try_get("notify_url")?,
        client_back_url: row.try_get("client_back_url")?,
        amount_refunded: row.try_get("amount_refunded")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

pub(crate) fn map_attempt_row(row: &sqlx::postgres::PgRow) -> Result<AttemptRow> {
    let status: String = row.try_get("status")?;
    Ok(AttemptRow {
        id: row.try_get("id")?,
        payment_id: row.try_get("payment_id")?,
        provider_code: row.try_get("provider_code")?,
        provider_trade_no: row.try_get("provider_trade_no")?,
        status: parse_enum_column("status", &status)?,
        rtn_code: row.try_get("rtn_code")?,
        simulate_paid: row.try_get("simulate_paid")?,
        failure_code: row.try_get("failure_code")?,
        provider_charge_id: row.try_get("provider_charge_id")?,
        card_brand: row.try_get("card_brand")?,
        card_last4: row.try_get("card_last4")?,
        eci: row.try_get("eci")?,
        auth_code: row.try_get("auth_code")?,
        started_at: row.try_get("started_at")?,
        queried_at: row.try_get("queried_at")?,
        settled_at: row.try_get("settled_at")?,
    })
}

/// LOCK ORDER INVARIANT: whenever one transaction needs `FOR UPDATE` on both
/// a `payments` row and a `payment_attempts` row, it MUST lock `payments`
/// FIRST, then `payment_attempts` — parent before child, never the reverse.
/// Every one of these four `lock_*` helpers is safe to call on its own; the
/// invariant is about the ORDER two of them are called in, together, inside
/// one transaction, which the compiler cannot check for you — only a reader
/// (or a test that actually races two transactions) can.
///
/// Why `payments` anchors the order rather than `payment_attempts`: a
/// `payment_attempts` row always belongs to exactly one `payments` row
/// (`payment_attempts.payment_id`), never the other way around, and some
/// write paths (`repo::orders::create_order`) touch `payments` alone, with
/// no attempt in the picture at all. Anchoring the global order on the table
/// every payment-touching transaction has in common — rather than on the
/// child table only some of them touch — is what lets every function follow
/// the same order without any of them needing to know what any other one
/// does.
///
/// Breaking it is a textbook ABBA deadlock: transaction A locks `payments`
/// then waits on `payment_attempts`; transaction B has already locked that
/// same `payment_attempts` row and is waiting on that same `payments` row.
/// Neither can proceed and neither is wrong to wait — PostgreSQL can only
/// detect the cycle and abort one side (`40P01 deadlock detected`), never
/// prevent it. This is not hypothetical:
///
/// - `repo::callbacks::apply_callback` used to lock the attempt first (it
///   starts from the attempt's provider identity, not a payment id) while
///   `repo::refunds::create_refund` locks the payment first — a refund
///   racing a callback on the same payment deadlocked for real, reproduced
///   by `concurrent_refund_and_callback_on_the_same_payment_never_deadlock`
///   in `tests/postgres_repo.rs` (35 of 40 racing pairs hit `40P01` before
///   the fix, 0 after). Fixed by resolving the payment id with an UNLOCKED
///   read first — see `apply_callback`'s own comment for why that is sound
///   — then taking both locks in the canonical order.
/// - `repo::reconcile::settle_from_query` and `repo::reconcile::abandon_attempt`
///   have this SAME inversion (`lock_attempt_by_id` then `lock_payment_by_id`)
///   as of this writing, and have not yet been fixed. There the exposure is
///   worse, not equal: the reconciler deliberately holds one transaction
///   open across every provider HTTP call in a batch (so that only one query
///   reaches the provider, not one per attempt), so the misordered attempt
///   lock there is held for however long the provider takes to answer —
///   up to seconds, times the batch size — not the microseconds an ordinary
///   callback holds it for. A merchant refund landing during a
///   reconciliation pass has a correspondingly wider window to deadlock
///   against it.
///
/// If a function only ever starts from the attempt's identity (its provider
/// trade number, or its own id) rather than a payment id, resolve the
/// payment id it belongs to with an UNLOCKED read first — an attempt's
/// `payment_id` is written once, at `INSERT`, and never changed again by any
/// code in this crate, so reading it unlocked now and re-reading (and
/// locking) the same attempt under lock a moment later observes the same
/// fact, just at two different times — then lock `payments`, then
/// `payment_attempts`. Never lock the attempt first "to get to" the payment.
///
/// `SELECT ... FOR UPDATE` on one payment, scoped to `merchant_id` so a
/// caller can never lock (or learn the existence of) another merchant's row
/// — the same query answers "not found" and "not yours" identically, which
/// is what every `404 PAYMENT_NOT_FOUND` in the features requires.
pub(crate) async fn lock_payment(
    tx: &mut Transaction<'_, Postgres>,
    payment_id: Uuid,
    merchant_id: MerchantId,
) -> Result<Option<PaymentRow>> {
    let row = sqlx::query(
        "SELECT id, merchant_id, merchant_trade_no, amount, currency, status, item_desc, \
                card_brand, card_last4, notify_url, client_back_url, amount_refunded, \
                created_at, updated_at \
           FROM payments \
          WHERE id = $1 AND merchant_id = $2 \
          FOR UPDATE",
    )
    .bind(payment_id)
    .bind(merchant_id)
    .fetch_optional(&mut **tx)
    .await?;
    row.as_ref().map(map_payment_row).transpose()
}

/// `SELECT ... FOR UPDATE` on one payment by id alone, with no `merchant_id`
/// filter — used by provider-initiated paths (a callback, the reconciler)
/// that do not act on the merchant's behalf and have no merchant id to check
/// against; the merchant boundary there is enforced by the attempt lookup
/// (`UNIQUE (provider_code, provider_trade_no)`) instead. If the caller also
/// needs an attempt lock in the same transaction, this one goes FIRST — see
/// the lock order invariant on [`lock_payment`], above.
pub(crate) async fn lock_payment_by_id(
    tx: &mut Transaction<'_, Postgres>,
    payment_id: Uuid,
) -> Result<Option<PaymentRow>> {
    let row = sqlx::query(
        "SELECT id, merchant_id, merchant_trade_no, amount, currency, status, item_desc, \
                card_brand, card_last4, notify_url, client_back_url, amount_refunded, \
                created_at, updated_at \
           FROM payments \
          WHERE id = $1 \
          FOR UPDATE",
    )
    .bind(payment_id)
    .fetch_optional(&mut **tx)
    .await?;
    row.as_ref().map(map_payment_row).transpose()
}

/// `SELECT ... FOR UPDATE` on one attempt, by its provider identity — the
/// callback's own key (`spec.md`, "Concurrency": "Two forms never share a
/// provider number"). If the caller also needs the payment locked in the
/// same transaction, lock IT first — see the lock order invariant on
/// [`lock_payment`], above.
pub(crate) async fn lock_attempt_by_provider_trade_no(
    tx: &mut Transaction<'_, Postgres>,
    provider_code: &str,
    provider_trade_no: &str,
) -> Result<Option<AttemptRow>> {
    let row = sqlx::query(
        "SELECT id, payment_id, provider_code, provider_trade_no, status, rtn_code, \
                simulate_paid, failure_code, provider_charge_id, card_brand, card_last4, \
                eci, auth_code, started_at, queried_at, settled_at \
           FROM payment_attempts \
          WHERE provider_code = $1 AND provider_trade_no = $2 \
          FOR UPDATE",
    )
    .bind(provider_code)
    .bind(provider_trade_no)
    .fetch_optional(&mut **tx)
    .await?;
    row.as_ref().map(map_attempt_row).transpose()
}

/// `SELECT ... FOR UPDATE` on one attempt, by id. If the caller also needs
/// the payment locked in the same transaction, lock IT first — see the lock
/// order invariant on [`lock_payment`], above.
pub(crate) async fn lock_attempt_by_id(
    tx: &mut Transaction<'_, Postgres>,
    attempt_id: Uuid,
) -> Result<Option<AttemptRow>> {
    let row = sqlx::query(
        "SELECT id, payment_id, provider_code, provider_trade_no, status, rtn_code, \
                simulate_paid, failure_code, provider_charge_id, card_brand, card_last4, \
                eci, auth_code, started_at, queried_at, settled_at \
           FROM payment_attempts \
          WHERE id = $1 \
          FOR UPDATE",
    )
    .bind(attempt_id)
    .fetch_optional(&mut **tx)
    .await?;
    row.as_ref().map(map_attempt_row).transpose()
}

/// The next `payment_events.seq` for `payment_id`. Safe to compute this way
/// — without its own lock — only because every caller already holds the
/// `payments` row `FOR UPDATE` before calling this, which serialises every
/// writer of that payment's events (`spec.md`, "Concurrency").
pub(crate) async fn next_seq(tx: &mut Transaction<'_, Postgres>, payment_id: Uuid) -> Result<i32> {
    let row = sqlx::query(
        "SELECT COALESCE(MAX(seq), 0) + 1 AS next FROM payment_events WHERE payment_id = $1",
    )
    .bind(payment_id)
    .fetch_one(&mut **tx)
    .await?;
    Ok(row.try_get::<i32, _>("next")?)
}

/// Append one row to the audit log. Returns the `event_id` so a caller that
/// needs it (a notification payload naming `refund_id`, for instance) has it
/// without a second query.
pub(crate) async fn insert_event(
    tx: &mut Transaction<'_, Postgres>,
    payment_id: Uuid,
    merchant_id: MerchantId,
    event_type: paygate_domain::EventType,
    payload: Value,
) -> Result<Uuid> {
    let event_id = Uuid::now_v7();
    let seq = next_seq(tx, payment_id).await?;
    sqlx::query(
        "INSERT INTO payment_events (event_id, payment_id, merchant_id, seq, event_type, payload) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(event_id)
    .bind(payment_id)
    .bind(merchant_id)
    .bind(seq)
    .bind(event_type.as_str())
    .bind(payload)
    .execute(&mut **tx)
    .await?;
    Ok(event_id)
}

/// Write the notification a merchant is owed, in the same transaction as the
/// outcome it reports (`spec.md`, "Telling the merchant"). `url` is the
/// payment's own `notify_url`; the notifier resolves it against
/// `MERCHANT_BASE_URL` when it actually delivers.
pub(crate) async fn insert_notification(
    tx: &mut Transaction<'_, Postgres>,
    payment_id: Uuid,
    merchant_id: MerchantId,
    url: &str,
    payload: Value,
) -> Result<i64> {
    let row = sqlx::query(
        "INSERT INTO notifications (payment_id, merchant_id, url, payload) \
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(payment_id)
    .bind(merchant_id)
    .bind(url)
    .bind(payload)
    .fetch_one(&mut **tx)
    .await?;
    Ok(row.try_get("id")?)
}

/// Whether a `sqlx` error is a PostgreSQL unique-violation (SQLSTATE
/// `23505`), the signal every repo function uses to turn a racing `INSERT`
/// into the right domain-level conflict instead of a generic `500`.
pub(crate) fn is_unique_violation(err: &sqlx::Error) -> bool {
    match err {
        sqlx::Error::Database(db_err) => db_err.code().as_deref() == Some("23505"),
        _ => false,
    }
}

/// The merchant context a callback settlement, a refund completion, or a
/// reconciled late settlement needs to build (not send — signing the wire
/// form and delivering it are `notifier`'s job once the row exists) the
/// notification an outcome owes.
#[derive(Debug, Clone)]
pub struct MerchantContext {
    pub merchant_id: MerchantId,
    pub provider_merchant_id: String,
    pub merchant_trade_no: String,
    pub notify_url: String,
}

/// The ECPay shape a merchant (or paygate itself, one level up) is told a
/// payment settled: `MerchantID`, `MerchantTradeNo`, `TradeNo`, `RtnCode`,
/// `RtnMsg`, `TradeAmt`, `PaymentDate` (`spec.md`, "Telling the merchant").
/// `trade_no` is paygate's OWN payment id here — `notify.feature` asserts
/// `payload->>'TradeNo' = '{paymentId}'` — because from the merchant's side
/// paygate is the gateway, and a gateway's `TradeNo` is its own trade number.
#[derive(Debug, Clone)]
pub struct MerchantNotifyFacts {
    pub provider_merchant_id: String,
    pub merchant_trade_no: String,
    pub payment_id: Uuid,
    pub rtn_code: i32,
    pub rtn_msg: &'static str,
    pub amount: Amount,
    pub paid_at: DateTime<Utc>,
}

/// Build the (unsigned) notification payload. Signing with the merchant's
/// own `hash_key`/`hash_iv` is `paygate_provider::ecpay::check_mac_value`,
/// applied by the caller once every field below is in the map — this
/// function only knows the field vocabulary, not how to sign it.
pub(crate) fn merchant_notify_fields(
    facts: &MerchantNotifyFacts,
) -> std::collections::BTreeMap<String, String> {
    let mut fields = std::collections::BTreeMap::new();
    fields.insert("MerchantID".to_string(), facts.provider_merchant_id.clone());
    fields.insert(
        "MerchantTradeNo".to_string(),
        facts.merchant_trade_no.clone(),
    );
    fields.insert("TradeNo".to_string(), facts.payment_id.to_string());
    fields.insert("RtnCode".to_string(), facts.rtn_code.to_string());
    fields.insert("RtnMsg".to_string(), facts.rtn_msg.to_string());
    fields.insert("TradeAmt".to_string(), facts.amount.to_string());
    fields.insert(
        "PaymentDate".to_string(),
        facts.paid_at.format("%Y/%m/%d %H:%M:%S").to_string(),
    );
    fields
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_enum_column_reads_the_schemas_snake_case_text() {
        let status: PaymentStatus = parse_enum_column("status", "succeeded").unwrap();
        assert_eq!(status, PaymentStatus::Succeeded);
    }

    #[test]
    fn parse_enum_column_refuses_an_unknown_value() {
        let err = parse_enum_column::<PaymentStatus>("status", "not_a_status").unwrap_err();
        assert!(matches!(err, Error::Database(_)));
    }

    #[test]
    fn merchant_notify_fields_carries_paygates_own_id_as_trade_no() {
        let facts = MerchantNotifyFacts {
            provider_merchant_id: "2000132".to_string(),
            merchant_trade_no: "ACME-N-1".to_string(),
            payment_id: Uuid::nil(),
            rtn_code: 1,
            rtn_msg: "paid",
            amount: 1250,
            paid_at: DateTime::UNIX_EPOCH,
        };
        let fields = merchant_notify_fields(&facts);
        assert_eq!(fields["TradeNo"], Uuid::nil().to_string());
        assert_eq!(fields["MerchantTradeNo"], "ACME-N-1");
        assert_eq!(fields["RtnMsg"], "paid");
        assert_eq!(fields["TradeAmt"], "1250");
    }
}
