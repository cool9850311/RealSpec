//! The two credentials this API accepts (`spec.md`, "Authentication"), each
//! its own extractor so a handler simply names the one it needs and gets a
//! `401` for free otherwise: [`MerchantAuth`] (a merchant's own `Authorization:
//! Bearer sk_test_...`, plus the per-merchant rate limit, since every
//! endpoint that accepts this credential is also rate-limited —
//! `openapi.yaml`'s "Rate limits" section / `rate_limits.feature`) and
//! [`DashboardSession`] (the `session` cookie a dashboard login mints).

use axum::extract::{FromRequestParts, State};
use axum::http::request::Parts;
use sha2::{Digest, Sha256};

use paygate_domain::MerchantId;

use crate::db;
use crate::error::ApiError;
use crate::state::AppState;

pub fn sha256_hex(s: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(s.as_bytes());
    hex::encode(hasher.finalize())
}

/// A merchant's server, authenticated and already charged one token of its
/// rate-limit bucket. Every endpoint under `Orders` in `openapi.yaml` uses
/// this extractor and nothing else does, which is what keeps checkouts and
/// callbacks outside the limit (`rate_limits.feature`'s own description).
#[derive(Debug)]
pub struct MerchantAuth {
    pub merchant_id: MerchantId,
}

impl FromRequestParts<AppState> for MerchantAuth {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let State(state) = State::<AppState>::from_request_parts(parts, state)
            .await
            .map_err(|_| ApiError::Internal)?;

        let header = parts
            .headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .ok_or(ApiError::Unauthenticated)?;
        // The literal scheme this gateway issues, byte for byte — not RFC
        // 9110's case-insensitive scheme name, and not with a stray extra
        // space in front of the key (`api_keys.feature`'s own examples).
        let token = header
            .strip_prefix("Bearer ")
            .ok_or(ApiError::Unauthenticated)?;
        if token.is_empty() {
            return Err(ApiError::Unauthenticated);
        }
        let key_hash = sha256_hex(token);

        let merchant_id = resolve_api_key(&state, &key_hash)
            .await
            .ok_or(ApiError::Unauthenticated)?;

        let merchant = db::fetch_merchant(&state.db, merchant_id)
            .await
            .map_err(ApiError::from)?
            .ok_or(ApiError::Unauthenticated)?;

        // Bounded the same way as every other direct Redis call this crate
        // makes (`crate::state::REDIS_OP_TIMEOUT`): the rate limiter already
        // fails OPEN on an outright Redis error
        // (`RedisStore::check_rate_limit`'s own doc comment), but that
        // promise is worth nothing if the call never returns at all
        // (`resilience.feature`, "Without Redis...").
        let decision = tokio::time::timeout(
            crate::state::REDIS_OP_TIMEOUT,
            state
                .redis
                .check_rate_limit(merchant_id, merchant.rate_limit_per_minute),
        )
        .await
        .unwrap_or(paygate_infra::redis_store::RateLimitDecision::Allowed);
        match decision {
            paygate_infra::redis_store::RateLimitDecision::Allowed => {}
            paygate_infra::redis_store::RateLimitDecision::Limited { retry_after_secs } => {
                return Err(ApiError::RateLimited { retry_after_secs });
            }
        }

        Ok(MerchantAuth { merchant_id })
    }
}

/// Redis cache -> PostgreSQL fallback, per `spec.md`'s "Authentication" row
/// for the API key: "SHA-256 -> Redis cache -> PostgreSQL" — the same
/// degradation pattern `idempotency.rs` uses for its own cache, applied
/// here to the key lookup.
async fn resolve_api_key(state: &AppState, key_hash: &str) -> Option<MerchantId> {
    match crate::state::redis_bounded(state.redis.get_cached_api_key(key_hash)).await {
        Ok(Some(cached)) => {
            if let Some(id) = cached.get("merchant_id").and_then(|v| v.as_i64()) {
                return Some(id);
            }
        }
        Ok(None) => {}
        Err(_) => {
            // Redis unavailable: PostgreSQL alone answers.
            return match db::find_active_api_key(&state.db, key_hash).await {
                Ok(Some(row)) => Some(row.merchant_id),
                _ => None,
            };
        }
    }

    let row = db::find_active_api_key(&state.db, key_hash).await.ok()??;
    let payload = serde_json::json!({ "merchant_id": row.merchant_id });
    let _ = crate::state::redis_bounded(state.redis.cache_api_key(
        key_hash,
        &payload,
        state.config.api_key_cache_ttl,
    ))
    .await;
    Some(row.merchant_id)
}

/// A signed-in dashboard user. `merchant_id` and `email` come straight out
/// of the session payload Redis holds; nothing here trusts anything the
/// client sent except the opaque token itself.
#[derive(Debug)]
pub struct DashboardSession {
    pub merchant_id: MerchantId,
    pub email: String,
}

fn read_cookie<'a>(parts: &'a Parts, name: &str) -> Option<&'a str> {
    let header = parts
        .headers
        .get(axum::http::header::COOKIE)?
        .to_str()
        .ok()?;
    for part in header.split(';') {
        let part = part.trim();
        if let Some(value) = part.strip_prefix(name).and_then(|v| v.strip_prefix('=')) {
            return Some(value);
        }
    }
    None
}

impl FromRequestParts<AppState> for DashboardSession {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let State(state) = State::<AppState>::from_request_parts(parts, state)
            .await
            .map_err(|_| ApiError::Internal)?;

        let token = read_cookie(parts, "session").ok_or(ApiError::Unauthenticated)?;
        if token.is_empty() {
            return Err(ApiError::Unauthenticated);
        }
        let token_hash = sha256_hex(token);

        // A Redis failure here degrades to "unauthenticated", never a `503`
        // — that answer is reserved for the one action (signing in) that has
        // nowhere else to write a session at all (`resilience.feature`,
        // "Without Redis the dashboard refuses to sign in, and signs in
        // again once it is back").
        let payload = crate::state::redis_bounded(state.redis.get_session(&token_hash))
            .await
            .ok()
            .flatten()
            .ok_or(ApiError::Unauthenticated)?;

        let merchant_id = payload
            .get("merchant_id")
            .and_then(|v| v.as_i64())
            .ok_or(ApiError::Unauthenticated)?;
        let email = payload
            .get("email")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .ok_or(ApiError::Unauthenticated)?;

        Ok(DashboardSession { merchant_id, email })
    }
}
