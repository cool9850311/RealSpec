//! Reproduces, then proves fixed, an ABBA lock-order deadlock between the
//! reconciler's own query path (`repo::settle_from_query`,
//! `repo::abandon_attempt`) and an ordinary merchant refund
//! (`repo::create_refund`) racing on the SAME payment and the SAME attempt.
//!
//! `repo::create_refund` (`repo/refunds.rs`, owned by a different agent
//! fixing the matching inversion in `repo/callbacks.rs`) already takes the
//! canonical lock order this workspace settled on: `payments` locked first,
//! `payment_attempts` second. `settle_from_query`/`abandon_attempt` used to
//! take the REVERSE order — `payment_attempts` first, `payments` second —
//! which is exactly the shape of a classic two-resource deadlock once both
//! sides contend for the same two rows: one transaction holding the attempt
//! lock and waiting on the payment lock, the other holding the payment lock
//! and waiting on that very same attempt lock.
//!
//! The reconciler makes this far likelier than the callback-vs-refund case
//! the other agent found: `reconciler::run_pass` holds ONE open transaction
//! across an entire round trip to the provider per attempt in its batch, so
//! the window in which the wrong-order lock is held is however long that
//! call takes (up to `RECONCILE_HTTP_TIMEOUT_MS`), not a couple of local
//! queries.
//!
//! Many concurrent pairs are driven against ONE shared payment/attempt
//! (rather than one pair per payment) to make the two lock orders collide as
//! often as possible within a single test run — this test is not
//! deterministic in WHICH pair deadlocks, only that, before the fix, at
//! least one reliably does.

mod common;

use paygate_domain::ProviderCode;
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
async fn a_reconciliation_requery_and_a_merchant_refund_never_deadlock_on_the_same_attempt() {
    let fx = common::start_postgres().await;
    // `common::start_postgres`'s own pool (`pool_max = 10`) is sized for the
    // crate's other, mostly-sequential integration tests; this test needs
    // enough connections for many pairs of tasks to be genuinely in flight
    // at once, or a starved pool acquire (not a deadlock at all) would be
    // mistaken for one. Same container, a bigger pool.
    let host = fx._container.get_host().await.expect("host");
    let port = fx
        ._container
        .get_host_port_ipv4(5432)
        .await
        .expect("mapped port");
    let dsn = format!("postgres://postgres:postgres@{host}:{port}/postgres");
    let db = std::sync::Arc::new(
        paygate_infra::Db::connect(&dsn, 64)
            .await
            .expect("connect with a larger pool for the concurrent phase"),
    );
    let merchant = common::merchant_context("ACME-LOCK-ORDER-1");

    let order = repo::create_order(&db, new_order("ACME-LOCK-ORDER-1", 1_000_000))
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

    // Settle the order through the ordinary callback path first, so a
    // merchant refund against it is legal, and so `settle_from_query`
    // re-asking about the SAME attempt lands as `NoOp` (a late or
    // redelivered query answer for an attempt a callback already settled —
    // `reconcile.feature`'s own "A payment that already settled is never
    // asked about" is the ordinary case of this; nothing stops the provider
    // answering a query in flight the instant after a callback wins the
    // race, which is the realistic shape of this exact collision).
    let paid = common::paid_facts(
        &attempt.provider_trade_no,
        "evt-lock-order-settle",
        1_000_000,
    );
    repo::apply_callback(
        &db,
        ProviderCode::Ecpay,
        &paid,
        &merchant,
        common::MERCHANT_HASH_KEY,
        common::MERCHANT_HASH_IV,
    )
    .await
    .expect("settle the order so a merchant refund becomes legal");

    const PAIRS: usize = 30;
    let mut handles = Vec::new();

    for i in 0..PAIRS {
        // The reconciliation side: re-asks about the SAME attempt.
        // `settle_from_query` used to lock `payment_attempts` first, then
        // `payments` — the reverse of `create_refund`, below.
        let db_a = db.clone();
        let attempt_a = attempt.clone();
        let merchant_a = merchant.clone();
        handles.push(tokio::spawn(async move {
            let facts = common::paid_facts(
                &attempt_a.provider_trade_no,
                &format!("evt-lock-order-requery-{i}"),
                1_000_000,
            );
            let mut tx = db_a.begin().await.expect("begin");
            match repo::settle_from_query(
                &mut tx,
                attempt_a.attempt_id,
                &facts,
                "raw provider answer",
                &merchant_a,
                common::MERCHANT_HASH_KEY,
                common::MERCHANT_HASH_IV,
            )
            .await
            {
                Ok(_) => tx.commit().await.map_err(paygate_infra::Error::from),
                Err(e) => Err(e),
            }
        }));

        // The merchant's own ordinary refund: locks `payments` first
        // (`repo::create_refund`'s own, already-canonical order), then the
        // SAME succeeded attempt (`attempt_id: None` resolves to "the
        // attempt that actually settled the order" — there is only one).
        let db_b = db.clone();
        let payment_id = order.id;
        handles.push(tokio::spawn(async move {
            repo::create_refund(
                &db_b,
                common::MERCHANT_ID,
                payment_id,
                Uuid::now_v7(),
                1,
                None,
                None,
            )
            .await
            .map(|_| ())
        }));
    }

    let mut deadlocks = Vec::new();
    for handle in handles {
        if let Err(err) = handle.await.expect("task panicked") {
            let message = err.to_string();
            if message.to_lowercase().contains("deadlock") {
                deadlocks.push(message);
            }
            // Any other error (`RefundExceedsRemaining` once the concurrent
            // 1-unit refunds add up, a `NoOp`'s own bookkeeping racing
            // itself, etc.) is an ordinary business outcome, not what this
            // test is about.
        }
    }
    assert!(
        deadlocks.is_empty(),
        "the reconciler's query path and an ordinary merchant refund must never deadlock on the \
         same payment/attempt pair, but got: {deadlocks:#?}"
    );
}
