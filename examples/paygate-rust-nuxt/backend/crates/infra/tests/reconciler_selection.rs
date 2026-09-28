//! The reconciler's selection query, against a real PostgreSQL: only
//! `redirected`, only older than the window, only when `queried_at` is null
//! or older than the retry interval, batch-limited.

mod common;

use std::time::Duration;

use paygate_domain::ProviderCode;
use paygate_infra::{reconciler, repo};
use uuid::Uuid;

fn new_order(merchant_trade_no: &str) -> repo::NewOrder {
    repo::NewOrder {
        merchant_id: common::MERCHANT_ID,
        merchant_trade_no: merchant_trade_no.to_string(),
        amount: 1000,
        currency: "USD".to_string(),
        item_desc: "Beans".to_string(),
        notify_url: "/demo-merchant/api/notify".to_string(),
        client_back_url: "/shop/result".to_string(),
    }
}

async fn age_attempt(db: &paygate_infra::Db, attempt_id: Uuid, started_ago_secs: i64) {
    sqlx::query(
        "UPDATE payment_attempts SET started_at = now() - make_interval(secs => $1) WHERE id = $2",
    )
    .bind(started_ago_secs as f64)
    .bind(attempt_id)
    .execute(db.pool())
    .await
    .unwrap();
}

async fn set_queried_ago(db: &paygate_infra::Db, attempt_id: Uuid, queried_ago_secs: Option<i64>) {
    match queried_ago_secs {
        Some(secs) => {
            sqlx::query(
                "UPDATE payment_attempts SET queried_at = now() - make_interval(secs => $1) WHERE id = $2",
            )
            .bind(secs as f64)
            .bind(attempt_id)
            .execute(db.pool())
            .await
            .unwrap();
        }
        None => {
            sqlx::query("UPDATE payment_attempts SET queried_at = NULL WHERE id = $1")
                .bind(attempt_id)
                .execute(db.pool())
                .await
                .unwrap();
        }
    }
}

#[tokio::test]
async fn the_selection_query_applies_every_documented_filter() {
    let fx = common::start_postgres().await;
    let db = &fx.db;
    let after = Duration::from_secs(3600); // RECONCILE_AFTER_MINUTES = 60
    let retry = Duration::from_secs(3600); // RECONCILE_RETRY_MINUTES = 60

    // A: too young — started 30 minutes ago, well inside the window.
    let order_a = repo::create_order(db, new_order("SEL-A")).await.unwrap();
    let attempt_a = repo::open_attempt(
        db,
        common::MERCHANT_ID,
        order_a.id,
        ProviderCode::Ecpay,
        "A",
    )
    .await
    .unwrap();
    age_attempt(db, attempt_a.attempt_id, 30 * 60).await;

    // B: old enough, never queried — eligible.
    let order_b = repo::create_order(db, new_order("SEL-B")).await.unwrap();
    let attempt_b = repo::open_attempt(
        db,
        common::MERCHANT_ID,
        order_b.id,
        ProviderCode::Ecpay,
        "B",
    )
    .await
    .unwrap();
    age_attempt(db, attempt_b.attempt_id, 61 * 60).await;

    // C: old enough, but queried recently — still inside the retry window.
    let order_c = repo::create_order(db, new_order("SEL-C")).await.unwrap();
    let attempt_c = repo::open_attempt(
        db,
        common::MERCHANT_ID,
        order_c.id,
        ProviderCode::Ecpay,
        "C",
    )
    .await
    .unwrap();
    age_attempt(db, attempt_c.attempt_id, 61 * 60).await;
    set_queried_ago(db, attempt_c.attempt_id, Some(5 * 60)).await;

    // D: old enough, queried long enough ago — eligible again.
    let order_d = repo::create_order(db, new_order("SEL-D")).await.unwrap();
    let attempt_d = repo::open_attempt(
        db,
        common::MERCHANT_ID,
        order_d.id,
        ProviderCode::Ecpay,
        "D",
    )
    .await
    .unwrap();
    age_attempt(db, attempt_d.attempt_id, 61 * 60).await;
    set_queried_ago(db, attempt_d.attempt_id, Some(61 * 60)).await;

    // E: old enough, never queried, but already succeeded — never selected,
    // whatever its age, because it is not `redirected` any more.
    let order_e = repo::create_order(db, new_order("SEL-E")).await.unwrap();
    let attempt_e = repo::open_attempt(
        db,
        common::MERCHANT_ID,
        order_e.id,
        ProviderCode::Ecpay,
        "E",
    )
    .await
    .unwrap();
    age_attempt(db, attempt_e.attempt_id, 61 * 60).await;
    let facts = common::paid_facts(&attempt_e.provider_trade_no, "evt-e", 1000);
    let merchant = common::merchant_context("SEL-E");
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

    let mut tx = db.begin().await.unwrap();
    let selected = reconciler::select_candidate_attempt_ids(&mut tx, after, retry, 50)
        .await
        .unwrap();
    tx.rollback().await.unwrap();

    let selected: std::collections::HashSet<Uuid> = selected.into_iter().collect();
    assert!(
        !selected.contains(&attempt_a.attempt_id),
        "too young must not be selected"
    );
    assert!(
        selected.contains(&attempt_b.attempt_id),
        "old and never queried must be selected"
    );
    assert!(
        !selected.contains(&attempt_c.attempt_id),
        "queried too recently must not be selected"
    );
    assert!(
        selected.contains(&attempt_d.attempt_id),
        "queried long enough ago must be selected again"
    );
    assert!(
        !selected.contains(&attempt_e.attempt_id),
        "an already-settled attempt must never be selected"
    );
}

#[tokio::test]
async fn the_selection_query_is_batch_limited() {
    let fx = common::start_postgres().await;
    let db = &fx.db;
    let after = Duration::from_secs(60);
    let retry = Duration::from_secs(60);

    for i in 0..5 {
        let order = repo::create_order(db, new_order(&format!("SEL-BATCH-{i}")))
            .await
            .unwrap();
        let attempt =
            repo::open_attempt(db, common::MERCHANT_ID, order.id, ProviderCode::Ecpay, "B")
                .await
                .unwrap();
        age_attempt(db, attempt.attempt_id, 3600).await;
    }

    let mut tx = db.begin().await.unwrap();
    let selected = reconciler::select_candidate_attempt_ids(&mut tx, after, retry, 3)
        .await
        .unwrap();
    tx.rollback().await.unwrap();

    assert_eq!(
        selected.len(),
        3,
        "the batch size must cap how many are selected at once"
    );
}
