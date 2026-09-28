//! PostgreSQL's own idempotency truth: `idempotency_keys`, primary keyed on
//! `(merchant_id, idempotency_key)`. `spec.md`, "Two kinds of duplicate":
//! "The guarantee is PostgreSQL's ... Redis only makes the common paths
//! cheaper" — [`crate::redis_store`] is the cheap path an `api` handler tries
//! first; this module is what it falls back to when Redis is unavailable,
//! and also where the FINAL answer is always persisted once a request
//! completes, since a Redis-only cache does not survive a restart
//! (`idempotency.feature`, the key seeded straight into this table and
//! `resilience.feature`'s races).
//!
//! **Calling convention, for whichever crate orchestrates a handler around
//! this** (documented here because these are primitives, not a request
//! pipeline): call [`begin`] before doing any work; on [`BeginOutcome::Started`],
//! do the work and then call EITHER [`complete`] (only for a response that
//! should be replayed verbatim on a retry — a validation failure or a
//! `PROVIDER_UNAVAILABLE` must NOT be cached, since
//! `idempotency.feature`'s "does not consume its key" and `refunds.feature`'s
//! "the key was not spent on an answer nobody got" both retry the SAME key
//! with a DIFFERENT body afterwards) OR [`release`] (for every other
//! response, freeing the key immediately rather than leaving it `in_progress`
//! until its lock would otherwise be reclaimed).

use chrono::Utc;
use paygate_domain::MerchantId;
use serde_json::Value;
use sqlx::Row;
use std::time::Duration;

use crate::db::Db;
use crate::error::Result;

/// What starting an idempotent request found.
#[derive(Debug, Clone, PartialEq)]
pub enum BeginOutcome {
    /// A fresh key (or one whose retention already expired): proceed.
    Started,
    /// A request with this exact key is still being processed elsewhere —
    /// `409 IDEMPOTENCY_KEY_IN_USE`.
    InUse,
    /// A request with this key already completed, with a matching
    /// fingerprint: replay this stored answer rather than doing the work
    /// again.
    Replay { status: i32, body: Value },
    /// This key was already used for a REQUEST THAT DIFFERED — `422
    /// IDEMPOTENCY_KEY_REUSED`.
    Reused,
}

/// Begin (or resume) an idempotent request. `fingerprint` is
/// `paygate_domain::idempotency_fingerprint`, or the caller's own extension
/// of it — `idempotency.feature`'s "a key spent on one order's refund cannot
/// be spent on another's" needs the resource path folded in too, which is
/// the caller's business, not this function's.
pub async fn begin(
    db: &Db,
    merchant_id: MerchantId,
    key: &str,
    fingerprint: &str,
    ttl: Duration,
) -> Result<BeginOutcome> {
    let mut tx = db.begin().await?;

    let ttl_secs = ttl.as_secs() as i64;
    let inserted = sqlx::query(
        "INSERT INTO idempotency_keys \
            (merchant_id, idempotency_key, request_fingerprint, state, expires_at) \
         VALUES ($1, $2, $3, 'in_progress', now() + make_interval(secs => $4)) \
         ON CONFLICT (merchant_id, idempotency_key) DO NOTHING",
    )
    .bind(merchant_id)
    .bind(key)
    .bind(fingerprint)
    .bind(ttl_secs as f64)
    .execute(&mut *tx)
    .await?;

    if inserted.rows_affected() == 1 {
        tx.commit().await?;
        return Ok(BeginOutcome::Started);
    }

    // Someone got here first (or a row from a previous, expired use is still
    // sitting there): decide from what is actually there, under its lock.
    let row = sqlx::query(
        "SELECT request_fingerprint, state, response_status, response_body, expires_at \
           FROM idempotency_keys \
          WHERE merchant_id = $1 AND idempotency_key = $2 \
          FOR UPDATE",
    )
    .bind(merchant_id)
    .bind(key)
    .fetch_one(&mut *tx)
    .await?;

    let stored_fingerprint: String = row.try_get("request_fingerprint")?;
    let state: String = row.try_get("state")?;
    let expires_at: chrono::DateTime<Utc> = row.try_get("expires_at")?;

    if expires_at <= Utc::now() {
        // A key whose retention has expired is a new key
        // (`idempotency.feature`).
        sqlx::query(
            "UPDATE idempotency_keys \
                SET request_fingerprint = $1, state = 'in_progress', response_status = NULL, \
                    response_body = NULL, created_at = now(), \
                    expires_at = now() + make_interval(secs => $2) \
              WHERE merchant_id = $3 AND idempotency_key = $4",
        )
        .bind(fingerprint)
        .bind(ttl_secs as f64)
        .bind(merchant_id)
        .bind(key)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        return Ok(BeginOutcome::Started);
    }

    let outcome = match state.as_str() {
        "in_progress" => BeginOutcome::InUse,
        "completed" if stored_fingerprint == fingerprint => {
            let status: i32 = row.try_get("response_status")?;
            let body: Value = row
                .try_get::<Option<Value>, _>("response_body")?
                .unwrap_or(Value::Null);
            BeginOutcome::Replay { status, body }
        }
        "completed" => BeginOutcome::Reused,
        _ => BeginOutcome::InUse,
    };
    tx.commit().await?;
    Ok(outcome)
}

/// Store the final, replayable answer. Only call this for a response that
/// should be handed back verbatim to a retry of this exact key — see the
/// module doc's calling convention.
pub async fn complete(
    db: &Db,
    merchant_id: MerchantId,
    key: &str,
    status: i32,
    body: &Value,
) -> Result<()> {
    sqlx::query(
        "UPDATE idempotency_keys SET state = 'completed', response_status = $1, response_body = $2 \
          WHERE merchant_id = $3 AND idempotency_key = $4",
    )
    .bind(status)
    .bind(body)
    .bind(merchant_id)
    .bind(key)
    .execute(&db.0)
    .await?;
    Ok(())
}

/// Free a key that did not reach a cacheable outcome, so an immediate retry
/// (with the same or a different body) finds nothing in its way. Only
/// removes a row still `in_progress` — never a `completed` one, so this can
/// never undo a real answer another concurrent request already stored.
pub async fn release(db: &Db, merchant_id: MerchantId, key: &str) -> Result<()> {
    sqlx::query(
        "DELETE FROM idempotency_keys \
          WHERE merchant_id = $1 AND idempotency_key = $2 AND state = 'in_progress'",
    )
    .bind(merchant_id)
    .bind(key)
    .execute(&db.0)
    .await?;
    Ok(())
}
