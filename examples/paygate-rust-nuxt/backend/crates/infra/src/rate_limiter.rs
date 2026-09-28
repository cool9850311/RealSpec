//! The token-bucket arithmetic behind the per-merchant rate limit
//! (`spec.md`, "Authentication" table's neighbour, `rate_limits.feature`):
//! `rate_limit_per_minute` tokens, refilling continuously, spent atomically
//! per request.
//!
//! Pure on purpose. [`crate::redis_store::RedisStore::check_rate_limit`]
//! runs the equivalent of this arithmetic as one Lua script inside Redis (so
//! a burst of simultaneous requests against one bucket is still exact, which
//! `rate_limits.feature`'s "six simultaneous requests... let exactly three
//! through" needs and a check-then-set from Rust cannot promise), but the
//! DECISION itself — given a bucket's state, how much refilled, is there a
//! token — is this module, and it is what the unit tests exercise directly.

/// One bucket's state: how many tokens it held, and when that count was
/// current. `None` (no stored state at all) means a bucket nothing has ever
/// touched, OR one Redis forgot across a restart — both read the same way,
/// which is exactly `spec.md`'s "Redis — nothing that matters": "the rate
/// limiter's buckets come back full... a restart is a burst".
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BucketState {
    pub tokens: f64,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Decision {
    pub allowed: bool,
    /// The state to store back, whether or not the request was admitted —
    /// refilled-but-not-consumed on a refusal, so "being refused does not
    /// cost a token" (`rate_limits.feature`) and the next caller sees the
    /// same wait.
    pub next_state: BucketState,
    /// Whole seconds until the bucket holds one token, meaningful only when
    /// `!allowed`.
    pub retry_after_secs: u64,
}

/// Decide (and spend, if admitted) one request against a bucket of
/// `capacity` tokens refilling at `refill_per_minute` tokens/minute.
/// `capacity` is `rate_limit_per_minute` itself — the burst a merchant may
/// spend at once is exactly its steady-state rate, which is the simplest
/// token bucket ECPay-scale traffic needs and matches every figure in
/// `rate_limits.feature` (a bucket of 3 lets exactly 3 through, not more).
pub fn decide(
    state: Option<BucketState>,
    now_ms: i64,
    capacity: u32,
    refill_per_minute: u32,
) -> Decision {
    let capacity_f = capacity as f64;
    let refill_per_ms = refill_per_minute as f64 / 60_000.0;

    let tokens = match state {
        Some(s) => {
            let elapsed_ms = (now_ms - s.updated_at_ms).max(0) as f64;
            (s.tokens + elapsed_ms * refill_per_ms).min(capacity_f)
        }
        None => capacity_f,
    };

    if tokens >= 1.0 {
        Decision {
            allowed: true,
            next_state: BucketState {
                tokens: tokens - 1.0,
                updated_at_ms: now_ms,
            },
            retry_after_secs: 0,
        }
    } else {
        let missing = 1.0 - tokens;
        let retry_after_secs = if refill_per_ms > 0.0 {
            (missing / refill_per_ms / 1000.0).ceil() as u64
        } else {
            u64::MAX
        };
        Decision {
            allowed: false,
            next_state: BucketState {
                tokens,
                updated_at_ms: now_ms,
            },
            retry_after_secs: retry_after_secs.max(1),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_bucket_comes_back_full() {
        let d = decide(None, 0, 3, 3);
        assert!(d.allowed);
        assert_eq!(d.next_state.tokens, 2.0);
    }

    #[test]
    fn three_requests_from_a_bucket_of_three_are_admitted_and_the_fourth_is_not() {
        let mut state = None;
        for _ in 0..3 {
            let d = decide(state, 0, 3, 3);
            assert!(d.allowed);
            state = Some(d.next_state);
        }
        let d = decide(state, 0, 3, 3);
        assert!(!d.allowed);
    }

    #[test]
    fn the_fourth_requests_retry_after_matches_the_per_minute_refill() {
        // rate_limit_per_minute = 3 refills one token every 20 seconds.
        let mut state = None;
        for _ in 0..3 {
            let d = decide(state, 0, 3, 3);
            state = Some(d.next_state);
        }
        let d = decide(state, 0, 3, 3);
        assert_eq!(d.retry_after_secs, 20);
    }

    #[test]
    fn a_bucket_of_zero_refuses_the_very_first_request() {
        let d = decide(None, 0, 0, 0);
        assert!(!d.allowed);
    }

    #[test]
    fn a_refusal_does_not_spend_a_token_so_the_next_caller_waits_the_same_amount() {
        let mut state = None;
        for _ in 0..3 {
            let d = decide(state, 0, 3, 3);
            state = Some(d.next_state);
        }
        let first_refusal = decide(state, 1000, 3, 3);
        assert!(!first_refusal.allowed);
        let second_refusal = decide(Some(first_refusal.next_state), 2000, 3, 3);
        // Neither refusal consumed a token: the second refusal still reports
        // the same retry_after minus the elapsed second, not a longer wait
        // caused by the first refusal itself "spending" anything.
        assert!(!second_refusal.allowed);
        assert!(second_refusal.retry_after_secs <= first_refusal.retry_after_secs);
    }

    #[test]
    fn tokens_refill_continuously_up_to_capacity_but_never_beyond() {
        let d = decide(
            Some(BucketState {
                tokens: 2.9,
                updated_at_ms: 0,
            }),
            1_000_000,
            3,
            3,
        );
        // A huge elapsed time refills to capacity, then spends one.
        assert!(d.allowed);
        assert_eq!(d.next_state.tokens, 2.0);
    }

    #[test]
    fn a_restart_that_forgets_the_bucket_is_a_burst_not_a_continued_refusal() {
        // Redis remembers nothing across a restart (spec.md, "Redis —
        // nothing that matters"); the caller represents that as `state:
        // None` again, which must be treated exactly like a fresh bucket.
        let exhausted = decide(None, 0, 1, 1);
        assert!(exhausted.allowed);
        let refused = decide(Some(exhausted.next_state), 0, 1, 1);
        assert!(!refused.allowed);
        // "Restart" = state forgotten = None again.
        let after_restart = decide(None, 0, 1, 1);
        assert!(after_restart.allowed);
    }
}
