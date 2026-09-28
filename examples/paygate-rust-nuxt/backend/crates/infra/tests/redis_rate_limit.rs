//! `RedisStore::check_rate_limit` against a real Redis, started by
//! `testcontainers`: the atomic Lua script under concurrency, and that a
//! restart leaves the bucket full (`resilience.feature`'s deliberate burst).

mod common;

use paygate_infra::redis_store::RateLimitDecision;

#[tokio::test]
async fn three_tokens_are_admitted_and_the_fourth_is_limited_with_the_documented_retry_after() {
    let fx = common::start_redis().await;
    let store = &fx.store;

    for _ in 0..3 {
        assert_eq!(
            store.check_rate_limit(1, 3).await,
            RateLimitDecision::Allowed
        );
    }
    match store.check_rate_limit(1, 3).await {
        RateLimitDecision::Limited { retry_after_secs } => {
            assert_eq!(retry_after_secs, 20, "3/minute refills one token every 20s");
        }
        other => panic!("expected Limited, got {other:?}"),
    }
}

#[tokio::test]
async fn six_simultaneous_requests_against_a_bucket_of_three_let_exactly_three_through() {
    // The property the Lua script exists for: a bare GET-then-SET from Rust
    // could not promise this under real concurrency.
    let fx = common::start_redis().await;
    let store = std::sync::Arc::new(fx.store);

    let mut handles = Vec::new();
    for _ in 0..6 {
        let store = store.clone();
        handles.push(tokio::spawn(
            async move { store.check_rate_limit(42, 3).await },
        ));
    }

    let mut allowed = 0;
    let mut limited = 0;
    for h in handles {
        match h.await.unwrap() {
            RateLimitDecision::Allowed => allowed += 1,
            RateLimitDecision::Limited { .. } => limited += 1,
        }
    }
    assert_eq!(allowed, 3);
    assert_eq!(limited, 3);
}

#[tokio::test]
async fn different_merchants_have_independent_buckets() {
    let fx = common::start_redis().await;
    let store = &fx.store;

    assert_eq!(
        store.check_rate_limit(1, 1).await,
        RateLimitDecision::Allowed
    );
    assert!(matches!(
        store.check_rate_limit(1, 1).await,
        RateLimitDecision::Limited { .. }
    ));
    // A different merchant's bucket is untouched by merchant 1 exhausting its own.
    assert_eq!(
        store.check_rate_limit(2, 1).await,
        RateLimitDecision::Allowed
    );
}

#[tokio::test]
async fn a_restart_forgets_the_bucket_and_the_next_request_sees_it_full() {
    let fx = common::start_redis().await;

    assert_eq!(
        fx.store.check_rate_limit(9, 1).await,
        RateLimitDecision::Allowed
    );
    assert!(matches!(
        fx.store.check_rate_limit(9, 1).await,
        RateLimitDecision::Limited { .. }
    ));

    // `service "redis" is stopped` / `is started`, the exact operations
    // `spec.md`, "Isolation" describes: the library's own lifecycle
    // methods, and the host port re-read afterwards because Docker
    // reassigns it.
    fx.container.stop().await.expect("stop redis");
    fx.container.start().await.expect("start redis");
    let store_after_restart = common::connect_redis(&fx.container).await;

    // Nothing survived the restart (spec.md, "Redis — nothing that
    // matters"): the very next request for the same merchant is admitted
    // again, exactly as if the bucket had never been touched — a burst, by
    // design.
    assert_eq!(
        store_after_restart.check_rate_limit(9, 1).await,
        RateLimitDecision::Allowed,
        "a restart must present a full bucket, not remember the earlier refusal"
    );
}
