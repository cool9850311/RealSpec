//! Integration tests against a real PostgreSQL, started by `testcontainers`
//! (`spec.md`, "Isolation": the library owns every container's lifecycle).
//!
//! These are the ones `spec.md`, "Layers" (the "Unit tests, by crate" list
//! under it) asks for that a pure function cannot prove: that the whole
//! write really is one transaction, that the conditional
//! `UPDATE … WHERE status = 'pending'` really does surface as
//! "state did not change" under a genuine race, that `create_refund`'s
//! reservation really does serialise four concurrent refunds under the row
//! lock, and that `apply_callback`'s six outcomes really land as the right
//! rows — including that the same `(provider_code, event_id)` delivered
//! twice settles once.

mod common;

use paygate_domain::{CallbackOutcome, ProviderCode};
use paygate_infra::repo;
use uuid::Uuid;

fn new_order(merchant_trade_no: &str, amount: i64) -> repo::NewOrder {
    repo::NewOrder {
        merchant_id: common::MERCHANT_ID,
        merchant_trade_no: merchant_trade_no.to_string(),
        amount,
        currency: "USD".to_string(),
        item_desc: "Beans".to_string(),
        notify_url: "/demo-merchant/api/notify".to_string(),
        client_back_url: "/shop/result".to_string(),
    }
}

#[tokio::test]
async fn the_whole_settlement_is_one_transaction_and_a_conflict_rolls_back_everything() {
    let fx = common::start_postgres().await;
    let db = &fx.db;

    let order = repo::create_order(db, new_order("ACME-TX-1", 2500))
        .await
        .expect("create order");
    let attempt = repo::open_attempt(
        db,
        common::MERCHANT_ID,
        order.id,
        ProviderCode::Ecpay,
        "ACME1",
    )
    .await
    .expect("open attempt");

    // Force the LAST write of the transaction (the notification) to fail
    // with a genuine PostgreSQL error, so that the earlier writes in the
    // same transaction — the payment's own status, the attempt, the
    // provider_events row, the PaymentSucceeded audit row — can only survive
    // if this really is one transaction rather than four separate ones. A
    // dropped column is a blunt way to manufacture a real error, but it
    // exercises Postgres's and sqlx's own rollback, not anything this crate
    // special-cased for a test.
    sqlx::query("ALTER TABLE notifications DROP COLUMN payload")
        .execute(db.pool())
        .await
        .expect("drop the notifications payload column");

    let facts = common::paid_facts(&attempt.provider_trade_no, "evt-collide", 2500);
    let merchant = common::merchant_context("ACME-TX-1");
    let result = repo::apply_callback(
        db,
        ProviderCode::Ecpay,
        &facts,
        &merchant,
        common::MERCHANT_HASH_KEY,
        common::MERCHANT_HASH_IV,
    )
    .await;
    assert!(
        result.is_err(),
        "the notifications insert failing must fail the whole call"
    );

    // Nothing from the failed attempt survived: not the payment's status,
    // not the attempt's, not a provider_events row, not a notification.
    let (status, card_brand): (String, Option<String>) =
        sqlx::query_as("SELECT status, card_brand FROM payments WHERE id = $1")
            .bind(order.id)
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(status, "pending");
    assert_eq!(card_brand, None);

    let attempt_status: String =
        sqlx::query_scalar("SELECT status FROM payment_attempts WHERE id = $1")
            .bind(attempt.attempt_id)
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(attempt_status, "redirected");

    let provider_events: i64 =
        sqlx::query_scalar("SELECT count(*) FROM provider_events WHERE payment_id = $1")
            .bind(order.id)
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(
        provider_events, 0,
        "the provider_events insert must not have survived"
    );

    let notifications: i64 =
        sqlx::query_scalar("SELECT count(*) FROM notifications WHERE payment_id = $1")
            .bind(order.id)
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(notifications, 0);

    // And the successful path really does write all four kinds of row, in
    // one go, once nothing is in its way.
    sqlx::query("ALTER TABLE notifications ADD COLUMN payload JSONB NOT NULL DEFAULT '{}'::jsonb")
        .execute(db.pool())
        .await
        .unwrap();
    let facts2 = common::paid_facts(&attempt.provider_trade_no, "evt-real", 2500);
    let application = repo::apply_callback(
        db,
        ProviderCode::Ecpay,
        &facts2,
        &merchant,
        common::MERCHANT_HASH_KEY,
        common::MERCHANT_HASH_IV,
    )
    .await
    .expect("apply_callback should now succeed");
    assert_eq!(application.outcome, CallbackOutcome::Applied);

    let (status2,): (String,) = sqlx::query_as("SELECT status FROM payments WHERE id = $1")
        .bind(order.id)
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(status2, "succeeded");
    let notifications2: i64 =
        sqlx::query_scalar("SELECT count(*) FROM notifications WHERE payment_id = $1")
            .bind(order.id)
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(notifications2, 1);
}

#[tokio::test]
async fn two_concurrent_settles_of_one_attempt_produce_one_applied_and_one_no_op() {
    let fx = common::start_postgres().await;
    let db = std::sync::Arc::new(fx.db);

    let order = repo::create_order(&db, new_order("ACME-RACE-1", 1000))
        .await
        .expect("create order");
    let attempt = repo::open_attempt(
        &db,
        common::MERCHANT_ID,
        order.id,
        ProviderCode::Ecpay,
        "ACME1",
    )
    .await
    .expect("open attempt");

    let merchant = common::merchant_context("ACME-RACE-1");
    let trade_no = attempt.provider_trade_no.clone();

    // Two DIFFERENT callback deliveries (different provider_event_id) for
    // the SAME attempt, arriving at one instant — exactly the race the
    // reconciler and a redelivered callback create. `apply_callback`'s own
    // row lock on the attempt must serialise them so exactly one settles the
    // order (`Applied`) and the other finds it already done (`NoOp`), never
    // both `Applied` and never a lost update.
    let db_a = db.clone();
    let merchant_a = merchant.clone();
    let trade_no_a = trade_no.clone();
    let handle_a = tokio::spawn(async move {
        let facts = common::paid_facts(&trade_no_a, "evt-a", 1000);
        repo::apply_callback(
            &db_a,
            ProviderCode::Ecpay,
            &facts,
            &merchant_a,
            common::MERCHANT_HASH_KEY,
            common::MERCHANT_HASH_IV,
        )
        .await
    });

    let db_b = db.clone();
    let merchant_b = merchant.clone();
    let trade_no_b = trade_no.clone();
    let handle_b = tokio::spawn(async move {
        let facts = common::paid_facts(&trade_no_b, "evt-b", 1000);
        repo::apply_callback(
            &db_b,
            ProviderCode::Ecpay,
            &facts,
            &merchant_b,
            common::MERCHANT_HASH_KEY,
            common::MERCHANT_HASH_IV,
        )
        .await
    });

    let outcome_a = handle_a.await.unwrap().expect("apply_callback a");
    let outcome_b = handle_b.await.unwrap().expect("apply_callback b");

    let outcomes = [outcome_a.outcome, outcome_b.outcome];
    let applied = outcomes
        .iter()
        .filter(|o| **o == CallbackOutcome::Applied)
        .count();
    let no_op = outcomes
        .iter()
        .filter(|o| **o == CallbackOutcome::NoOp)
        .count();
    assert_eq!(
        applied, 1,
        "exactly one delivery should settle the order: {outcomes:?}"
    );
    assert_eq!(
        no_op, 1,
        "the loser should find nothing left to do: {outcomes:?}"
    );

    let (status,): (String,) = sqlx::query_as("SELECT status FROM payments WHERE id = $1")
        .bind(order.id)
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(status, "succeeded");

    // Settled once: one PaymentSucceeded event, one notification, whichever
    // delivery won.
    let succeeded_events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM payment_events WHERE payment_id = $1 AND event_type = 'PaymentSucceeded'",
    )
    .bind(order.id)
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(succeeded_events, 1);
    let notifications: i64 =
        sqlx::query_scalar("SELECT count(*) FROM notifications WHERE payment_id = $1")
            .bind(order.id)
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(notifications, 1);
}

#[tokio::test]
async fn four_concurrent_refunds_never_move_more_than_the_charge() {
    let fx = common::start_postgres().await;
    let db = std::sync::Arc::new(fx.db);

    let order = repo::create_order(&db, new_order("ACME-REFUND-RACE", 1000))
        .await
        .expect("create order");
    let attempt = repo::open_attempt(
        &db,
        common::MERCHANT_ID,
        order.id,
        ProviderCode::Ecpay,
        "ACME1",
    )
    .await
    .expect("open attempt");
    let merchant = common::merchant_context("ACME-REFUND-RACE");
    let facts = common::paid_facts(&attempt.provider_trade_no, "evt-settle", 1000);
    repo::apply_callback(
        &db,
        ProviderCode::Ecpay,
        &facts,
        &merchant,
        common::MERCHANT_HASH_KEY,
        common::MERCHANT_HASH_IV,
    )
    .await
    .expect("settle the order so it can be refunded");

    // Four concurrent refunds of 400 against a 1000 charge: room for exactly
    // two (800), never three (1200).
    let mut handles = Vec::new();
    for _ in 0..4 {
        let db = db.clone();
        let payment_id = order.id;
        handles.push(tokio::spawn(async move {
            repo::create_refund(
                &db,
                common::MERCHANT_ID,
                payment_id,
                Uuid::now_v7(),
                400,
                None,
                None,
            )
            .await
        }));
    }

    let mut ok = 0;
    let mut exceeded = 0;
    for h in handles {
        match h.await.unwrap() {
            Ok(_) => ok += 1,
            Err(paygate_infra::Error::RefundExceedsRemaining { .. }) => exceeded += 1,
            Err(other) => panic!("unexpected error: {other:?}"),
        }
    }
    assert_eq!(ok, 2, "exactly two of the four refunds should be admitted");
    assert_eq!(exceeded, 2);

    let amount_refunded: i64 =
        sqlx::query_scalar("SELECT amount_refunded FROM payments WHERE id = $1")
            .bind(order.id)
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(amount_refunded, 800, "never past what the charge allows");

    let refund_rows: i64 = sqlx::query_scalar("SELECT count(*) FROM refunds WHERE payment_id = $1")
        .bind(order.id)
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(refund_rows, 2);
}

#[tokio::test]
async fn apply_callback_produces_every_outcome_and_deduplicates_a_redelivered_event() {
    let fx = common::start_postgres().await;
    let db = &fx.db;
    let merchant = common::merchant_context("ACME-OUTCOMES");

    // -- Applied, and the same event delivered twice settles once --------
    let order = repo::create_order(db, new_order("ACME-OUTCOMES-1", 1000))
        .await
        .unwrap();
    let attempt = repo::open_attempt(
        db,
        common::MERCHANT_ID,
        order.id,
        ProviderCode::Ecpay,
        "ACME1",
    )
    .await
    .unwrap();
    let facts = common::paid_facts(&attempt.provider_trade_no, "evt-dup-1", 1000);
    let first = repo::apply_callback(
        db,
        ProviderCode::Ecpay,
        &facts,
        &merchant,
        common::MERCHANT_HASH_KEY,
        common::MERCHANT_HASH_IV,
    )
    .await
    .unwrap();
    assert_eq!(first.outcome, CallbackOutcome::Applied);
    let second = repo::apply_callback(
        db,
        ProviderCode::Ecpay,
        &facts,
        &merchant,
        common::MERCHANT_HASH_KEY,
        common::MERCHANT_HASH_IV,
    )
    .await
    .unwrap();
    assert_eq!(
        second.outcome,
        CallbackOutcome::Applied,
        "a redelivery replays the stored outcome"
    );
    let provider_events: i64 =
        sqlx::query_scalar("SELECT count(*) FROM provider_events WHERE payment_id = $1")
            .bind(order.id)
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(
        provider_events, 1,
        "the same event_id must not be recorded twice"
    );
    let succeeded_events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM payment_events WHERE payment_id = $1 AND event_type = 'PaymentSucceeded'",
    )
    .bind(order.id)
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(succeeded_events, 1, "settled once, not twice");

    // -- AmountMismatch ----------------------------------------------------
    let order2 = repo::create_order(db, new_order("ACME-OUTCOMES-2", 1000))
        .await
        .unwrap();
    let attempt2 = repo::open_attempt(
        db,
        common::MERCHANT_ID,
        order2.id,
        ProviderCode::Ecpay,
        "ACME1",
    )
    .await
    .unwrap();
    let facts2 = common::paid_facts(&attempt2.provider_trade_no, "evt-mismatch", 999);
    let mismatch = repo::apply_callback(
        db,
        ProviderCode::Ecpay,
        &facts2,
        &merchant,
        common::MERCHANT_HASH_KEY,
        common::MERCHANT_HASH_IV,
    )
    .await
    .unwrap();
    assert_eq!(mismatch.outcome, CallbackOutcome::AmountMismatch);
    assert!(!repo::should_acknowledge(mismatch.outcome));

    // -- Applied (declined), then Conflict on a second attempt -------------
    let order3 = repo::create_order(db, new_order("ACME-OUTCOMES-3", 1000))
        .await
        .unwrap();
    let attempt3a = repo::open_attempt(
        db,
        common::MERCHANT_ID,
        order3.id,
        ProviderCode::Ecpay,
        "ACME1",
    )
    .await
    .unwrap();
    // Both attempts are opened while the order is still pending — exactly
    // like a customer who reloads before paying either one
    // (`spec.md`, "One order, three numbers"); `open_attempt` refuses a
    // second attempt once the order has already settled, so the second
    // attempt MUST exist before the first one's callback is applied.
    let attempt3b = repo::open_attempt(
        db,
        common::MERCHANT_ID,
        order3.id,
        ProviderCode::Ecpay,
        "ACME1",
    )
    .await
    .unwrap();
    let paid3 = common::paid_facts(&attempt3a.provider_trade_no, "evt-3-paid", 1000);
    repo::apply_callback(
        db,
        ProviderCode::Ecpay,
        &paid3,
        &merchant,
        common::MERCHANT_HASH_KEY,
        common::MERCHANT_HASH_IV,
    )
    .await
    .unwrap();
    let declined3 = common::failed_facts(
        &attempt3b.provider_trade_no,
        "evt-3-declined",
        1000,
        10_100_058,
    );
    let conflict = repo::apply_callback(
        db,
        ProviderCode::Ecpay,
        &declined3,
        &merchant,
        common::MERCHANT_HASH_KEY,
        common::MERCHANT_HASH_IV,
    )
    .await
    .unwrap();
    assert_eq!(conflict.outcome, CallbackOutcome::Conflict);
    let attempt3b_status: String =
        sqlx::query_scalar("SELECT status FROM payment_attempts WHERE id = $1")
            .bind(attempt3b.attempt_id)
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(
        attempt3b_status, "redirected",
        "a conflict leaves the attempt untouched"
    );

    // -- Duplicate: a second attempt of a settled order also paid ----------
    let order4 = repo::create_order(db, new_order("ACME-OUTCOMES-4", 1000))
        .await
        .unwrap();
    let attempt4a = repo::open_attempt(
        db,
        common::MERCHANT_ID,
        order4.id,
        ProviderCode::Ecpay,
        "ACME1",
    )
    .await
    .unwrap();
    let attempt4b = repo::open_attempt(
        db,
        common::MERCHANT_ID,
        order4.id,
        ProviderCode::Ecpay,
        "ACME1",
    )
    .await
    .unwrap();
    let paid4a = common::paid_facts(&attempt4a.provider_trade_no, "evt-4-a", 1000);
    repo::apply_callback(
        db,
        ProviderCode::Ecpay,
        &paid4a,
        &merchant,
        common::MERCHANT_HASH_KEY,
        common::MERCHANT_HASH_IV,
    )
    .await
    .unwrap();
    let paid4b = common::paid_facts(&attempt4b.provider_trade_no, "evt-4-b", 1000);
    let duplicate = repo::apply_callback(
        db,
        ProviderCode::Ecpay,
        &paid4b,
        &merchant,
        common::MERCHANT_HASH_KEY,
        common::MERCHANT_HASH_IV,
    )
    .await
    .unwrap();
    assert_eq!(duplicate.outcome, CallbackOutcome::Duplicate);
    assert_eq!(duplicate.duplicate_amount, Some(1000));

    // -- UnknownCode ---------------------------------------------------------
    let order5 = repo::create_order(db, new_order("ACME-OUTCOMES-5", 1000))
        .await
        .unwrap();
    let attempt5 = repo::open_attempt(
        db,
        common::MERCHANT_ID,
        order5.id,
        ProviderCode::Ecpay,
        "ACME1",
    )
    .await
    .unwrap();
    let pending5 = common::failed_facts(&attempt5.provider_trade_no, "evt-5", 1000, 10_300_066);
    let unknown = repo::apply_callback(
        db,
        ProviderCode::Ecpay,
        &pending5,
        &merchant,
        common::MERCHANT_HASH_KEY,
        common::MERCHANT_HASH_IV,
    )
    .await
    .unwrap();
    assert_eq!(unknown.outcome, CallbackOutcome::UnknownCode);
    let (attempt5_status, attempt5_rtn): (String, Option<i32>) =
        sqlx::query_as("SELECT status, rtn_code FROM payment_attempts WHERE id = $1")
            .bind(attempt5.attempt_id)
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(attempt5_status, "redirected");
    assert_eq!(attempt5_rtn, Some(10_300_066));
}

#[tokio::test]
async fn a_refused_amount_mismatch_settles_once_a_corrected_redelivery_arrives() {
    let fx = common::start_postgres().await;
    let db = &fx.db;
    let merchant = common::merchant_context("ACME-REDELIVER-1");

    let order = repo::create_order(db, new_order("ACME-REDELIVER-1", 2500))
        .await
        .unwrap();
    let attempt = repo::open_attempt(
        db,
        common::MERCHANT_ID,
        order.id,
        ProviderCode::Ecpay,
        "ACME1",
    )
    .await
    .unwrap();

    // First delivery: the provider's own event id, but the wrong amount —
    // refused outright, and nothing is settled (`spec.md`, "Being told by
    // the provider": "An amount that is not the order's is refused, not
    // reconciled away").
    let mut facts = common::paid_facts(&attempt.provider_trade_no, "evt-redeliver-1", 2501);
    let refused = repo::apply_callback(
        db,
        ProviderCode::Ecpay,
        &facts,
        &merchant,
        common::MERCHANT_HASH_KEY,
        common::MERCHANT_HASH_IV,
    )
    .await
    .unwrap();
    assert_eq!(refused.outcome, CallbackOutcome::AmountMismatch);
    assert!(!repo::should_acknowledge(refused.outcome));

    let (status,): (String,) = sqlx::query_as("SELECT status FROM payments WHERE id = $1")
        .bind(order.id)
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(status, "pending", "a refused delivery settles nothing");

    // The SAME event id (a real redelivery, not a fresh one), this time with
    // the correct amount — the provider's own retry once the callback it is
    // still holding finally matches. `amount_mismatch` committed nothing the
    // first time, so this must be RE-DECIDED, not replayed, or the order can
    // never settle (the bug this test guards against).
    facts.amount = 2500;
    let settled = repo::apply_callback(
        db,
        ProviderCode::Ecpay,
        &facts,
        &merchant,
        common::MERCHANT_HASH_KEY,
        common::MERCHANT_HASH_IV,
    )
    .await
    .unwrap();
    assert_eq!(
        settled.outcome,
        CallbackOutcome::Applied,
        "the corrected redelivery must be re-evaluated, not replayed as amount_mismatch forever"
    );
    assert!(repo::should_acknowledge(settled.outcome));

    let (status2,): (String,) = sqlx::query_as("SELECT status FROM payments WHERE id = $1")
        .bind(order.id)
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(status2, "succeeded");

    // The row is keyed on (provider_code, event_id) alone: this can only
    // ever be ONE row, updated in place from amount_mismatch to applied —
    // never a second row for the same event.
    let outcomes: Vec<(String,)> =
        sqlx::query_as("SELECT outcome FROM provider_events WHERE payment_id = $1")
            .bind(order.id)
            .fetch_all(db.pool())
            .await
            .unwrap();
    assert_eq!(
        outcomes,
        vec![("applied".to_string(),)],
        "one event id is one row, however many times its outcome changed"
    );

    let succeeded_events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM payment_events WHERE payment_id = $1 AND event_type = 'PaymentSucceeded'",
    )
    .bind(order.id)
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(succeeded_events, 1);

    let notifications: i64 =
        sqlx::query_scalar("SELECT count(*) FROM notifications WHERE payment_id = $1")
            .bind(order.id)
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(
        notifications, 1,
        "the merchant is owed exactly one notification"
    );
}

#[tokio::test]
async fn concurrent_redelivery_of_an_already_applied_event_still_settles_once() {
    let fx = common::start_postgres().await;
    let db = std::sync::Arc::new(fx.db);

    let order = repo::create_order(&db, new_order("ACME-REDELIVER-2", 1500))
        .await
        .unwrap();
    let attempt = repo::open_attempt(
        &db,
        common::MERCHANT_ID,
        order.id,
        ProviderCode::Ecpay,
        "ACME1",
    )
    .await
    .unwrap();

    let merchant = common::merchant_context("ACME-REDELIVER-2");
    let trade_no = attempt.provider_trade_no.clone();

    // Four copies of ONE callback — same provider_event_id — arriving at one
    // instant: `webhooks.feature`'s "Four copies of one callback arriving at
    // one instant settle it once", and the same shape `scaling.feature`
    // fires six times across three replicas. Whichever copy wins the
    // attempt's row lock settles it; every other one must find the row
    // already `applied` and replay it verbatim rather than re-decide —
    // proving Bug 1's fix (re-evaluate when nothing committed) did not
    // weaken the outcome that DOES commit something.
    let mut handles = Vec::new();
    for _ in 0..4 {
        let db = db.clone();
        let merchant = merchant.clone();
        let trade_no = trade_no.clone();
        handles.push(tokio::spawn(async move {
            let facts = common::paid_facts(&trade_no, "evt-redeliver-2", 1500);
            repo::apply_callback(
                &db,
                ProviderCode::Ecpay,
                &facts,
                &merchant,
                common::MERCHANT_HASH_KEY,
                common::MERCHANT_HASH_IV,
            )
            .await
        }));
    }

    let mut outcomes = Vec::new();
    for h in handles {
        outcomes.push(h.await.unwrap().expect("apply_callback").outcome);
    }
    assert!(
        outcomes.iter().all(|o| *o == CallbackOutcome::Applied),
        "every copy of the same event id reports the same, once-settled outcome: {outcomes:?}"
    );

    let (status,): (String,) = sqlx::query_as("SELECT status FROM payments WHERE id = $1")
        .bind(order.id)
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(status, "succeeded");

    let events: i64 =
        sqlx::query_scalar("SELECT count(*) FROM provider_events WHERE payment_id = $1")
            .bind(order.id)
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(
        events, 1,
        "one event id, one row, however many copies arrived"
    );

    let succeeded_events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM payment_events WHERE payment_id = $1 AND event_type = 'PaymentSucceeded'",
    )
    .bind(order.id)
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(succeeded_events, 1, "settled exactly once");

    let notifications: i64 =
        sqlx::query_scalar("SELECT count(*) FROM notifications WHERE payment_id = $1")
            .bind(order.id)
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(notifications, 1, "owed exactly one notification, not four");
}

#[tokio::test]
async fn a_simulated_callback_records_simulate_paid_without_settling_anything() {
    let fx = common::start_postgres().await;
    let db = &fx.db;
    let merchant = common::merchant_context("ACME-SIMULATE-1");

    let order = repo::create_order(db, new_order("ACME-SIMULATE-1", 1200))
        .await
        .unwrap();
    let attempt = repo::open_attempt(
        db,
        common::MERCHANT_ID,
        order.id,
        ProviderCode::Ecpay,
        "ACME1",
    )
    .await
    .unwrap();

    // `SimulatePaid=1` set independently of `RtnCode` — the exact case
    // `webhooks.feature`'s "SimulatePaid overrides even a genuine decline"
    // asserts. It must settle nothing, but the flag itself must still land
    // on the attempt's own row: the column existed
    // (`payment_attempts.simulate_paid`) but nothing ever wrote it (Bug 2).
    let mut facts = common::failed_facts(&attempt.provider_trade_no, "evt-sim-1", 1200, 10_100_058);
    facts.simulate_paid = true;

    let result = repo::apply_callback(
        db,
        ProviderCode::Ecpay,
        &facts,
        &merchant,
        common::MERCHANT_HASH_KEY,
        common::MERCHANT_HASH_IV,
    )
    .await
    .unwrap();
    assert_eq!(result.outcome, CallbackOutcome::NoOp);
    assert!(repo::should_acknowledge(result.outcome));

    let (status, attempt_status, simulate_paid): (String, String, bool) = sqlx::query_as(
        "SELECT p.status, a.status, a.simulate_paid \
           FROM payments p JOIN payment_attempts a ON a.payment_id = p.id \
          WHERE p.id = $1",
    )
    .bind(order.id)
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(status, "pending", "SimulatePaid settles nothing");
    assert_eq!(
        attempt_status, "redirected",
        "not moved to failed either, whatever RtnCode said underneath"
    );
    assert!(
        simulate_paid,
        "the flag itself must be recorded on the attempt"
    );

    let outcome_events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM payment_events WHERE payment_id = $1 \
          AND event_type IN ('PaymentSucceeded', 'PaymentAttemptFailed')",
    )
    .bind(order.id)
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(outcome_events, 0);

    let notifications: i64 =
        sqlx::query_scalar("SELECT count(*) FROM notifications WHERE payment_id = $1")
            .bind(order.id)
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(notifications, 0);
}

/// Settle `order_id`'s first attempt honestly, then hand back a SECOND
/// attempt of the same order recorded as `Duplicate` for `amount` — the
/// shared setup every refund-accounting test below needs, built the same way
/// `apply_callback_produces_every_outcome_and_deduplicates_a_redelivered_event`'s
/// own "Duplicate" case is.
async fn settle_then_duplicate(
    db: &paygate_infra::Db,
    merchant_trade_no: &str,
    amount: i64,
) -> (repo::PaymentRow, repo::OpenedAttempt) {
    let order = repo::create_order(db, new_order(merchant_trade_no, amount))
        .await
        .unwrap();
    let attempt_a = repo::open_attempt(
        db,
        common::MERCHANT_ID,
        order.id,
        ProviderCode::Ecpay,
        "ACME1",
    )
    .await
    .unwrap();
    let attempt_b = repo::open_attempt(
        db,
        common::MERCHANT_ID,
        order.id,
        ProviderCode::Ecpay,
        "ACME1",
    )
    .await
    .unwrap();
    let merchant = common::merchant_context(merchant_trade_no);

    let paid_a = common::paid_facts(&attempt_a.provider_trade_no, "evt-settle-a", amount);
    repo::apply_callback(
        db,
        ProviderCode::Ecpay,
        &paid_a,
        &merchant,
        common::MERCHANT_HASH_KEY,
        common::MERCHANT_HASH_IV,
    )
    .await
    .unwrap();

    let paid_b = common::paid_facts(&attempt_b.provider_trade_no, "evt-settle-b", amount);
    let duplicate = repo::apply_callback(
        db,
        ProviderCode::Ecpay,
        &paid_b,
        &merchant,
        common::MERCHANT_HASH_KEY,
        common::MERCHANT_HASH_IV,
    )
    .await
    .unwrap();
    assert_eq!(duplicate.outcome, CallbackOutcome::Duplicate);

    (order, attempt_b)
}

#[tokio::test]
async fn a_duplicate_refund_leaves_the_orders_own_accounting_untouched() {
    let fx = common::start_postgres().await;
    let db = &fx.db;
    let (order, dup_attempt) = settle_then_duplicate(db, "ACME-REFUND-DUP-1", 2500).await;
    let merchant = common::merchant_context("ACME-REFUND-DUP-1");

    let refund_id = Uuid::now_v7();
    let reservation = repo::create_refund(
        db,
        common::MERCHANT_ID,
        order.id,
        refund_id,
        2500,
        Some("duplicate"),
        Some(dup_attempt.attempt_id),
    )
    .await
    .expect("a duplicate refund must be accepted");
    assert_eq!(reservation.attempt_id, dup_attempt.attempt_id);

    // Reserving it must not touch the ORDER's own budget at all — unlike an
    // ordinary refund, there is nothing to roll back later because nothing
    // was ever reserved.
    let (status, amount_refunded): (String, i64) =
        sqlx::query_as("SELECT status, amount_refunded FROM payments WHERE id = $1")
            .bind(order.id)
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(status, "succeeded");
    assert_eq!(amount_refunded, 0);

    let completion = repo::complete_refund(
        db,
        order.id,
        refund_id,
        2500,
        repo::RefundOutcome::Succeeded {
            provider_refund_id: "prov-refund-dup-1".to_string(),
        },
        &merchant,
        common::MERCHANT_HASH_KEY,
        common::MERCHANT_HASH_IV,
    )
    .await
    .expect("complete_refund");

    // Still not moved, and the order is not closed as `refunded` — the
    // duplicate's own money settling has nothing to do with what the
    // merchant may still give back of the order itself.
    assert_eq!(completion.amount_refunded, 0);
    assert_eq!(
        completion.payment_status,
        paygate_domain::PaymentStatus::Succeeded
    );

    let (refund_status, refund_reason): (String, Option<String>) =
        sqlx::query_as("SELECT status, reason FROM refunds WHERE id = $1")
            .bind(refund_id)
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(refund_status, "succeeded");
    assert_eq!(refund_reason.as_deref(), Some("duplicate"));

    // One notification total — the original `PaymentSucceeded` from
    // `settle_then_duplicate`'s first, honest attempt. The duplicate's own
    // refund must add none of its own.
    let refund_notifications: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM notifications WHERE payment_id = $1 AND payload->>'RtnMsg' = 'refunded'",
    )
    .bind(order.id)
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(
        refund_notifications, 0,
        "a duplicate's own automatic refund is not reported to the merchant"
    );
}

#[tokio::test]
async fn a_merchant_initiated_full_refund_still_moves_the_orders_own_accounting() {
    let fx = common::start_postgres().await;
    let db = &fx.db;

    let order = repo::create_order(db, new_order("ACME-REFUND-FULL-1", 1500))
        .await
        .unwrap();
    let attempt = repo::open_attempt(
        db,
        common::MERCHANT_ID,
        order.id,
        ProviderCode::Ecpay,
        "ACME1",
    )
    .await
    .unwrap();
    let merchant = common::merchant_context("ACME-REFUND-FULL-1");
    let facts = common::paid_facts(&attempt.provider_trade_no, "evt-settle", 1500);
    repo::apply_callback(
        db,
        ProviderCode::Ecpay,
        &facts,
        &merchant,
        common::MERCHANT_HASH_KEY,
        common::MERCHANT_HASH_IV,
    )
    .await
    .unwrap();

    let refund_id = Uuid::now_v7();
    // `reason: None` here — an ordinary, merchant-initiated refund, exactly
    // what `POST /payments/{id}/refunds` reserves. This IS bounded by, and
    // DOES move, the order's own `amount_refunded` — the regression this
    // test guards against is treating every refund like a duplicate's.
    repo::create_refund(
        db,
        common::MERCHANT_ID,
        order.id,
        refund_id,
        1500,
        None,
        None,
    )
    .await
    .expect("a full, ordinary refund must be accepted");

    let reserved: i64 = sqlx::query_scalar("SELECT amount_refunded FROM payments WHERE id = $1")
        .bind(order.id)
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(reserved, 1500, "the reservation must move amount_refunded");

    let completion = repo::complete_refund(
        db,
        order.id,
        refund_id,
        1500,
        repo::RefundOutcome::Succeeded {
            provider_refund_id: "prov-refund-full-1".to_string(),
        },
        &merchant,
        common::MERCHANT_HASH_KEY,
        common::MERCHANT_HASH_IV,
    )
    .await
    .expect("complete_refund");

    assert_eq!(completion.amount_refunded, 1500);
    assert_eq!(
        completion.payment_status,
        paygate_domain::PaymentStatus::Refunded,
        "fully refunding the order's own money must close it"
    );

    // Two notifications total — the order's own `PaymentSucceeded` plus this
    // refund's — but only one of them is the refund telling the merchant its
    // money came back.
    let refund_notifications: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM notifications WHERE payment_id = $1 AND payload->>'RtnMsg' = 'refunded'",
    )
    .bind(order.id)
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(
        refund_notifications, 1,
        "a merchant-initiated refund IS reported to the merchant"
    );
}

#[tokio::test]
async fn a_duplicate_refund_still_settles_once_the_order_is_already_fully_refunded() {
    let fx = common::start_postgres().await;
    let db = &fx.db;
    let (order, dup_attempt) = settle_then_duplicate(db, "ACME-REFUND-DUP-2", 1000).await;
    let merchant = common::merchant_context("ACME-REFUND-DUP-2");

    // The merchant fully refunds the order itself FIRST — of its own money,
    // nothing to do with the duplicate attempt at all.
    let merchant_refund_id = Uuid::now_v7();
    repo::create_refund(
        db,
        common::MERCHANT_ID,
        order.id,
        merchant_refund_id,
        1000,
        None,
        None,
    )
    .await
    .unwrap();
    repo::complete_refund(
        db,
        order.id,
        merchant_refund_id,
        1000,
        repo::RefundOutcome::Succeeded {
            provider_refund_id: "prov-refund-merchant".to_string(),
        },
        &merchant,
        common::MERCHANT_HASH_KEY,
        common::MERCHANT_HASH_IV,
    )
    .await
    .unwrap();
    let (status_after_merchant_refund,): (String,) =
        sqlx::query_as("SELECT status FROM payments WHERE id = $1")
            .bind(order.id)
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(status_after_merchant_refund, "refunded");

    // NOW the duplicate's own automatic refund is attempted, against an
    // order that is `refunded`, not `succeeded` — and with a `remaining`
    // budget of exactly 0 by the ordinary `check_refund` arithmetic. Decided
    // above: a duplicate refund is bounded by neither. It must still be
    // accepted, still call the provider, still leave a real `refunds` row —
    // and still leave the order's own accounting exactly where the
    // merchant's own refund left it.
    let dup_refund_id = Uuid::now_v7();
    let reservation = repo::create_refund(
        db,
        common::MERCHANT_ID,
        order.id,
        dup_refund_id,
        1000,
        Some("duplicate"),
        Some(dup_attempt.attempt_id),
    )
    .await
    .expect("a duplicate refund must be accepted even once the order is fully refunded");
    assert_eq!(reservation.attempt_id, dup_attempt.attempt_id);

    let (status_after_reservation, amount_refunded_after_reservation): (String, i64) =
        sqlx::query_as("SELECT status, amount_refunded FROM payments WHERE id = $1")
            .bind(order.id)
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(status_after_reservation, "refunded");
    assert_eq!(amount_refunded_after_reservation, 1000);

    let completion = repo::complete_refund(
        db,
        order.id,
        dup_refund_id,
        1000,
        repo::RefundOutcome::Succeeded {
            provider_refund_id: "prov-refund-dup-2".to_string(),
        },
        &merchant,
        common::MERCHANT_HASH_KEY,
        common::MERCHANT_HASH_IV,
    )
    .await
    .expect("complete_refund");
    assert_eq!(
        completion.amount_refunded, 1000,
        "still exactly the merchant's own refund"
    );
    assert_eq!(
        completion.payment_status,
        paygate_domain::PaymentStatus::Refunded
    );

    let refund_rows: i64 = sqlx::query_scalar("SELECT count(*) FROM refunds WHERE payment_id = $1")
        .bind(order.id)
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(
        refund_rows, 2,
        "the merchant's own refund AND the duplicate's, both real rows"
    );

    let duplicate_refund_status: String =
        sqlx::query_scalar("SELECT status FROM refunds WHERE id = $1")
            .bind(dup_refund_id)
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(duplicate_refund_status, "succeeded");
}

/// The ABBA lock-order deadlock this test guards against: `create_refund`
/// (`repo::refunds`) takes `payments` `FOR UPDATE` and then, to find the
/// attempt it is refunding, `payment_attempts` `FOR UPDATE` — parent before
/// child. Before this fix, `apply_callback` (`repo::callbacks`) took the
/// SAME two locks in the opposite order: it found the attempt by its
/// provider identity first, then locked the payment its `payment_id`
/// pointed at. A merchant-initiated refund and an inbound provider callback
/// for the SAME payment, racing each other, could each hold one row and
/// wait for the other — PostgreSQL's own `40P01 deadlock detected`, a
/// random `500` on the refund or a callback the provider resends forever
/// believing it was never received.
///
/// This drives the two paths against one payment, many times over
/// (`ITERATIONS`), naming the exact same attempt on both sides so that both
/// transactions really do contend for the same two rows — a refund of a
/// different attempt, or a callback for a different payment, would only
/// serialise, never deadlock. Run against the code before the fix, this
/// reproduces `40P01` within the first few iterations; after the fix,
/// neither path should ever see one, and the payment's own accounting must
/// still be exactly what it should be once every iteration has landed.
#[tokio::test]
async fn concurrent_refund_and_callback_on_the_same_payment_never_deadlock() {
    const ITERATIONS: usize = 40;

    let fx = common::start_postgres().await;
    let db = std::sync::Arc::new(fx.db);

    let order = repo::create_order(&db, new_order("ACME-LOCK-ORDER-1", 10_000))
        .await
        .expect("create order");
    let attempt = repo::open_attempt(
        &db,
        common::MERCHANT_ID,
        order.id,
        ProviderCode::Ecpay,
        "ACME1",
    )
    .await
    .expect("open attempt");
    let merchant = common::merchant_context("ACME-LOCK-ORDER-1");

    // Settle it first, so `create_refund` has a succeeded attempt to find
    // and the order is in a refundable status.
    let settle_facts = common::paid_facts(&attempt.provider_trade_no, "evt-settle", 10_000);
    repo::apply_callback(
        &db,
        ProviderCode::Ecpay,
        &settle_facts,
        &merchant,
        common::MERCHANT_HASH_KEY,
        common::MERCHANT_HASH_IV,
    )
    .await
    .expect("settle the order so it can be refunded");

    let mut deadlocks = 0;

    for i in 0..ITERATIONS {
        // The refund path: `reason = "duplicate"` against this same attempt
        // so repeated iterations never exhaust `amount - amount_refunded`
        // (a duplicate's own refund is not bounded by it — see
        // `repo::refunds`'s own doc comment) — this test is about lock
        // order, not the refund budget.
        let db_refund = db.clone();
        let payment_id = order.id;
        let attempt_id = attempt.attempt_id;
        let refund_handle = tokio::spawn(async move {
            repo::create_refund(
                &db_refund,
                common::MERCHANT_ID,
                payment_id,
                Uuid::now_v7(),
                1,
                Some("duplicate"),
                Some(attempt_id),
            )
            .await
        });

        // The callback path: a fresh `provider_event_id` every iteration (so
        // it is never deduplicated away before taking any lock at all), but
        // the SAME attempt's `provider_trade_no` — the attempt is already
        // `succeeded`, so this always decides `ReplayOfResolvedAttempt`
        // (`NoOp`), but deciding that still requires taking both locks.
        let db_cb = db.clone();
        let merchant_cb = merchant.clone();
        let trade_no = attempt.provider_trade_no.clone();
        let event_id = format!("evt-race-{i}");
        let callback_handle = tokio::spawn(async move {
            let facts = common::paid_facts(&trade_no, &event_id, 10_000);
            repo::apply_callback(
                &db_cb,
                ProviderCode::Ecpay,
                &facts,
                &merchant_cb,
                common::MERCHANT_HASH_KEY,
                common::MERCHANT_HASH_IV,
            )
            .await
        });

        let refund_result = refund_handle.await.unwrap();
        let callback_result = callback_handle.await.unwrap();

        fn is_deadlock(err: &paygate_infra::Error) -> bool {
            matches!(
                err,
                paygate_infra::Error::Database(sqlx::Error::Database(db_err))
                    if db_err.code().as_deref() == Some("40P01")
            )
        }

        match &refund_result {
            Err(err) if is_deadlock(err) => {
                deadlocks += 1;
                eprintln!("iteration {i}: refund hit 40P01 deadlock detected");
            }
            _ => assert!(
                refund_result.is_ok(),
                "iteration {i}: refund failed with an error other than a deadlock: {refund_result:?}"
            ),
        }
        match &callback_result {
            Err(err) if is_deadlock(err) => {
                deadlocks += 1;
                eprintln!("iteration {i}: callback hit 40P01 deadlock detected");
            }
            _ => assert!(
                callback_result.is_ok(),
                "iteration {i}: callback failed with an error other than a deadlock: {callback_result:?}"
            ),
        }
    }

    assert_eq!(
        deadlocks, 0,
        "no refund/callback pair should ever deadlock against the other"
    );

    // Correctness, not just the absence of a deadlock: every iteration's
    // refund really landed (a real `refunds` row each), and the callback
    // side never mutated anything it should not have (the order stays
    // `succeeded`, `amount_refunded` stays untouched — every refund here is
    // the duplicate's own, which never moves it).
    let refund_rows: i64 = sqlx::query_scalar("SELECT count(*) FROM refunds WHERE payment_id = $1")
        .bind(order.id)
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(refund_rows, ITERATIONS as i64);

    let (status, amount_refunded): (String, i64) =
        sqlx::query_as("SELECT status, amount_refunded FROM payments WHERE id = $1")
            .bind(order.id)
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(status, "succeeded");
    assert_eq!(
        amount_refunded, 0,
        "every refund here was the duplicate's own; the order's own budget is untouched"
    );

    let succeeded_events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM payment_events WHERE payment_id = $1 AND event_type = 'PaymentSucceeded'",
    )
    .bind(order.id)
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(
        succeeded_events, 1,
        "the original settlement, and nothing re-settled it"
    );
}
