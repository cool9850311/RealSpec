//! Orchestrates one idempotent request around the two primitives
//! `paygate-infra` hands this crate: [`paygate_infra::redis_store::RedisStore`]'s
//! lock (a fast, best-effort mutex) and [`paygate_infra::repo::idempotency`]
//! (the PostgreSQL source of truth — `spec.md`, "Redis — nothing that
//! matters": "everything else Redis holds has an answer in PostgreSQL,
//! including an idempotency key whose first answer was cached in Redis
//! and whose retry arrives after Redis is gone"). Postgres alone already
//! gives every guarantee `idempotency.feature` and `resilience.feature`
//! ask for (its `begin` is a single `INSERT ... ON CONFLICT` under one
//! row's lock); Redis only saves a request that is plainly losing a race
//! the round trip a lock miss would otherwise cost.
//!
//! Calling convention, mirroring `repo::idempotency`'s own doc comment:
//! call [`begin`], and on [`Outcome::Started`] do the work, then call EITHER
//! [`complete`] (only for an answer that should be replayed verbatim on a
//! retry) OR [`release`] (for every other answer — a validation failure, a
//! `404`, a `502` — so the key is free the instant a corrected retry arrives,
//! per `openapi.yaml`: "Answers produced before processing starts... are not
//! stored, so a corrected request may reuse its key").

use std::time::Duration;

use paygate_domain::MerchantId;
use paygate_infra::repo::BeginOutcome;
use serde_json::Value;

use crate::state::AppState;

#[derive(Debug, Clone)]
pub enum Outcome {
    Started,
    Replay { status: i32, body: Value },
    InUse,
    Reused,
}

pub async fn begin(
    state: &AppState,
    merchant_id: MerchantId,
    key: &str,
    fingerprint: &str,
) -> Result<Outcome, paygate_infra::Error> {
    let ttl = state.config.idempotency_ttl;
    let lock_ttl = state.config.idempotency_lock_ttl;

    // Bounded (`crate::state::REDIS_OP_TIMEOUT`) rather than a bare
    // `.await`: `resilience.feature`'s "Without Redis, four refunds racing
    // with one key still produce one refund" needs the `Err` branch below
    // (PostgreSQL alone) to actually run within the request, not after
    // however long a dead connection takes to give up on its own.
    match crate::state::redis_bounded(state.redis.try_acquire_idempotency_lock(
        merchant_id,
        key,
        lock_ttl,
    ))
    .await
    {
        Ok(true) => {
            // We hold the fast lock; Postgres still makes the real decision,
            // and we give the lock back immediately unless the decision is
            // "start the work" (in which case `complete`/`release` gives it
            // back once the work is done).
            let outcome = pg_begin(state, merchant_id, key, fingerprint, ttl).await?;
            if !matches!(outcome, Outcome::Started) {
                release_lock(state, merchant_id, key).await;
            }
            Ok(outcome)
        }
        Ok(false) => {
            // Somebody else is holding the lock for this exact key right
            // now — a concurrent duplicate, the case every race scenario in
            // idempotency.feature and scaling.feature exercises.
            Ok(Outcome::InUse)
        }
        Err(_) => {
            // Redis is unavailable: fall back to PostgreSQL alone
            // (resilience.feature's races run this exact path).
            pg_begin(state, merchant_id, key, fingerprint, ttl).await
        }
    }
}

async fn pg_begin(
    state: &AppState,
    merchant_id: MerchantId,
    key: &str,
    fingerprint: &str,
    ttl: Duration,
) -> Result<Outcome, paygate_infra::Error> {
    let outcome = paygate_infra::repo::begin(&state.db, merchant_id, key, fingerprint, ttl).await?;
    Ok(match outcome {
        BeginOutcome::Started => Outcome::Started,
        BeginOutcome::Replay { status, body } => Outcome::Replay { status, body },
        BeginOutcome::InUse => Outcome::InUse,
        BeginOutcome::Reused => Outcome::Reused,
    })
}

async fn release_lock(state: &AppState, merchant_id: MerchantId, key: &str) {
    let _ =
        crate::state::redis_bounded(state.redis.release_idempotency_lock(merchant_id, key)).await;
}

/// Store the final, replayable answer — only ever call this for a response
/// that a retry of the same key should get back verbatim.
pub async fn complete(
    state: &AppState,
    merchant_id: MerchantId,
    key: &str,
    status: u16,
    body: &Value,
) {
    if let Err(err) =
        paygate_infra::repo::complete(&state.db, merchant_id, key, status as i32, body).await
    {
        tracing::error!(error = %err, "failed to persist the idempotent answer");
    }
    release_lock(state, merchant_id, key).await;
}

/// Free a key that did not reach a cacheable outcome.
pub async fn release(state: &AppState, merchant_id: MerchantId, key: &str) {
    if let Err(err) = paygate_infra::repo::release(&state.db, merchant_id, key).await {
        tracing::error!(error = %err, "failed to release an idempotency key");
    }
    release_lock(state, merchant_id, key).await;
}
