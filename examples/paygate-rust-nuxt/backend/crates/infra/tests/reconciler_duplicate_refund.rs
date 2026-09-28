//! Proves the fix for the reconciler's own duplicate-payment bug: a
//! `Duplicate` outcome found by `settle_from_query` — the reconciler's OWN
//! query path, never a callback — must leave a `pending` refund behind for
//! `reconciler::resend_pending_refunds` to deliver, exactly like the
//! callback path's own automatic refund (`crates/api/src/routes/webhooks.rs`,
//! ~line 144). `spec.md`, "When the customer pays twice": "the money is
//! refunded automatically" has to hold for this path too — one
//! `duplicate_payment.feature` never exercises, since it only ever drives
//! the callback.
//!
//! Also proves the rule `repo::refunds`'s own module doc states and
//! `repo::create_refund`'s `reason = "duplicate"` path already keeps: a
//! duplicate's own refund must never move `payments.amount_refunded` and
//! must never change `payments.status` — that second charge was never the
//! order's own money.

mod common;

use paygate_domain::ProviderCode;
use paygate_infra::repo::{self, ReconcileOutcome};

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
async fn a_duplicate_found_by_the_query_leaves_a_pending_refund_behind() {
    let fx = common::start_postgres().await;
    let db = &fx.db;
    let merchant = common::merchant_context("ACME-RECON-DUP-1");

    let order = repo::create_order(db, new_order("ACME-RECON-DUP-1", 2500))
        .await
        .expect("create order");
    let attempt_a = repo::open_attempt(
        db,
        common::MERCHANT_ID,
        order.id,
        ProviderCode::Ecpay,
        "ACME1",
    )
    .await
    .expect("open attempt a");
    let attempt_b = repo::open_attempt(
        db,
        common::MERCHANT_ID,
        order.id,
        ProviderCode::Ecpay,
        "ACME1",
    )
    .await
    .expect("open attempt b");

    // Attempt A settles the order honestly, through the ordinary callback
    // path — the same fixture shape `postgres_repo.rs`'s own
    // `settle_then_duplicate` uses.
    let paid_a = common::paid_facts(&attempt_a.provider_trade_no, "evt-recon-dup-a", 2500);
    repo::apply_callback(
        db,
        ProviderCode::Ecpay,
        &paid_a,
        &merchant,
        common::MERCHANT_HASH_KEY,
        common::MERCHANT_HASH_IV,
    )
    .await
    .expect("attempt a settles the order");

    // Attempt B is the second, unwanted charge — found this time by ASKING
    // the provider (`settle_from_query`), the reconciler's own path, never a
    // callback. `run_pass` holds one open transaction across the whole
    // round trip to the provider and hands it to `settle_from_query`
    // directly; this test does the same, by hand.
    let paid_b = common::paid_facts(&attempt_b.provider_trade_no, "evt-recon-dup-b", 2500);
    let mut tx = db.begin().await.expect("begin");
    let application = repo::settle_from_query(
        &mut tx,
        attempt_b.attempt_id,
        &paid_b,
        "raw provider answer",
        &merchant,
        common::MERCHANT_HASH_KEY,
        common::MERCHANT_HASH_IV,
    )
    .await
    .expect("settle_from_query");
    assert_eq!(
        application.outcome,
        ReconcileOutcome::Duplicate,
        "the order already settled through attempt a; attempt b's own paid answer must be a duplicate"
    );
    let duplicate_amount = application
        .duplicate_amount
        .expect("a Duplicate outcome always carries the amount that was charged twice");
    assert_eq!(duplicate_amount, 2500);

    // The fix under test: `reconciler::run_pass`, on exactly this outcome,
    // calls `create_duplicate_refund` in the SAME still-open transaction —
    // never a fresh `repo::create_refund` (`&Db`), which would try to
    // re-lock the very `payments`/`payment_attempts` rows this transaction
    // is still holding and block on itself for the rest of the pass.
    let refund_id = repo::create_duplicate_refund(
        &mut tx,
        common::MERCHANT_ID,
        application.payment_id,
        application.attempt_id,
        duplicate_amount,
    )
    .await
    .expect("create_duplicate_refund");
    tx.commit().await.expect("commit");

    let (refund_status, refund_reason, refund_amount, refund_attempt_id): (
        String,
        Option<String>,
        i64,
        uuid::Uuid,
    ) = sqlx::query_as("SELECT status, reason, amount, attempt_id FROM refunds WHERE id = $1")
        .bind(refund_id)
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(
        refund_status, "pending",
        "left pending for resend_pending_refunds to deliver, this pass or the next"
    );
    assert_eq!(refund_reason.as_deref(), Some("duplicate"));
    assert_eq!(refund_amount, 2500);
    assert_eq!(
        refund_attempt_id, attempt_b.attempt_id,
        "the duplicate's own refund must target the DUPLICATED attempt, never the one that \
         actually settled the order"
    );

    // The order's own accounting must be completely untouched: a duplicate's
    // charge was never this order's own money (`repo::refunds`'s own module
    // doc; `repo/refunds.rs` is what actually enforces this on `create_refund`
    // and `complete_refund` for `reason = "duplicate"` — this INSERT alone
    // never touches either column in the first place).
    let (payment_status, amount_refunded): (String, i64) =
        sqlx::query_as("SELECT status, amount_refunded FROM payments WHERE id = $1")
            .bind(order.id)
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(
        payment_status, "succeeded",
        "a duplicate's own pending refund must not change the order's status"
    );
    assert_eq!(
        amount_refunded, 0,
        "a duplicate's own pending refund must not move amount_refunded"
    );
}
