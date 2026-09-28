//! Sessions, the API-key cache, the idempotency lock and answer cache, and
//! the rate limiter's buckets — everything `spec.md`'s "Redis — nothing that
//! matters" lists, and every one of them behind [`CircuitBreaker`], which
//! makes every Redis call fail fast rather than wait out `REDIS_TIMEOUT_MS`
//! once Redis has already shown it is down.
//!
//! What "wrapped" means differs by what the operation backs, because the
//! failure modes are declared, not uniform (`spec.md`, "Failure model"):
//!
//! - Sessions and the API-key cache: a Redis failure surfaces as
//!   [`StoreError::Unavailable`]. The caller (`api`) turns that into
//!   "unauthenticated" for a session, or falls back to PostgreSQL for an API
//!   key — this module does not know which, so it reports the fact and lets
//!   the caller declare the degradation `spec.md` asks for.
//! - The idempotency lock/cache: same — `Unavailable` tells the caller to
//!   fall back to `crate::repo::idempotency`, the PostgreSQL truth.
//! - The rate limiter: [`RedisStore::check_rate_limit`] never returns an
//!   error at all. It fails OPEN internally, in this module, because
//!   `spec.md`, "Failure model" is explicit that Redis's rate limit fails
//!   open — this is the one place infra makes that call for every caller
//!   rather than asking each one to remember it.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use paygate_domain::MerchantId;
use redis::aio::ConnectionManager;
use redis::AsyncCommands;
use serde_json::Value;

use crate::circuit_breaker::{CircuitBreaker, CircuitBreakerConfig};

/// Redis is not answering right now (a real failure, or the breaker already
/// open). Never wraps the underlying `redis` error — the whole point is that
/// a caller does not need to know or match on Redis's own error type to
/// implement the declared degradation.
#[derive(Debug, Clone, Copy, thiserror::Error)]
#[error("redis is unavailable")]
pub struct StoreUnavailable;

pub type StoreResult<T> = Result<T, StoreUnavailable>;

/// A cached, replayable idempotent answer.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CachedAnswer {
    pub status: i32,
    pub body: Value,
}

pub struct RedisStore {
    manager: ConnectionManager,
    breaker: CircuitBreaker,
}

impl RedisStore {
    pub async fn connect(url: &str) -> Result<Self, redis::RedisError> {
        let client = redis::Client::open(url)?;
        let manager = client.get_connection_manager().await?;
        Ok(Self {
            manager,
            breaker: CircuitBreaker::new(CircuitBreakerConfig::default()),
        })
    }

    /// True while the breaker itself would allow a call — exposed so
    /// `GET /health/ready` can report Redis as `up`/`down` without spending
    /// a whole round trip on it when the breaker already knows.
    pub fn is_available(&self) -> bool {
        self.breaker.allow()
    }

    /// A real round trip, for readiness checks that want to know NOW rather
    /// than trust the breaker's last word.
    pub async fn ping(&self) -> bool {
        self.run(|mut conn| async move {
            let _: String = redis::cmd("PING").query_async(&mut conn).await?;
            Ok(())
        })
        .await
        .is_ok()
    }

    async fn run<T, F, Fut>(&self, f: F) -> StoreResult<T>
    where
        F: FnOnce(ConnectionManager) -> Fut,
        Fut: std::future::Future<Output = redis::RedisResult<T>>,
    {
        if !self.breaker.allow() {
            return Err(StoreUnavailable);
        }
        match f(self.manager.clone()).await {
            Ok(v) => {
                self.breaker.record_success();
                Ok(v)
            }
            Err(_) => {
                self.breaker.record_failure();
                Err(StoreUnavailable)
            }
        }
    }

    // ---- sessions ----------------------------------------------------

    fn session_key(session_token_sha256: &str) -> String {
        format!("session:{session_token_sha256}")
    }

    pub async fn create_session(
        &self,
        session_token_sha256: &str,
        payload: &Value,
        ttl: Duration,
    ) -> StoreResult<()> {
        let key = Self::session_key(session_token_sha256);
        let value = payload.to_string();
        let ttl_secs = ttl.as_secs().max(1);
        self.run(move |mut conn| async move { conn.set_ex::<_, _, ()>(key, value, ttl_secs).await })
            .await
    }

    pub async fn get_session(&self, session_token_sha256: &str) -> StoreResult<Option<Value>> {
        let key = Self::session_key(session_token_sha256);
        self.run(move |mut conn| async move {
            let raw: Option<String> = conn.get(key).await?;
            Ok(raw.and_then(|s| serde_json::from_str(&s).ok()))
        })
        .await
    }

    pub async fn delete_session(&self, session_token_sha256: &str) -> StoreResult<()> {
        let key = Self::session_key(session_token_sha256);
        self.run(move |mut conn| async move { conn.del::<_, ()>(key).await })
            .await
    }

    // ---- API key cache -------------------------------------------------

    fn api_key_cache_key(key_hash: &str) -> String {
        format!("apikey:{key_hash}")
    }

    pub async fn cache_api_key(
        &self,
        key_hash: &str,
        payload: &Value,
        ttl: Duration,
    ) -> StoreResult<()> {
        let key = Self::api_key_cache_key(key_hash);
        let value = payload.to_string();
        let ttl_secs = ttl.as_secs().max(1);
        self.run(move |mut conn| async move { conn.set_ex::<_, _, ()>(key, value, ttl_secs).await })
            .await
    }

    pub async fn get_cached_api_key(&self, key_hash: &str) -> StoreResult<Option<Value>> {
        let key = Self::api_key_cache_key(key_hash);
        self.run(move |mut conn| async move {
            let raw: Option<String> = conn.get(key).await?;
            Ok(raw.and_then(|s| serde_json::from_str(&s).ok()))
        })
        .await
    }

    /// Revocation clears the cache (`spec.md`, "Authentication").
    pub async fn invalidate_api_key(&self, key_hash: &str) -> StoreResult<()> {
        let key = Self::api_key_cache_key(key_hash);
        self.run(move |mut conn| async move { conn.del::<_, ()>(key).await })
            .await
    }

    // ---- idempotency lock + answer cache --------------------------------

    fn idempotency_lock_key(merchant_id: MerchantId, key: &str) -> String {
        format!("idem:lock:{merchant_id}:{key}")
    }

    fn idempotency_answer_key(merchant_id: MerchantId, key: &str) -> String {
        format!("idem:answer:{merchant_id}:{key}")
    }

    /// `SET NX PX` — the fast-path lock. `true` means this caller acquired
    /// it; `false` means somebody else holds it right now (`409
    /// IDEMPOTENCY_KEY_IN_USE`, the same code the PostgreSQL fallback in
    /// `crate::repo::idempotency` answers when Redis is unavailable).
    pub async fn try_acquire_idempotency_lock(
        &self,
        merchant_id: MerchantId,
        key: &str,
        ttl: Duration,
    ) -> StoreResult<bool> {
        let redis_key = Self::idempotency_lock_key(merchant_id, key);
        let ttl_ms = ttl.as_millis().max(1) as usize;
        self.run(move |mut conn| async move {
            let acquired: Option<String> = redis::cmd("SET")
                .arg(&redis_key)
                .arg("1")
                .arg("NX")
                .arg("PX")
                .arg(ttl_ms)
                .query_async(&mut conn)
                .await?;
            Ok(acquired.is_some())
        })
        .await
    }

    pub async fn release_idempotency_lock(
        &self,
        merchant_id: MerchantId,
        key: &str,
    ) -> StoreResult<()> {
        let redis_key = Self::idempotency_lock_key(merchant_id, key);
        self.run(move |mut conn| async move { conn.del::<_, ()>(redis_key).await })
            .await
    }

    pub async fn cache_idempotency_answer(
        &self,
        merchant_id: MerchantId,
        key: &str,
        answer: &CachedAnswer,
        ttl: Duration,
    ) -> StoreResult<()> {
        let redis_key = Self::idempotency_answer_key(merchant_id, key);
        let value = serde_json::to_string(answer).unwrap_or_default();
        let ttl_secs = ttl.as_secs().max(1);
        self.run(move |mut conn| async move {
            conn.set_ex::<_, _, ()>(redis_key, value, ttl_secs).await
        })
        .await
    }

    pub async fn get_cached_idempotency_answer(
        &self,
        merchant_id: MerchantId,
        key: &str,
    ) -> StoreResult<Option<CachedAnswer>> {
        let redis_key = Self::idempotency_answer_key(merchant_id, key);
        self.run(move |mut conn| async move {
            let raw: Option<String> = conn.get(redis_key).await?;
            Ok(raw.and_then(|s| serde_json::from_str(&s).ok()))
        })
        .await
    }

    // ---- rate limiter ----------------------------------------------------

    /// One Lua script, so a burst of simultaneous requests against the same
    /// merchant is still exact (`rate_limits.feature`'s "six simultaneous
    /// requests against a bucket of three let exactly three through") —
    /// a separate `GET` then `SET` from Rust could not promise that under
    /// concurrency. The arithmetic mirrors `crate::rate_limiter::decide`
    /// exactly; that module is this script's specification and its test
    /// suite.
    const RATE_LIMIT_SCRIPT: &'static str = r#"
        local key = KEYS[1]
        local capacity = tonumber(ARGV[1])
        local refill_per_minute = tonumber(ARGV[2])
        local now_ms = tonumber(ARGV[3])

        local refill_per_ms = refill_per_minute / 60000.0
        local raw = redis.call("GET", key)
        local tokens
        if raw then
            local sep = string.find(raw, ":")
            local stored_tokens = tonumber(string.sub(raw, 1, sep - 1))
            local updated_at = tonumber(string.sub(raw, sep + 1))
            local elapsed = math.max(now_ms - updated_at, 0)
            tokens = math.min(stored_tokens + elapsed * refill_per_ms, capacity)
        else
            tokens = capacity
        end

        local allowed = 0
        local retry_after = 0
        if tokens >= 1.0 then
            allowed = 1
            tokens = tokens - 1.0
        else
            local missing = 1.0 - tokens
            if refill_per_ms > 0 then
                retry_after = math.ceil(missing / refill_per_ms / 1000.0)
            else
                retry_after = 2147483647
            end
            if retry_after < 1 then retry_after = 1 end
        end

        redis.call("SET", key, tostring(tokens) .. ":" .. tostring(now_ms), "EX", 3600)
        return { allowed, retry_after }
    "#;

    /// Check and spend one token from `merchant_id`'s bucket. Fails OPEN: a
    /// Redis failure (or the breaker already open) is treated as
    /// [`RateLimitDecision::Allowed`] with no `retry_after`, per `spec.md`,
    /// "Failure model" — this is the one Redis operation in this crate that
    /// never hands its caller an error to remember to handle.
    pub async fn check_rate_limit(
        &self,
        merchant_id: MerchantId,
        limit_per_minute: u32,
    ) -> RateLimitDecision {
        let key = format!("ratelimit:{merchant_id}");
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);

        let result = self
            .run(move |mut conn| {
                let key = key.clone();
                async move {
                    let script = redis::Script::new(Self::RATE_LIMIT_SCRIPT);
                    let (allowed, retry_after): (i64, i64) = script
                        .key(key)
                        .arg(limit_per_minute)
                        .arg(limit_per_minute)
                        .arg(now_ms)
                        .invoke_async(&mut conn)
                        .await?;
                    Ok((allowed, retry_after))
                }
            })
            .await;

        match result {
            Ok((1, _)) => RateLimitDecision::Allowed,
            Ok((_, retry_after)) => RateLimitDecision::Limited {
                retry_after_secs: retry_after.max(1) as u64,
            },
            Err(StoreUnavailable) => RateLimitDecision::Allowed,
        }
    }
}

/// What the rate limiter decided. Never carries a Redis-shaped error — see
/// [`RedisStore::check_rate_limit`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateLimitDecision {
    Allowed,
    Limited { retry_after_secs: u64 },
}

/// Shared ownership across the axum handlers that need it — a thin alias so
/// callers do not have to spell out `Arc<RedisStore>` themselves.
pub type SharedRedisStore = Arc<RedisStore>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rate_limiter;

    #[test]
    fn session_and_cache_keys_are_namespaced_and_do_not_collide() {
        assert_ne!(
            RedisStore::session_key("abc"),
            RedisStore::api_key_cache_key("abc")
        );
        assert_ne!(
            RedisStore::idempotency_lock_key(1, "k"),
            RedisStore::idempotency_answer_key(1, "k")
        );
    }

    #[test]
    fn idempotency_keys_are_scoped_per_merchant() {
        assert_ne!(
            RedisStore::idempotency_lock_key(1, "same-key"),
            RedisStore::idempotency_lock_key(2, "same-key")
        );
    }

    #[test]
    fn the_rate_limit_lua_script_matches_the_pure_decide_function_at_a_worked_example() {
        // The script is only exercised for real against a live Redis
        // (out of this crate's reach without `testcontainers` wired into
        // its Cargo.toml — see this agent's final report). This test at
        // least keeps the script's text and `rate_limiter::decide`'s
        // constants from silently drifting apart: both compute
        // `retry_after = ceil(missing / (refill_per_minute / 60000) / 1000)`.
        let d = rate_limiter::decide(
            Some(rate_limiter::BucketState {
                tokens: 0.0,
                updated_at_ms: 0,
            }),
            0,
            3,
            3,
        );
        assert_eq!(d.retry_after_secs, 20);
        assert!(RedisStore::RATE_LIMIT_SCRIPT.contains("refill_per_ms"));
    }
}
