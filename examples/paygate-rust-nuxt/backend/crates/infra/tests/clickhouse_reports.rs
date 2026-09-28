//! `ClickHouseClient` against a real ClickHouse, started by
//! `testcontainers`: `FINAL` deduplicating a row inserted twice, and a
//! merchant's own timezone shifting which day an event falls on.

mod common;

use chrono::{NaiveDate, TimeZone, Utc};
use paygate_infra::clickhouse::ReportEventRow;
use uuid::Uuid;

fn row(event_id: Uuid, occurred_at: chrono::DateTime<Utc>, amount: i64) -> ReportEventRow {
    ReportEventRow {
        event_id,
        seq: 2,
        merchant_id: 1,
        payment_id: Uuid::now_v7(),
        event_type: "PaymentSucceeded".to_string(),
        provider_code: "ecpay".to_string(),
        amount,
        currency: "USD".to_string(),
        card_brand: "visa".to_string(),
        failure_code: String::new(),
        reference: "ACME-CH-1".to_string(),
        refund_id: Uuid::nil(),
        occurred_at,
        ingester_id: "ing-test".to_string(),
    }
}

#[tokio::test]
async fn an_event_inserted_twice_is_deduplicated_by_final() {
    let fx = common::start_clickhouse().await;
    let client = &fx.client;

    let event_id = Uuid::now_v7();
    let occurred_at = Utc::now();

    client
        .insert_events(&[row(event_id, occurred_at, 4200)])
        .await
        .unwrap();
    // A relay retry: the same event_id republished (`reports.feature`, "An
    // event the relay publishes twice is counted once").
    client
        .insert_events(&[row(event_id, occurred_at, 4200)])
        .await
        .unwrap();

    let report = client
        .daily_report(
            1,
            "USD",
            "UTC",
            occurred_at.date_naive(),
            occurred_at.date_naive(),
        )
        .await
        .unwrap();
    assert_eq!(
        report.totals.succeeded_count, 1,
        "FINAL must deduplicate the republished row"
    );
    assert_eq!(report.totals.gross_amount, 4200);
}

#[tokio::test]
async fn a_merchants_timezone_shifts_which_day_an_event_falls_on() {
    let fx = common::start_clickhouse().await;
    let client = &fx.client;

    // 2026-09-17T15:59:59Z is still 2026-09-17 in UTC, but already
    // 2026-09-17T23:59:59+08:00 — one second later it is Taipei's next day
    // (`reports.feature`, "Days are cut at the merchant's midnight").
    let just_before_taipei_midnight = Utc.with_ymd_and_hms(2026, 9, 17, 15, 59, 59).unwrap();
    let just_after_taipei_midnight = Utc.with_ymd_and_hms(2026, 9, 17, 16, 0, 0).unwrap();

    client
        .insert_events(&[row(Uuid::now_v7(), just_before_taipei_midnight, 1000)])
        .await
        .unwrap();
    client
        .insert_events(&[row(Uuid::now_v7(), just_after_taipei_midnight, 2000)])
        .await
        .unwrap();

    let from = NaiveDate::from_ymd_opt(2026, 9, 17).unwrap();
    let to = NaiveDate::from_ymd_opt(2026, 9, 18).unwrap();

    let taipei_report = client
        .daily_report(1, "USD", "Asia/Taipei", from, to)
        .await
        .unwrap();
    let day17 = taipei_report.days.iter().find(|d| d.date == from).unwrap();
    let day18 = taipei_report.days.iter().find(|d| d.date == to).unwrap();
    assert_eq!(day17.gross_amount, 1000, "still Sept 17 in Taipei");
    assert_eq!(day18.gross_amount, 2000, "already Sept 18 in Taipei");

    // The same two events, read back in UTC, both fall on the 17th.
    let utc_report = client
        .daily_report(1, "USD", "UTC", from, to)
        .await
        .unwrap();
    let utc_day17 = utc_report.days.iter().find(|d| d.date == from).unwrap();
    let utc_day18 = utc_report.days.iter().find(|d| d.date == to).unwrap();
    assert_eq!(
        utc_day17.gross_amount, 3000,
        "both events are still Sept 17 in UTC"
    );
    assert_eq!(utc_day18.gross_amount, 0);
}

#[tokio::test]
async fn declines_are_grouped_by_failure_code_in_the_totals() {
    let fx = common::start_clickhouse().await;
    let client = &fx.client;
    let now = Utc::now();

    let mut declined = row(Uuid::now_v7(), now, 500);
    declined.event_type = "PaymentAttemptFailed".to_string();
    declined.failure_code = "card_declined".to_string();
    client.insert_events(&[declined]).await.unwrap();

    let report = client
        .daily_report(1, "USD", "UTC", now.date_naive(), now.date_naive())
        .await
        .unwrap();
    assert_eq!(report.totals.failed_count, 1);
    assert_eq!(report.totals.declines.get("card_declined"), Some(&1));
}
