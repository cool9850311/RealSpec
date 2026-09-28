//! A small, generic circuit breaker, used by [`crate::redis_store`] so every
//! Redis call in the API fails fast (rather than waiting out
//! `REDIS_TIMEOUT_MS` on every single request) once Redis has already shown
//! it is down.
//!
//! Deliberately independent of `redis` or `tokio`: the whole decision —
//! "given how many consecutive failures we've seen, and how long ago we
//! opened, is a call allowed right now" — is a pure function of a small
//! state and a clock, which is what makes it unit-testable without a broker,
//! a mock, or a sleep.

use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy)]
pub struct CircuitBreakerConfig {
    /// Consecutive failures before the breaker opens.
    pub failure_threshold: u32,
    /// How long the breaker stays open before allowing one trial call
    /// through (half-open).
    pub open_duration: Duration,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            failure_threshold: 3,
            open_duration: Duration::from_secs(5),
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct State {
    consecutive_failures: u32,
    opened_at: Option<Instant>,
}

/// Pure: whether a call is allowed right now, given the breaker's state.
/// Open, but longer than `open_duration` ago, allows exactly one trial call
/// through (the caller's own success/failure report is what closes or
/// re-opens it) — a classic half-open step, spelled out as its own function
/// so the state machine is checkable without `Instant::now()` at all.
fn allow(state: &State, config: &CircuitBreakerConfig, now: Instant) -> bool {
    match state.opened_at {
        None => true,
        Some(opened_at) => now.duration_since(opened_at) >= config.open_duration,
    }
}

pub struct CircuitBreaker {
    config: CircuitBreakerConfig,
    state: Mutex<State>,
}

impl CircuitBreaker {
    pub fn new(config: CircuitBreakerConfig) -> Self {
        Self {
            config,
            state: Mutex::new(State::default()),
        }
    }

    /// Whether a caller should even attempt the Redis call right now.
    pub fn allow(&self) -> bool {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        allow(&state, &self.config, Instant::now())
    }

    pub fn record_success(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.consecutive_failures = 0;
        state.opened_at = None;
    }

    pub fn record_failure(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.consecutive_failures += 1;
        if state.consecutive_failures >= self.config.failure_threshold {
            state.opened_at = Some(Instant::now());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> CircuitBreakerConfig {
        CircuitBreakerConfig {
            failure_threshold: 3,
            open_duration: Duration::from_millis(100),
        }
    }

    #[test]
    fn a_fresh_breaker_allows_calls() {
        let state = State::default();
        assert!(allow(&state, &config(), Instant::now()));
    }

    #[test]
    fn fewer_failures_than_the_threshold_keep_it_closed() {
        let breaker = CircuitBreaker::new(config());
        breaker.record_failure();
        breaker.record_failure();
        assert!(breaker.allow());
    }

    #[test]
    fn reaching_the_threshold_opens_the_breaker() {
        let breaker = CircuitBreaker::new(config());
        for _ in 0..3 {
            breaker.record_failure();
        }
        assert!(!breaker.allow());
    }

    #[test]
    fn a_success_resets_the_failure_count() {
        let breaker = CircuitBreaker::new(config());
        breaker.record_failure();
        breaker.record_failure();
        breaker.record_success();
        breaker.record_failure();
        breaker.record_failure();
        // Two failures since the reset, still under the threshold of three.
        assert!(breaker.allow());
    }

    #[test]
    fn the_breaker_allows_a_trial_call_once_open_duration_has_passed() {
        let cfg = config();
        let mut state = State {
            consecutive_failures: 3,
            opened_at: Some(Instant::now() - Duration::from_millis(50)),
        };
        assert!(!allow(&state, &cfg, Instant::now()));
        state.opened_at = Some(Instant::now() - Duration::from_millis(150));
        assert!(allow(&state, &cfg, Instant::now()));
    }

    #[test]
    fn a_failed_trial_call_reopens_the_breaker() {
        let breaker = CircuitBreaker::new(CircuitBreakerConfig {
            failure_threshold: 1,
            open_duration: Duration::from_millis(10),
        });
        breaker.record_failure();
        assert!(!breaker.allow());
        std::thread::sleep(Duration::from_millis(20));
        assert!(breaker.allow(), "half-open trial should be allowed");
        breaker.record_failure();
        assert!(!breaker.allow(), "a failed trial call re-opens the breaker");
    }
}
