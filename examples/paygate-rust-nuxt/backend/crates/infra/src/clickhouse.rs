//! ClickHouse over plain HTTP: no client crate — the queries are plain SQL.
//! Four jobs, in this module:
//!
//! - [`map_event_to_report_row`] — the pure audit-row -> `report_events`
//!   mapping the ingester applies to every batch it reads off Kafka. Pure
//!   and total, so it is what this module's own unit tests exercise, rather
//!   than a live ClickHouse.
//! - [`ClickHouseClient::insert_events`] — the batch insert.
//! - [`ClickHouseClient::daily_report`] — the report query, `FINAL` (`spec.md`,
//!   "The report's projection": "a ClickHouse MV fires before deduplication
//!   and would count duplicates the pipeline is allowed to produce").
//! - [`ClickHouseClient::truncate`] plus [`checkpoint`] — what a rebuild
//!   needs: throw the projection away, and reset the PostgreSQL bookkeeping
//!   of how far a projector has read `payment_events` (`spec.md`, "Data
//!   model": `projection_checkpoints`, "read by a rebuild and at startup").

use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use chrono_tz::Tz;
use paygate_domain::{Amount, EventType, MerchantId};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::str::FromStr;
use uuid::Uuid;

use crate::db::Db;
use crate::error::{Error, Result};

/// One row of `report_events` — `spec.md`'s "Data model" -> "ClickHouse"
/// column list, in Rust.
#[derive(Debug, Clone, PartialEq)]
pub struct ReportEventRow {
    pub event_id: Uuid,
    pub seq: u32,
    pub merchant_id: MerchantId,
    pub payment_id: Uuid,
    pub event_type: String,
    pub provider_code: String,
    pub amount: Amount,
    pub currency: String,
    pub card_brand: String,
    pub failure_code: String,
    pub reference: String,
    pub refund_id: Uuid,
    pub occurred_at: DateTime<Utc>,
    pub ingester_id: String,
}

/// One `payment_events` row, as read back from PostgreSQL for projection —
/// deliberately not `crate::repo::PaymentRow`, since this is the raw audit
/// row (`event_type`, `payload` JSON) rather than a payment's current state.
#[derive(Debug, Clone)]
pub struct AuditEventRow {
    pub event_id: Uuid,
    pub payment_id: Uuid,
    pub merchant_id: MerchantId,
    pub seq: i32,
    pub event_type: String,
    pub payload: Value,
    pub occurred_at: DateTime<Utc>,
}

fn str_field(payload: &Value, key: &str) -> String {
    payload
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn amount_field(payload: &Value) -> Amount {
    payload.get("amount").and_then(Value::as_i64).unwrap_or(0)
}

fn uuid_field(payload: &Value, key: &str) -> Uuid {
    payload
        .get(key)
        .and_then(Value::as_str)
        .and_then(|s| Uuid::from_str(s).ok())
        .unwrap_or(Uuid::nil())
}

/// Map one audit row to a `report_events` row, or `None` when
/// `EventType::is_projected` says this event type never reaches the report
/// (`spec.md`'s Events table). Pure and total: an audit row's `payload` is a
/// free-form JSONB blob by design (`payment_events.payload jsonb`), so every
/// field read here degrades to an empty/zero default rather than failing —
/// the report is allowed to show a blank `card_brand` for an event that
/// never had one; it is never allowed to stop the ingester.
pub fn map_event_to_report_row(row: &AuditEventRow, ingester_id: &str) -> Option<ReportEventRow> {
    let event_type: EventType = row.event_type.parse().ok()?;
    if !event_type.is_projected() {
        return None;
    }
    Some(ReportEventRow {
        event_id: row.event_id,
        seq: row.seq.max(0) as u32,
        merchant_id: row.merchant_id,
        payment_id: row.payment_id,
        event_type: row.event_type.clone(),
        provider_code: str_field(&row.payload, "provider_code"),
        amount: amount_field(&row.payload),
        currency: str_field(&row.payload, "currency"),
        card_brand: str_field(&row.payload, "card_brand"),
        failure_code: str_field(&row.payload, "failure_code"),
        reference: str_field(&row.payload, "reference"),
        refund_id: uuid_field(&row.payload, "refund_id"),
        occurred_at: row.occurred_at,
        ingester_id: ingester_id.to_string(),
    })
}

fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\'', "\\'")
}

fn row_to_jsonl(row: &ReportEventRow) -> String {
    serde_json::json!({
        "event_id": row.event_id,
        "seq": row.seq,
        "merchant_id": row.merchant_id,
        "payment_id": row.payment_id,
        "event_type": row.event_type,
        "provider_code": row.provider_code,
        "amount": row.amount,
        "currency": row.currency,
        "card_brand": row.card_brand,
        "failure_code": row.failure_code,
        "reference": row.reference,
        "refund_id": row.refund_id,
        "occurred_at": row.occurred_at.format("%Y-%m-%d %H:%M:%S%.3f").to_string(),
        "ingested_at": Utc::now().format("%Y-%m-%d %H:%M:%S%.3f").to_string(),
        "ingester_id": row.ingester_id,
    })
    .to_string()
}

pub struct ClickHouseClient {
    http: reqwest::Client,
    url: String,
    database: String,
    user: String,
    password: String,
}

impl ClickHouseClient {
    pub fn new(url: &str, database: &str, user: &str, password: &str) -> Self {
        Self {
            http: reqwest::Client::new(),
            url: url.trim_end_matches('/').to_string(),
            database: database.to_string(),
            user: user.to_string(),
            password: password.to_string(),
        }
    }

    /// Send `sql`, plus `data` when there is a payload beyond the statement
    /// itself (an `INSERT ... FORMAT JSONEachRow`'s rows). Always as the
    /// POST BODY, never as a `?query=` URL parameter: `reqwest`'s POST
    /// without an explicit body sets neither `Content-Length` nor
    /// `Transfer-Encoding: chunked`, which a real ClickHouse server refuses
    /// outright (`411 Length Required`) — found by this crate's own
    /// integration tests against a live ClickHouse container, not by
    /// inspection. A statement is never empty, so this sidesteps the
    /// ambiguity entirely rather than special-casing an empty body.
    async fn execute(&self, sql: &str, data: Option<String>) -> Result<String> {
        let body = match data {
            Some(data) => format!("{sql}\n{data}"),
            None => sql.to_string(),
        };
        let resp = self
            .http
            .post(&self.url)
            .basic_auth(&self.user, Some(&self.password))
            .query(&[
                ("database", self.database.as_str()),
                // ClickHouse's `JSONEachRow` quotes 64-bit integers
                // (`UInt64`/`Int64`, which `count()` and every amount column
                // here are) as JSON strings by default, since JavaScript
                // cannot represent them exactly. This crate parses them
                // straight into `u64`/`i64`, so it asks ClickHouse not to
                // quote them instead of teaching every row type its own
                // string-or-number deserializer.
                ("output_format_json_quote_64bit_integers", "0"),
            ])
            .body(body)
            .send()
            .await
            .map_err(|e| Error::ClickHouseUnavailable(e.to_string()))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(Error::ClickHouseUnavailable(format!("{status}: {text}")));
        }
        resp.text()
            .await
            .map_err(|e| Error::ClickHouseUnavailable(e.to_string()))
    }

    pub async fn ping(&self) -> bool {
        self.execute("SELECT 1", None).await.is_ok()
    }

    /// Run arbitrary DDL/SQL with no result expected — this crate's own
    /// migration file (`backend/migrations/clickhouse/0001_init.sql`) is a
    /// single `CREATE TABLE IF NOT EXISTS`, and this is what runs it, both
    /// for a deployment's own migration step and for this crate's
    /// integration tests, which create the real schema in a fresh container
    /// rather than approximate it by hand.
    pub async fn execute_ddl(&self, sql: &str) -> Result<()> {
        self.execute(sql, None).await?;
        Ok(())
    }

    /// Batch-insert rows as `JSONEachRow`, the format ClickHouse's HTTP
    /// interface reads a POST body as directly.
    pub async fn insert_events(&self, rows: &[ReportEventRow]) -> Result<()> {
        if rows.is_empty() {
            return Ok(());
        }
        let body = rows.iter().map(row_to_jsonl).collect::<Vec<_>>().join("\n");
        self.execute(
            "INSERT INTO report_events \
                (event_id, seq, merchant_id, payment_id, event_type, provider_code, amount, \
                 currency, card_brand, failure_code, reference, refund_id, occurred_at, \
                 ingested_at, ingester_id) \
             FORMAT JSONEachRow",
            Some(body),
        )
        .await?;
        Ok(())
    }

    /// Truncate the projection — half of a rebuild; the other half is
    /// [`checkpoint::reset`].
    pub async fn truncate(&self) -> Result<()> {
        self.execute("TRUNCATE TABLE report_events", None).await?;
        Ok(())
    }

    pub async fn daily_report(
        &self,
        merchant_id: MerchantId,
        currency: &str,
        timezone: &str,
        from: NaiveDate,
        to: NaiveDate,
    ) -> Result<DailyReport> {
        let tz: Tz = timezone
            .parse()
            .map_err(|_| Error::ClickHouseUnavailable(format!("invalid timezone {timezone:?}")))?;
        let range_start = day_start_utc(tz, from);
        let range_end_exclusive = day_start_utc(tz, to.succ_opt().unwrap_or(to));

        let query = format!(
            "SELECT \
                toDate(occurred_at, '{tz_e}') AS day, \
                countIf(event_type IN ('PaymentSucceeded', 'PaymentReconciled')) AS succeeded_count, \
                countIf(event_type = 'PaymentAttemptFailed') AS failed_count, \
                countIf(event_type = 'PaymentRefunded') AS refunded_count, \
                sumIf(amount, event_type IN ('PaymentSucceeded', 'PaymentReconciled')) AS gross_amount, \
                sumIf(amount, event_type = 'PaymentRefunded') AS refunded_amount \
             FROM report_events FINAL \
             WHERE merchant_id = {merchant_id} \
               AND occurred_at >= toDateTime64('{start}', 3, 'UTC') \
               AND occurred_at < toDateTime64('{end}', 3, 'UTC') \
             GROUP BY day \
             ORDER BY day \
             FORMAT JSONEachRow",
            tz_e = escape(timezone),
            merchant_id = merchant_id,
            start = range_start.format("%Y-%m-%d %H:%M:%S%.3f"),
            end = range_end_exclusive.format("%Y-%m-%d %H:%M:%S%.3f"),
        );
        let body = self.execute(&query, None).await?;
        let mut by_day: BTreeMap<NaiveDate, DayRow> = BTreeMap::new();
        for line in body.lines().filter(|l| !l.trim().is_empty()) {
            let row: RawDayRow = serde_json::from_str(line)
                .map_err(|e| Error::ClickHouseUnavailable(e.to_string()))?;
            let date = NaiveDate::parse_from_str(&row.day, "%Y-%m-%d")
                .map_err(|e| Error::ClickHouseUnavailable(e.to_string()))?;
            by_day.insert(
                date,
                DayRow {
                    succeeded_count: row.succeeded_count,
                    failed_count: row.failed_count,
                    refunded_count: row.refunded_count,
                    gross_amount: row.gross_amount,
                    refunded_amount: row.refunded_amount,
                },
            );
        }

        let declines_query = format!(
            "SELECT failure_code, count() AS c \
             FROM report_events FINAL \
             WHERE merchant_id = {merchant_id} \
               AND event_type = 'PaymentAttemptFailed' \
               AND occurred_at >= toDateTime64('{start}', 3, 'UTC') \
               AND occurred_at < toDateTime64('{end}', 3, 'UTC') \
               AND failure_code != '' \
             GROUP BY failure_code \
             FORMAT JSONEachRow",
            merchant_id = merchant_id,
            start = range_start.format("%Y-%m-%d %H:%M:%S%.3f"),
            end = range_end_exclusive.format("%Y-%m-%d %H:%M:%S%.3f"),
        );
        let declines_body = self.execute(&declines_query, None).await?;
        let mut declines = BTreeMap::new();
        for line in declines_body.lines().filter(|l| !l.trim().is_empty()) {
            let row: RawDeclineRow = serde_json::from_str(line)
                .map_err(|e| Error::ClickHouseUnavailable(e.to_string()))?;
            declines.insert(row.failure_code, row.c);
        }

        Ok(build_daily_report(
            currency, timezone, from, to, by_day, declines,
        ))
    }
}

fn day_start_utc(tz: Tz, date: NaiveDate) -> DateTime<Utc> {
    let naive = date.and_hms_opt(0, 0, 0).unwrap();
    tz.from_local_datetime(&naive)
        .single()
        .unwrap_or_else(|| tz.from_utc_datetime(&naive))
        .with_timezone(&Utc)
}

#[derive(Debug, Clone, Deserialize)]
struct RawDayRow {
    day: String,
    succeeded_count: u64,
    failed_count: u64,
    refunded_count: u64,
    gross_amount: i64,
    refunded_amount: i64,
}

#[derive(Debug, Clone, Deserialize)]
struct RawDeclineRow {
    failure_code: String,
    c: u64,
}

#[derive(Debug, Clone, Copy, Default)]
struct DayRow {
    succeeded_count: u64,
    failed_count: u64,
    refunded_count: u64,
    gross_amount: i64,
    refunded_amount: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DayFigures {
    pub date: NaiveDate,
    pub succeeded_count: u64,
    pub failed_count: u64,
    pub refunded_count: u64,
    pub gross_amount: i64,
    pub refunded_amount: i64,
    pub net_amount: i64,
    pub success_rate_bps: Option<u32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Totals {
    pub succeeded_count: u64,
    pub failed_count: u64,
    pub refunded_count: u64,
    pub gross_amount: i64,
    pub refunded_amount: i64,
    pub net_amount: i64,
    pub success_rate_bps: Option<u32>,
    pub declines: BTreeMap<String, u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DailyReport {
    pub currency: String,
    pub timezone: String,
    pub from: NaiveDate,
    pub to: NaiveDate,
    pub days: Vec<DayFigures>,
    pub totals: Totals,
}

/// `⌊ succeeded × 10000 ÷ (succeeded + failed) ⌋`, or `None` when there were
/// neither (`openapi.yaml`, `getDailyReport`).
pub fn success_rate_bps(succeeded: u64, failed: u64) -> Option<u32> {
    let denom = succeeded + failed;
    if denom == 0 {
        None
    } else {
        Some(((succeeded * 10_000) / denom) as u32)
    }
}

fn build_daily_report(
    currency: &str,
    timezone: &str,
    from: NaiveDate,
    to: NaiveDate,
    by_day: BTreeMap<NaiveDate, DayRow>,
    declines: BTreeMap<String, u64>,
) -> DailyReport {
    let mut days = Vec::new();
    let mut date = from;
    let mut totals = DayRow::default();
    while date <= to {
        let row = by_day.get(&date).copied().unwrap_or_default();
        totals.succeeded_count += row.succeeded_count;
        totals.failed_count += row.failed_count;
        totals.refunded_count += row.refunded_count;
        totals.gross_amount += row.gross_amount;
        totals.refunded_amount += row.refunded_amount;
        days.push(DayFigures {
            date,
            succeeded_count: row.succeeded_count,
            failed_count: row.failed_count,
            refunded_count: row.refunded_count,
            gross_amount: row.gross_amount,
            refunded_amount: row.refunded_amount,
            net_amount: row.gross_amount - row.refunded_amount,
            success_rate_bps: success_rate_bps(row.succeeded_count, row.failed_count),
        });
        date = date.succ_opt().unwrap_or(date);
        if date == days.last().unwrap().date {
            break; // guard against a NaiveDate overflow at the calendar's edge.
        }
    }

    DailyReport {
        currency: currency.to_string(),
        timezone: timezone.to_string(),
        from,
        to,
        days,
        totals: Totals {
            succeeded_count: totals.succeeded_count,
            failed_count: totals.failed_count,
            refunded_count: totals.refunded_count,
            gross_amount: totals.gross_amount,
            refunded_amount: totals.refunded_amount,
            net_amount: totals.gross_amount - totals.refunded_amount,
            success_rate_bps: success_rate_bps(totals.succeeded_count, totals.failed_count),
            declines,
        },
    }
}

/// The PostgreSQL half of a rebuild: `projection_checkpoints`
/// (`spec.md`, "Data model": "read by a rebuild and at startup").
pub mod checkpoint {
    use super::*;
    use sqlx::Row;

    #[derive(Debug, Clone)]
    pub struct Checkpoint {
        pub last_event_id: Option<Uuid>,
        pub last_position: i64,
    }

    pub async fn read(db: &Db, projection: &str) -> Result<Option<Checkpoint>> {
        let row = sqlx::query(
            "SELECT last_event_id, last_position FROM projection_checkpoints WHERE projection = $1",
        )
        .bind(projection)
        .fetch_optional(db.pool())
        .await?;
        Ok(row.map(|r| Checkpoint {
            last_event_id: r.get("last_event_id"),
            last_position: r.get("last_position"),
        }))
    }

    pub async fn write(
        db: &Db,
        projection: &str,
        last_event_id: Uuid,
        last_position: i64,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO projection_checkpoints (projection, last_event_id, last_position, updated_at) \
             VALUES ($1, $2, $3, now()) \
             ON CONFLICT (projection) DO UPDATE \
                SET last_event_id = EXCLUDED.last_event_id, \
                    last_position = EXCLUDED.last_position, \
                    updated_at = now()",
        )
        .bind(projection)
        .bind(last_event_id)
        .bind(last_position)
        .execute(db.pool())
        .await?;
        Ok(())
    }

    /// A rebuild's own half of "truncate and replay": the checkpoint goes
    /// back to zero together with the ClickHouse table
    /// ([`super::ClickHouseClient::truncate`]).
    pub async fn reset(db: &Db, projection: &str) -> Result<()> {
        sqlx::query(
            "INSERT INTO projection_checkpoints (projection, last_event_id, last_position, updated_at) \
             VALUES ($1, NULL, 0, now()) \
             ON CONFLICT (projection) DO UPDATE \
                SET last_event_id = NULL, last_position = 0, updated_at = now()",
        )
        .bind(projection)
        .execute(db.pool())
        .await?;
        Ok(())
    }
}

/// Read `payment_events` in `payment_events.id` order, from just after
/// `after_position`, in batches — what both the ingester's live tail and a
/// full rebuild replay from. Returns each row alongside its own `id` (the
/// bigserial POSITION — not `seq`, which is only unique per payment), which
/// a caller needs to advance the projection checkpoint.
pub async fn read_audit_events_after(
    db: &Db,
    after_position: i64,
    limit: i64,
) -> Result<Vec<(i64, AuditEventRow)>> {
    let rows = sqlx::query(
        "SELECT id, event_id, payment_id, merchant_id, seq, event_type, payload, occurred_at \
           FROM payment_events \
          WHERE id > $1 \
          ORDER BY id ASC \
          LIMIT $2",
    )
    .bind(after_position)
    .bind(limit)
    .fetch_all(db.pool())
    .await?;

    use sqlx::Row;
    rows.iter()
        .map(|r| {
            let position: i64 = r.try_get("id")?;
            Ok((
                position,
                AuditEventRow {
                    event_id: r.try_get("event_id")?,
                    payment_id: r.try_get("payment_id")?,
                    merchant_id: r.try_get("merchant_id")?,
                    seq: r.try_get("seq")?,
                    event_type: r.try_get("event_type")?,
                    payload: r.try_get("payload")?,
                    occurred_at: r.try_get("occurred_at")?,
                },
            ))
        })
        .collect()
}

/// `worker rebuild reports`: truncate the projection and the checkpoint,
/// then replay every projected event straight from PostgreSQL — never from
/// Kafka, whose retention is a setting rather than a record (`spec.md`, "The
/// report's projection"). Returns how many rows were written.
pub async fn rebuild_reports(
    db: &Db,
    ch: &ClickHouseClient,
    batch_size: i64,
    ingester_id: &str,
) -> Result<u64> {
    ch.truncate().await?;
    checkpoint::reset(db, "reports").await?;

    let mut position = 0i64;
    let mut total = 0u64;
    loop {
        let batch = read_audit_events_after(db, position, batch_size).await?;
        if batch.is_empty() {
            break;
        }
        let rows: Vec<ReportEventRow> = batch
            .iter()
            .filter_map(|(_, e)| map_event_to_report_row(e, ingester_id))
            .collect();
        if !rows.is_empty() {
            ch.insert_events(&rows).await?;
            total += rows.len() as u64;
        }
        position = batch.last().map(|(pos, _)| *pos).unwrap_or(position);
        checkpoint::write(db, "reports", batch.last().unwrap().1.event_id, position).await?;
        if (batch.len() as i64) < batch_size {
            break;
        }
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn audit_row(event_type: &str, payload: Value) -> AuditEventRow {
        AuditEventRow {
            event_id: Uuid::from_u128(1),
            payment_id: Uuid::from_u128(2),
            merchant_id: 7,
            seq: 2,
            event_type: event_type.to_string(),
            payload,
            occurred_at: Utc::now(),
        }
    }

    #[test]
    fn a_non_projected_event_maps_to_nothing() {
        let row = audit_row("PaymentCreated", serde_json::json!({"amount": 1000}));
        assert!(map_event_to_report_row(&row, "ing-1").is_none());
    }

    #[test]
    fn a_projected_event_maps_every_documented_field() {
        let row = audit_row(
            "PaymentSucceeded",
            serde_json::json!({
                "amount": 4200, "currency": "USD", "card_brand": "visa",
                "reference": "ACME-1", "provider_charge_id": "ch_1", "source": "callback"
            }),
        );
        let mapped = map_event_to_report_row(&row, "ing-1").unwrap();
        assert_eq!(mapped.event_type, "PaymentSucceeded");
        assert_eq!(mapped.amount, 4200);
        assert_eq!(mapped.currency, "USD");
        assert_eq!(mapped.card_brand, "visa");
        assert_eq!(mapped.reference, "ACME-1");
        assert_eq!(mapped.merchant_id, 7);
        assert_eq!(mapped.ingester_id, "ing-1");
    }

    #[test]
    fn a_missing_field_degrades_to_a_default_rather_than_failing() {
        let row = audit_row("PaymentRefunded", serde_json::json!({}));
        let mapped = map_event_to_report_row(&row, "ing-1").unwrap();
        assert_eq!(mapped.amount, 0);
        assert_eq!(mapped.card_brand, "");
        assert_eq!(mapped.refund_id, Uuid::nil());
    }

    #[test]
    fn every_projected_event_type_from_the_events_table_maps() {
        for et in [
            "PaymentAttemptFailed",
            "PaymentSucceeded",
            "PaymentReconciled",
            "PaymentDuplicatePaid",
            "PaymentRefunded",
        ] {
            let row = audit_row(et, serde_json::json!({}));
            assert!(
                map_event_to_report_row(&row, "i").is_some(),
                "{et} should project"
            );
        }
        for et in [
            "PaymentCreated",
            "PaymentAttemptStarted",
            "PaymentAttemptAbandoned",
            "ProviderCallTimedOut",
        ] {
            let row = audit_row(et, serde_json::json!({}));
            assert!(
                map_event_to_report_row(&row, "i").is_none(),
                "{et} should not project"
            );
        }
    }

    #[test]
    fn success_rate_is_the_floor_of_succeeded_over_attempts_in_basis_points() {
        assert_eq!(success_rate_bps(2, 1), Some(6666));
        assert_eq!(success_rate_bps(1, 1), Some(5000));
        assert_eq!(success_rate_bps(1, 0), Some(10000));
        assert_eq!(success_rate_bps(0, 0), None);
    }

    #[test]
    fn a_days_range_fills_in_every_day_even_with_no_activity() {
        let mut by_day = BTreeMap::new();
        by_day.insert(
            NaiveDate::from_ymd_opt(2026, 9, 17).unwrap(),
            DayRow {
                succeeded_count: 1,
                gross_amount: 1000,
                ..Default::default()
            },
        );
        let report = build_daily_report(
            "EUR",
            "UTC",
            NaiveDate::from_ymd_opt(2026, 9, 17).unwrap(),
            NaiveDate::from_ymd_opt(2026, 9, 18).unwrap(),
            by_day,
            BTreeMap::new(),
        );
        assert_eq!(report.days.len(), 2);
        assert_eq!(report.days[1].succeeded_count, 0);
        assert_eq!(report.days[1].success_rate_bps, None);
        assert_eq!(report.totals.succeeded_count, 1);
        assert_eq!(report.totals.gross_amount, 1000);
        assert_eq!(report.totals.net_amount, 1000);
    }

    #[test]
    fn net_amount_may_go_negative() {
        let mut by_day = BTreeMap::new();
        by_day.insert(
            NaiveDate::from_ymd_opt(2026, 9, 18).unwrap(),
            DayRow {
                gross_amount: 1000,
                refunded_amount: 1500,
                ..Default::default()
            },
        );
        let report = build_daily_report(
            "USD",
            "UTC",
            NaiveDate::from_ymd_opt(2026, 9, 18).unwrap(),
            NaiveDate::from_ymd_opt(2026, 9, 18).unwrap(),
            by_day,
            BTreeMap::new(),
        );
        assert_eq!(report.totals.net_amount, -500);
    }

    #[test]
    fn day_start_is_computed_in_the_merchants_own_timezone() {
        let taipei: Tz = "Asia/Taipei".parse().unwrap();
        let start = day_start_utc(taipei, NaiveDate::from_ymd_opt(2026, 9, 18).unwrap());
        // Midnight in Taipei (UTC+8) is 16:00 UTC the previous day.
        assert_eq!(
            start.format("%Y-%m-%dT%H:%M:%S").to_string(),
            "2026-09-17T16:00:00"
        );
    }
}
