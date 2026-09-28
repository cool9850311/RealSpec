//! Reproduces, then proves fixed, the SECOND half of the reconciler's own
//! ABBA lock-order deadlock — the CLAIM itself
//! (`reconciler::select_candidates`), not the `settle_from_query`/
//! `abandon_attempt` reorder proven by `reconciler_refund_lock_order.rs`.
//!
//! Even with `settle_from_query`/`abandon_attempt` taking the canonical
//! (`payments`, then `payment_attempts`) lock order internally,
//! `run_pass`'s real transaction acquires whichever row the CLAIM locks
//! FIRST — and the claim used to lock `payment_attempts`
//! (`FOR UPDATE OF a SKIP LOCKED`) before `settle_from_query` is ever
//! reached, fixing this whole transaction's actual lock-acquisition order to
//! attempt-then-payment regardless of what `settle_from_query` does
//! internally. `run_pass` then holds that transaction open across an entire
//! round trip to the provider before ever calling `settle_from_query`, so
//! the window in which the wrong-order lock is held is however long that
//! call takes — a short `sleep` stands in for it here, since driving a real
//! provider round trip is not this crate's concern.
//!
//! This test drives the claim itself (`select_candidate_attempt_ids`, kept
//! on the SAME transaction afterward, exactly `run_pass`'s own shape)
//! against a concurrent automatic duplicate-payment refund
//! (`repo::create_refund`, `reason = "duplicate"`, `attempt_id: Some(...)`)
//! targeting the SAME attempt the claim selected — the realistic shape of
//! `T2` in the lock-order finding this test is named for: a callback that
//! (re)discovers the very attempt the reconciler is mid-query on as a
//! duplicate, and refunds it.

mod common;

use std::time::Duration;

use paygate_domain::ProviderCode;
use paygate_infra::{reconciler, repo};
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
async fn a_reconciliation_claim_and_a_duplicate_refund_never_deadlock_on_the_same_attempt() {
    let fx = common::start_postgres().await;
    // A bigger, dedicated pool — see `reconciler_refund_lock_order.rs`'s own
    // note on why `common::start_postgres`'s pool (sized for mostly
    // sequential tests) would otherwise turn a starved pool acquire into a
    // false deadlock report.
    let host = fx._container.get_host().await.expect("host");
    let port = fx
        ._container
        .get_host_port_ipv4(5432)
        .await
        .expect("mapped port");
    let dsn = format!("postgres://postgres:postgres@{host}:{port}/postgres");
    let db = std::sync::Arc::new(
        paygate_infra::Db::connect(&dsn, 128)
            .await
            .expect("connect with a larger pool for the concurrent phase"),
    );

    const PAIRS: usize = 20;
    // (payment id, the duplicate attempt's own id) for each independent pair
    // — independent payments, so the claim's own `SKIP LOCKED` genuinely has
    // to spread concurrent claimants across them rather than serialising on
    // one row, the same way real, unrelated in-flight orders would.
    let mut pairs = Vec::with_capacity(PAIRS);

    for i in 0..PAIRS {
        let trade_no = format!("ACME-CLAIM-LOCK-{i}");
        let merchant = common::merchant_context(&trade_no);
        let order = repo::create_order(&db, new_order(&trade_no, 1000))
            .await
            .expect("create order");
        let attempt_settle =
            repo::open_attempt(&db, common::MERCHANT_ID, order.id, ProviderCode::Ecpay, "S")
                .await
                .expect("open the settling attempt");
        let attempt_dup =
            repo::open_attempt(&db, common::MERCHANT_ID, order.id, ProviderCode::Ecpay, "D")
                .await
                .expect("open the duplicate attempt");

        // Settle the order through the FIRST attempt, honestly, via the
        // ordinary callback path.
        let paid = common::paid_facts(
            &attempt_settle.provider_trade_no,
            &format!("evt-claim-settle-{i}"),
            1000,
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
        .expect("settle the order through the first attempt");

        // Age the SECOND attempt past the selection window, so the claim
        // picks it up — still `redirected`, the provider never having been
        // asked about it yet.
        sqlx::query(
            "UPDATE payment_attempts SET started_at = now() - interval '61 minutes' \
             WHERE id = $1",
        )
        .bind(attempt_dup.attempt_id)
        .execute(db.pool())
        .await
        .unwrap();

        pairs.push((order.id, attempt_dup.attempt_id));
    }

    let after = Duration::from_secs(3600);
    let retry = Duration::from_secs(3600);
    let mut handles = Vec::new();

    // T1-style tasks: the real claim, on an otherwise-open transaction
    // (`run_pass`'s own shape), a short sleep standing in for the provider
    // round trip, then `settle_from_query` on whatever it claimed. One task
    // per pair, `batch_size = 1` each — `SKIP LOCKED` spreads them across
    // the `PAIRS` distinct duplicate attempts.
    for i in 0..PAIRS {
        let db = db.clone();
        handles.push(tokio::spawn(async move {
            let mut tx = db.begin().await?;
            let claimed =
                reconciler::select_candidate_attempt_ids(&mut tx, after, retry, 1).await?;
            tokio::time::sleep(Duration::from_millis(60)).await;
            let merchant = common::merchant_context("ACME-CLAIM-LOCK");
            for attempt_id in claimed {
                let facts = common::paid_facts("whatever", &format!("evt-claim-requery-{i}"), 1000);
                repo::settle_from_query(
                    &mut tx,
                    attempt_id,
                    &facts,
                    "raw provider answer",
                    &merchant,
                    common::MERCHANT_HASH_KEY,
                    common::MERCHANT_HASH_IV,
                )
                .await?;
            }
            tx.commit().await.map_err(paygate_infra::Error::from)
        }));
    }

    // T2: the automatic duplicate refund, targeting the SAME attempt the
    // claim above will try to lock — `reason = "duplicate"`,
    // `attempt_id: Some(...)`, the one `create_refund` shape that locks a
    // SPECIFIC attempt without requiring the order to be fully resolved
    // first (`refundable_status` only asks that the order is not still
    // `pending`, which it is not — attempt A already settled it).
    for (payment_id, attempt_id) in pairs {
        let db = db.clone();
        handles.push(tokio::spawn(async move {
            repo::create_refund(
                &db,
                common::MERCHANT_ID,
                payment_id,
                Uuid::now_v7(),
                1,
                Some("duplicate"),
                Some(attempt_id),
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
            // Any other error is an ordinary business/race outcome (a
            // `NoOp` racing a `Duplicate`, `PaymentNotFound` if a race
            // resolved things in an unexpected order, etc.) — not what this
            // test is about.
        }
    }
    assert!(
        deadlocks.is_empty(),
        "the reconciler's own claim and an automatic duplicate refund must never deadlock on the \
         same attempt, but got: {deadlocks:#?}"
    );
}
