//! PostgreSQL/Redis-backed tests `spec.md`'s test plan asks of `api`
//! (`spec.md`, "Test plan" -> "Unit tests, by crate" -> `api`) and that were
//! previously only verified by hand against a throwaway stack, now that
//! `testcontainers`/`testcontainers-modules` are dev-dependencies of this
//! crate. Every container's lifecycle belongs to the library (`spec.md`,
//! "Isolation": "containers are Testcontainers' to manage... and nothing
//! else manages them"): no `docker run`, no hand-assigned ports, no
//! sleep-then-poll, and cleanup is left to the library's own teardown
//! rather than disarmed. Each test starts its own stack.

use std::collections::BTreeMap;

use axum::body::Bytes;
use axum::extract::{FromRequestParts, Path, State};
use axum::http::{Request, StatusCode};
use axum::response::IntoResponse;
use serde_json::json;
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, ImageExt};
use testcontainers_modules::{postgres::Postgres as PgImage, redis::Redis as RedisImage};

use paygate_domain::{PaymentStatus, ProviderCode};
use paygate_infra::clickhouse::ClickHouseClient;
use paygate_infra::config::ApiConfig;
use paygate_infra::redis_store::RedisStore;
use paygate_infra::{repo, Db};

use crate::auth::DashboardSession;
use crate::error::ApiError;
use crate::state::AppState;

async fn start_postgres() -> (Db, ContainerAsync<PgImage>) {
    // Pinned to the same tag `backend/apitest/src/stack.rs` pins its own
    // Postgres container to, rather than the module's own default
    // (`11-alpine`), which would otherwise run this suite against a major
    // version nobody chose.
    let container = PgImage::default()
        .with_tag("17-alpine")
        .start()
        .await
        .expect("postgres container should start");
    let host = container.get_host().await.expect("postgres host");
    let port = container
        .get_host_port_ipv4(5432)
        .await
        .expect("postgres port");
    let dsn = format!("postgres://postgres:postgres@{host}:{port}/postgres");
    let db = Db::connect(&dsn, 5)
        .await
        .expect("should connect to the freshly started postgres");
    db.migrate().await.expect("migrations should apply cleanly");
    (db, container)
}

/// `redis-server --save "" --appendonly no`: the default `Redis` module
/// image persists an RDB snapshot on a clean shutdown, and a container
/// stopped-then-started with persistence on would come back holding
/// whatever it held when it was stopped — exactly the state this file's own
/// "Redis restarts empty" style test must NOT be true of.
async fn start_redis() -> (RedisStore, ContainerAsync<RedisImage>) {
    // Pinned to the same tag `backend/apitest/src/stack.rs` pins its own
    // Redis container to, rather than the module's own default (`5.0`).
    let container = RedisImage::default()
        .with_tag("7-alpine")
        .with_cmd(["redis-server", "--save", "", "--appendonly", "no"])
        .start()
        .await
        .expect("redis container should start");
    let host = container.get_host().await.expect("redis host");
    let port = container
        .get_host_port_ipv4(6379)
        .await
        .expect("redis port");
    let store = RedisStore::connect(&format!("redis://{host}:{port}"))
        .await
        .expect("should connect to the freshly started redis");
    (store, container)
}

fn test_state(db: Db, redis: RedisStore, cookie_secure: bool) -> AppState {
    let clickhouse = ClickHouseClient::new("http://clickhouse.invalid", "paygate", "x", "x");
    let config = ApiConfig {
        provider_trade_no_prefix: "PGT".to_string(),
        public_base_url: "http://paygate.test".to_string(),
        psp_timeout: std::time::Duration::from_millis(500),
        session_ttl: std::time::Duration::from_secs(28800),
        api_key_cache_ttl: std::time::Duration::from_secs(60),
        idempotency_ttl: std::time::Duration::from_secs(3600 * 24),
        idempotency_lock_ttl: std::time::Duration::from_millis(30_000),
        cookie_secure,
        frontend_origin: "http://localhost:3000".to_string(),
    };
    let common = paygate_infra::config::CommonConfig {
        port: 0,
        instance_id: "integration-test".to_string(),
        log_level: "info".to_string(),
        shutdown_grace: std::time::Duration::from_secs(1),
    };
    AppState::new(
        db,
        redis,
        clickhouse,
        config,
        common,
        "http://provider.invalid".to_string(),
    )
}

async fn seed_ecpay_merchant(db: &Db) {
    sqlx::query(
        "INSERT INTO providers (code, platform_id, hash_key, hash_iv, cashier_url, query_url, refund_url) \
         VALUES ('ecpay', '3002607', 'pwFHCqoQZGmho4w6', 'EkRm7iFT261dpevs', \
                 '/provider/ecpay/Cashier/AioCheckOut/V5', '/provider/ecpay/Cashier/QueryTradeInfo/V5', \
                 '/provider/ecpay/CreditDetail/DoAction') \
         ON CONFLICT (code) DO NOTHING",
    )
    .execute(&db.0)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO merchants \
            (id, name, currency, timezone, provider_code, provider_merchant_id, \
             rate_limit_per_minute, hash_key, hash_iv) \
         VALUES (1, 'Acme Coffee', 'USD', 'Asia/Taipei', 'ecpay', '2000132', 600, \
                 'acmehashkey0123456789abcdef01234', 'acmehashiv012345') \
         ON CONFLICT (id) DO NOTHING",
    )
    .execute(&db.0)
    .await
    .unwrap();
}

async fn seed_dashboard_user(db: &Db) {
    seed_ecpay_merchant(db).await;
    sqlx::query(
        "INSERT INTO dashboard_users (id, merchant_id, email, password_hash) \
         VALUES (1, 1, 'owner@acme.test', \
                 '$2a$10$MwKxT13/lPxFMjvrkm0dL.QJuNll1zljDU.eOoRVvgrWF.kf/hyC2') \
         ON CONFLICT (id) DO NOTHING",
    )
    .execute(&db.0)
    .await
    .unwrap();
}

/// Signs a `BTreeMap` of ECPay callback fields exactly the way the real
/// provider does — sorted parameters wrapped in the platform's own
/// `HashKey`/`HashIV` — so `parse_callback` genuinely verifies it, rather
/// than a test that only exercises the "already verified" path.
fn sign_ecpay_callback(mut fields: BTreeMap<String, String>) -> BTreeMap<String, String> {
    fields.remove("CheckMacValue");
    let mac =
        paygate_provider::ecpay::check_mac_value("pwFHCqoQZGmho4w6", "EkRm7iFT261dpevs", &fields);
    fields.insert("CheckMacValue".to_string(), mac);
    fields
}

fn ecpay_callback_fields(provider_trade_no: &str, amount: i64) -> BTreeMap<String, String> {
    let mut fields = BTreeMap::new();
    fields.insert("MerchantID".to_string(), "2000132".to_string());
    fields.insert("MerchantTradeNo".to_string(), provider_trade_no.to_string());
    fields.insert("TradeNo".to_string(), "2109210000000".to_string());
    fields.insert("RtnCode".to_string(), "1".to_string());
    fields.insert("RtnMsg".to_string(), "Succeeded".to_string());
    fields.insert("TradeAmt".to_string(), amount.to_string());
    fields.insert("PaymentDate".to_string(), "2026/09/21 14:03:11".to_string());
    fields.insert("SimulatePaid".to_string(), "0".to_string());
    fields
}

async fn call_webhook(state: &AppState, form: &BTreeMap<String, String>) -> (StatusCode, Bytes) {
    let body = serde_urlencoded::to_string(form).unwrap();
    let response = crate::routes::webhooks::receive_callback(
        State(state.clone()),
        Path("ecpay".to_string()),
        Bytes::from(body),
    )
    .await;
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, bytes)
}

/// `spec.md`, "Test plan" -> `api`: "`1|OK` written only after the commit."
/// The half that is easy to get backwards is proven directly: a callback
/// that settles a real, seeded attempt is acknowledged AND the row really
/// is `succeeded` afterwards; a callback whose transaction never reaches a
/// commit at all (`apply_callback` returns `Err` before writing anything,
/// because the attempt it names does not exist) is answered with something
/// other than `1|OK`, and nothing in `provider_events` records it. Forcing
/// an actual mid-transaction Postgres failure (a killed connection) would
/// prove the same property no more directly than this does, and would trade
/// a real assertion for a flaky one.
#[tokio::test]
async fn one_ok_is_written_only_once_the_settlement_actually_committed() {
    let (db, _pg) = start_postgres().await;
    let (redis, _redis) = start_redis().await;
    seed_ecpay_merchant(&db).await;
    let state = test_state(db.clone(), redis, false);

    let payment = repo::create_order(
        &db,
        repo::NewOrder {
            merchant_id: 1,
            merchant_trade_no: "INT-COMMIT-1".to_string(),
            amount: 1000,
            currency: "USD".to_string(),
            item_desc: "Beans".to_string(),
            notify_url: "/demo-merchant/api/notify".to_string(),
            client_back_url: "/shop/result".to_string(),
        },
    )
    .await
    .unwrap();
    let opened = repo::open_attempt(&db, 1, payment.id, ProviderCode::Ecpay, "PGT")
        .await
        .unwrap();

    // A genuine callback for a real attempt: acknowledged, and committed.
    let good = sign_ecpay_callback(ecpay_callback_fields(&opened.provider_trade_no, 1000));
    let (status, body) = call_webhook(&state, &good).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(&body[..], b"1|OK");

    let settled = crate::db::find_payment_by_id_unscoped(&db, payment.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(settled.status, PaymentStatus::Succeeded);

    // A callback naming an attempt this database has never heard of: never
    // committed, never acknowledged.
    let unknown = sign_ecpay_callback(ecpay_callback_fields("DOESNOTEXIST0000001", 1000));
    let (status2, body2) = call_webhook(&state, &unknown).await;
    assert_ne!(status2, StatusCode::OK);
    assert_ne!(&body2[..], b"1|OK");

    // Exactly the one row the FIRST, genuine callback committed — the
    // second, uncommitted one added nothing.
    let events: i64 = sqlx::query_scalar("SELECT count(*) FROM provider_events")
        .fetch_one(&db.0)
        .await
        .unwrap();
    assert_eq!(
        events, 1,
        "an uncommitted callback must leave no additional audit row"
    );
}

/// The same callback delivered twice must settle once and be acknowledged
/// both times — the other half of "only after the commit" (the SECOND
/// delivery's `1|OK` is answered from the deduplication branch, which
/// commits nothing new, and must still be exactly `1|OK`).
#[tokio::test]
async fn a_redelivered_callback_is_acknowledged_again_without_settling_twice() {
    let (db, _pg) = start_postgres().await;
    let (redis, _redis) = start_redis().await;
    seed_ecpay_merchant(&db).await;
    let state = test_state(db.clone(), redis, false);

    let payment = repo::create_order(
        &db,
        repo::NewOrder {
            merchant_id: 1,
            merchant_trade_no: "INT-COMMIT-2".to_string(),
            amount: 500,
            currency: "USD".to_string(),
            item_desc: "Beans".to_string(),
            notify_url: "/demo-merchant/api/notify".to_string(),
            client_back_url: "/shop/result".to_string(),
        },
    )
    .await
    .unwrap();
    let opened = repo::open_attempt(&db, 1, payment.id, ProviderCode::Ecpay, "PGT")
        .await
        .unwrap();
    let form = sign_ecpay_callback(ecpay_callback_fields(&opened.provider_trade_no, 500));

    let (status1, body1) = call_webhook(&state, &form).await;
    let (status2, body2) = call_webhook(&state, &form).await;
    assert_eq!((status1, &body1[..]), (StatusCode::OK, &b"1|OK"[..]));
    assert_eq!((status2, &body2[..]), (StatusCode::OK, &b"1|OK"[..]));

    let succeeded_events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM payment_events WHERE payment_id = $1 AND event_type = 'PaymentSucceeded'",
    )
    .bind(payment.id)
    .fetch_one(&db.0)
    .await
    .unwrap();
    assert_eq!(succeeded_events, 1);
}

/// `spec.md`, "Test plan" -> `api`: "cookie attributes for both
/// `COOKIE_SECURE` settings" — driven through the real `POST
/// /dashboard/login` handler against a real seeded user and a real Redis,
/// not just the pure `cookies` module.
#[tokio::test]
async fn login_cookies_carry_secure_only_when_cookie_secure_is_true() {
    for cookie_secure in [true, false] {
        let (db, _pg) = start_postgres().await;
        let (redis, _redis) = start_redis().await;
        seed_dashboard_user(&db).await;
        let state = test_state(db, redis, cookie_secure);

        let body = Bytes::from(r#"{"email":"owner@acme.test","password":"secret123"}"#);
        let response = crate::routes::dashboard::login(State(state), body)
            .await
            .expect("valid credentials should sign in")
            .into_response();
        assert_eq!(response.status(), StatusCode::OK);

        let cookies: Vec<String> = response
            .headers()
            .get_all(axum::http::header::SET_COOKIE)
            .iter()
            .map(|v| v.to_str().unwrap().to_string())
            .collect();
        let session = cookies
            .iter()
            .find(|c| c.starts_with("session="))
            .expect("a session cookie");
        let hint = cookies
            .iter()
            .find(|c| c.starts_with("session_hint="))
            .expect("a session_hint cookie");
        assert!(session.contains("HttpOnly"));
        assert!(!hint.contains("HttpOnly"));
        assert_eq!(session.contains("; Secure"), cookie_secure);
        assert_eq!(hint.contains("; Secure"), cookie_secure);
    }
}

/// `spec.md`, "Test plan" -> `api`: "the `401` path clearing `session_hint`" —
/// exercised through the real [`DashboardSession`] extractor against a real
/// (empty) Redis, not the pure `ApiError` unit test alone.
#[tokio::test]
async fn a_session_redis_has_never_heard_of_clears_the_session_hint_cookie() {
    let (db, _pg) = start_postgres().await;
    let (redis, _redis) = start_redis().await;
    let state = test_state(db, redis, false);

    let request = Request::builder()
        .header(
            axum::http::header::COOKIE,
            "session=nobody-issued-this-token",
        )
        .body(())
        .unwrap();
    let (mut parts, ()) = request.into_parts();
    let err = DashboardSession::from_request_parts(&mut parts, &state)
        .await
        .expect_err("an unknown session token must be refused");
    assert!(matches!(err, ApiError::Unauthenticated));

    let response = err.into_response();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let cookie = response
        .headers()
        .get(axum::http::header::SET_COOKIE)
        .expect("a Set-Cookie header")
        .to_str()
        .unwrap();
    assert!(cookie.starts_with("session_hint=;"));
    assert!(cookie.contains("Max-Age=0"));
}

/// `spec.md`, "Refunds": "paygate sends its own refund id as the provider's
/// refund number, so the provider deduplicates a retry... A refund whose
/// answer was lost is safe to send again." Driven straight through
/// `refund_flow::execute` against a real, row-locked Postgres — `provider.
/// invalid` (`test_state`) makes every provider call fail, exactly the shape
/// of a lost answer, so a genuine merchant retry (same idempotency key, same
/// amount) can be told apart from a fresh reservation by whether it reuses
/// the SAME `refund_id` and the SAME `refunds` row, which is what actually
/// lets the real provider deduplicate it (`refunds.feature`, "A refund whose
/// answer was lost is safe to send again").
#[tokio::test]
async fn a_refund_retry_reuses_the_pending_reservations_id_not_a_fresh_one() {
    let (db, _pg) = start_postgres().await;
    let (redis, _redis) = start_redis().await;
    seed_ecpay_merchant(&db).await;
    let state = test_state(db.clone(), redis, false);

    let payment = repo::create_order(
        &db,
        repo::NewOrder {
            merchant_id: 1,
            merchant_trade_no: "INT-RETRY-1".to_string(),
            amount: 5000,
            currency: "USD".to_string(),
            item_desc: "Beans".to_string(),
            notify_url: "/demo-merchant/api/notify".to_string(),
            client_back_url: "/shop/result".to_string(),
        },
    )
    .await
    .unwrap();
    let opened = repo::open_attempt(&db, 1, payment.id, ProviderCode::Ecpay, "PGT")
        .await
        .unwrap();
    // Settle the attempt and the order by hand — no callback needed, since
    // this test is about the refund path, not settlement.
    sqlx::query(
        "UPDATE payment_attempts SET status = 'succeeded', provider_charge_id = 'ch_test', \
            settled_at = now() WHERE id = $1",
    )
    .bind(opened.attempt_id)
    .execute(&db.0)
    .await
    .unwrap();
    sqlx::query("UPDATE payments SET status = 'succeeded' WHERE id = $1")
        .bind(payment.id)
        .execute(&db.0)
        .await
        .unwrap();

    let merchant = crate::db::fetch_merchant(&db, 1).await.unwrap().unwrap();
    let payment_row = crate::db::find_payment_by_id_unscoped(&db, payment.id)
        .await
        .unwrap()
        .unwrap();

    // First attempt: the provider never answers (there is nothing at
    // `provider.invalid`), exactly like the lost-answer scenario's `408`.
    let first = crate::refund_flow::execute(
        &state,
        &payment_row,
        &merchant,
        None,
        1000,
        None,
        Some("retry-me"),
    )
    .await
    .unwrap();
    assert!(
        !first.succeeded,
        "an unreachable provider must not be reported as a success"
    );

    let amount_after_first: i64 =
        sqlx::query_scalar("SELECT amount_refunded FROM payments WHERE id = $1")
            .bind(payment.id)
            .fetch_one(&db.0)
            .await
            .unwrap();
    assert_eq!(
        amount_after_first, 0,
        "a failed provider call must roll the reservation back"
    );

    // The merchant retries with the SAME idempotency key and the SAME
    // amount — the only two facts a genuine retry is defined by.
    let second = crate::refund_flow::execute(
        &state,
        &payment_row,
        &merchant,
        None,
        1000,
        None,
        Some("retry-me"),
    )
    .await
    .unwrap();
    assert!(!second.succeeded);

    assert_eq!(
        first.refund_id, second.refund_id,
        "a retry must resend the SAME refund id, or the provider has nothing to deduplicate on"
    );

    let refund_rows: i64 = sqlx::query_scalar("SELECT count(*) FROM refunds WHERE payment_id = $1")
        .bind(payment.id)
        .fetch_one(&db.0)
        .await
        .unwrap();
    assert_eq!(
        refund_rows, 1,
        "a retry must resume the existing reservation, not open a second one"
    );

    // A DIFFERENT amount under the SAME key is a genuinely different
    // request (`refunds.feature`, "A provider that refuses the refund
    // changes nothing here") and must get its own, different id.
    let different_amount = crate::refund_flow::execute(
        &state,
        &payment_row,
        &merchant,
        None,
        2000,
        None,
        Some("retry-me"),
    )
    .await
    .unwrap();
    assert_ne!(different_amount.refund_id, first.refund_id);
}

/// Bug: resuming a timed-out retry whose re-reservation fails always
/// answered `PAYMENT_NOT_REFUNDABLE`, even when the payment is very much
/// still `succeeded` and the true problem is that a DIFFERENT refund (a
/// different idempotency key) took the remainder in between the first,
/// unanswered attempt and this retry — `refund_flow::execute`'s fresh-request
/// path already reports that accurately, via `check_refund`, as
/// `RefundExceedsRemaining { remaining }`; the resume path must now match it.
#[tokio::test]
async fn a_resumed_retry_that_can_no_longer_fit_reports_the_remaining_amount_not_a_blanket_refusal()
{
    let (db, _pg) = start_postgres().await;
    let (redis, _redis) = start_redis().await;
    seed_ecpay_merchant(&db).await;
    let state = test_state(db.clone(), redis, false);

    let payment = repo::create_order(
        &db,
        repo::NewOrder {
            merchant_id: 1,
            merchant_trade_no: "INT-RETRY-RACE-1".to_string(),
            amount: 600,
            currency: "USD".to_string(),
            item_desc: "Beans".to_string(),
            notify_url: "/demo-merchant/api/notify".to_string(),
            client_back_url: "/shop/result".to_string(),
        },
    )
    .await
    .unwrap();
    let opened = repo::open_attempt(&db, 1, payment.id, ProviderCode::Ecpay, "PGT")
        .await
        .unwrap();
    sqlx::query(
        "UPDATE payment_attempts SET status = 'succeeded', provider_charge_id = 'ch_test', \
            settled_at = now() WHERE id = $1",
    )
    .bind(opened.attempt_id)
    .execute(&db.0)
    .await
    .unwrap();
    sqlx::query("UPDATE payments SET status = 'succeeded' WHERE id = $1")
        .bind(payment.id)
        .execute(&db.0)
        .await
        .unwrap();

    let merchant = crate::db::fetch_merchant(&db, 1).await.unwrap().unwrap();
    let payment_row = crate::db::find_payment_by_id_unscoped(&db, payment.id)
        .await
        .unwrap()
        .unwrap();

    // Refund A, key K1, for the whole 600 — the provider never answers
    // (`http://provider.invalid`), the same shape as a genuine timeout. Its
    // reservation is rolled back, and its own `refunds` row is left
    // `pending`, exactly `refund_flow`'s own doc comment on the resume path.
    let refund_a =
        crate::refund_flow::execute(&state, &payment_row, &merchant, None, 600, None, Some("K1"))
            .await
            .unwrap();
    assert!(!refund_a.succeeded);

    // Refund B, a DIFFERENT request entirely (no idempotency key of its
    // own — this stands in for a second, successful refund under a
    // different key K2), reserves the very same 600 directly against the
    // order's own budget. Calling `repo::create_refund` alone (rather than
    // the whole `execute`, which would also complete-and-roll-back against
    // the same unreachable provider) is what leaves this reservation
    // standing, precisely the state a genuinely SUCCEEDED refund B would
    // leave behind.
    repo::create_refund(&db, 1, payment.id, uuid::Uuid::now_v7(), 600, None, None)
        .await
        .expect("refund B reserves the whole remainder");

    let amount_refunded: i64 =
        sqlx::query_scalar("SELECT amount_refunded FROM payments WHERE id = $1")
            .bind(payment.id)
            .fetch_one(&db.0)
            .await
            .unwrap();
    assert_eq!(amount_refunded, 600, "refund B took the whole remainder");

    // The client now retries K1 — the resume path. Nothing is left to give
    // it: the honest answer is `RefundExceedsRemaining { remaining: 0 }`,
    // never `PaymentNotRefundable` (the payment is still very much
    // `succeeded`).
    match crate::refund_flow::execute(&state, &payment_row, &merchant, None, 600, None, Some("K1"))
        .await
    {
        Ok(_) => panic!("nothing is left to resume this reservation into"),
        Err(ApiError::RefundExceedsRemaining { remaining }) => {
            assert_eq!(remaining, 0, "the whole 600 is already accounted for");
        }
        Err(other) => panic!(
            "expected RefundExceedsRemaining {{ remaining: 0 }}, got {other:?} \
             (PAYMENT_NOT_REFUNDABLE would wrongly tell the merchant this payment \
             can never be refunded, when the truth is only that none of it is left)"
        ),
    }
}

/// `spec.md`, "Test plan" -> `api`: the idempotency fast path degrading
/// correctly when Redis is unavailable while PostgreSQL still answers —
/// `resilience.feature`'s own race, exercised here at the level this crate
/// controls directly (`crate::idempotency`) rather than over HTTP, with a
/// REAL Redis stopped mid-test rather than one that was never configured.
#[tokio::test]
async fn idempotency_falls_back_to_postgres_once_redis_is_stopped() {
    let (db, _pg) = start_postgres().await;
    let (redis, redis_container) = start_redis().await;
    seed_ecpay_merchant(&db).await;
    let state = test_state(db.clone(), redis, false);

    // Started, then completed, while Redis is still up: the ordinary path.
    let outcome = crate::idempotency::begin(&state, 1, "int-idem-1", "fp-1")
        .await
        .unwrap();
    assert!(matches!(outcome, crate::idempotency::Outcome::Started));
    let stored_body = json!({ "id": "00000000-0000-7000-8000-000000000001", "amount": 4200 });
    crate::idempotency::complete(&state, 1, "int-idem-1", 201, &stored_body).await;

    // Stop Redis outright — not a restart, an actual outage for the rest of
    // this test.
    redis_container
        .stop()
        .await
        .expect("the redis container should stop");

    // The Redis lock can no longer be acquired at all; PostgreSQL alone
    // still knows this exact key completed, with this exact answer, and
    // replays it rather than doing the work again.
    let replay = crate::idempotency::begin(&state, 1, "int-idem-1", "fp-1")
        .await
        .unwrap();
    match replay {
        crate::idempotency::Outcome::Replay { status, body } => {
            assert_eq!(status, 201);
            assert_eq!(body, stored_body);
        }
        other => panic!("expected a PostgreSQL replay with Redis down, got {other:?}"),
    }

    // A key nobody has ever used still starts normally: PostgreSQL's own
    // `INSERT ... ON CONFLICT` needs no help from Redis to do that.
    let fresh = crate::idempotency::begin(&state, 1, "int-idem-2", "fp-2")
        .await
        .unwrap();
    assert!(matches!(fresh, crate::idempotency::Outcome::Started));

    // And the same key, sent again with a DIFFERENT fingerprint, is still
    // refused as reused — PostgreSQL's own comparison, not Redis's.
    let reused = crate::idempotency::begin(&state, 1, "int-idem-1", "fp-different")
        .await
        .unwrap();
    assert!(matches!(reused, crate::idempotency::Outcome::Reused));
}
