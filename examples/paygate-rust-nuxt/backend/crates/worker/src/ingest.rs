//! The ingest role: batches Kafka records into ClickHouse, committing the
//! offset only after the insert — `spec.md`, "The report's projection":
//! "Never skips: an event the ingester cannot apply stops it, with an alert
//! and without committing the offset."
//!
//! "Stops it" is read literally here: a batch this role cannot apply (because
//! a message will not parse, or because ClickHouse refuses the insert) is
//! retried, unchanged, forever — logged as an alert on every attempt — rather
//! than skipped or dropped. That is what lets `reports.feature`'s "With
//! ClickHouse down, reports are unavailable but payments are not" scenario
//! hold: the ingester process is never restarted in that scenario, only
//! ClickHouse is, so recovery has to be this role noticing its dependency
//! came back on its own, not an operator restarting it.

use std::collections::HashMap;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::watch;
use uuid::Uuid;

use paygate_domain::MerchantId;
use paygate_infra::clickhouse::{
    map_event_to_report_row, AuditEventRow, ClickHouseClient, ReportEventRow,
};
use paygate_kafkabus::{Consumer, OwnedRecord};

/// How long to wait between retries of a batch this role cannot yet apply
/// (a malformed message, or ClickHouse refusing the insert).
const APPLY_RETRY: Duration = Duration::from_secs(1);

pub struct IngestLoopConfig {
    pub batch_max: usize,
    pub batch_wait: Duration,
    pub instance_id: String,
}

/// The wire shape the relay writes (`paygate_infra::relay::kafka_value`):
/// the whole audit row, as JSON. A local mirror rather than reusing
/// `AuditEventRow` directly, since that type does not derive `Deserialize`
/// and `infra` is not this crate's to edit.
#[derive(Debug, Deserialize)]
struct WireEvent {
    event_id: Uuid,
    payment_id: Uuid,
    merchant_id: MerchantId,
    seq: i32,
    event_type: String,
    payload: Value,
    occurred_at: DateTime<Utc>,
}

impl From<WireEvent> for AuditEventRow {
    fn from(w: WireEvent) -> Self {
        AuditEventRow {
            event_id: w.event_id,
            payment_id: w.payment_id,
            merchant_id: w.merchant_id,
            seq: w.seq,
            event_type: w.event_type,
            payload: w.payload,
            occurred_at: w.occurred_at,
        }
    }
}

/// Parses every record in a batch, or names the first one that failed (topic,
/// partition, offset) — pure, so it is testable without a broker.
fn parse_batch(batch: &[OwnedRecord]) -> Result<Vec<AuditEventRow>, String> {
    batch
        .iter()
        .map(|record| {
            serde_json::from_slice::<WireEvent>(&record.payload)
                .map(AuditEventRow::from)
                .map_err(|err| {
                    format!(
                        "{}[{}]@{}: {err}",
                        record.topic, record.partition, record.offset
                    )
                })
        })
        .collect()
}

/// The highest-offset record per partition in `batch` — committing just
/// these advances every assigned partition to exactly where this batch left
/// it, without one synchronous commit per message.
fn select_commit_points(batch: &[OwnedRecord]) -> Vec<OwnedRecord> {
    let mut latest: HashMap<(String, i32), OwnedRecord> = HashMap::new();
    for record in batch {
        let key = (record.topic.clone(), record.partition);
        latest
            .entry(key)
            .and_modify(|current| {
                if record.offset > current.offset {
                    *current = record.clone();
                }
            })
            .or_insert_with(|| record.clone());
    }
    latest.into_values().collect()
}

fn commit_batch(consumer: &Consumer, batch: &[OwnedRecord]) {
    for record in select_commit_points(batch) {
        if let Err(err) = consumer.commit(&record) {
            tracing::error!(error = %err, "failed to commit ingester offset");
        }
    }
}

/// Collects up to `batch_max` records, waiting at most `batch_wait` for each
/// one to arrive — the batching the `INGEST_BATCH_MAX`/`INGEST_BATCH_WAIT_MS`
/// row of `spec.md`'s "Environment variables" describes. Returns whatever was
/// collected (possibly empty) the moment shutdown is requested, rather than
/// blocking on one more message that may never come.
async fn collect_batch(
    consumer: &Consumer,
    batch_max: usize,
    batch_wait: Duration,
    shutdown: &mut watch::Receiver<bool>,
) -> Vec<OwnedRecord> {
    let mut batch = Vec::new();
    while batch.len() < batch_max {
        if crate::shutdown::requested(shutdown) {
            break;
        }
        tokio::select! {
            biased;
            _ = shutdown.changed() => {
                if crate::shutdown::requested(shutdown) {
                    break;
                }
            }
            recv = consumer.recv(batch_wait) => {
                match recv {
                    Ok(record) => batch.push(record),
                    Err(_) => break,
                }
            }
        }
    }
    batch
}

/// Applies one batch: parse every record, map the projected ones to report
/// rows, insert, and only then commit — retrying in place (never skipping,
/// never committing early) on either kind of failure, per this module's own
/// doc comment. Returns early, with nothing committed, only when shutdown is
/// requested mid-retry.
async fn apply_batch(
    consumer: &Consumer,
    ch: &ClickHouseClient,
    batch: Vec<OwnedRecord>,
    instance_id: &str,
    shutdown: &mut watch::Receiver<bool>,
) {
    let audit_rows = loop {
        match parse_batch(&batch) {
            Ok(rows) => break rows,
            Err(reason) => {
                tracing::error!(alert = true, reason = %reason, "ingester cannot apply an event; holding the offset");
                if crate::shutdown::requested(shutdown) {
                    return;
                }
                crate::shutdown::sleep_or_shutdown(APPLY_RETRY, shutdown).await;
                if crate::shutdown::requested(shutdown) {
                    return;
                }
            }
        }
    };

    let report_rows: Vec<ReportEventRow> = audit_rows
        .iter()
        .filter_map(|row| map_event_to_report_row(row, instance_id))
        .collect();

    loop {
        match ch.insert_events(&report_rows).await {
            Ok(()) => break,
            Err(err) => {
                tracing::error!(alert = true, error = %err, "clickhouse insert failed; holding the offset and retrying");
                if crate::shutdown::requested(shutdown) {
                    return;
                }
                crate::shutdown::sleep_or_shutdown(APPLY_RETRY, shutdown).await;
                if crate::shutdown::requested(shutdown) {
                    return;
                }
            }
        }
    }

    commit_batch(consumer, &batch);
}

/// The ingest role's main loop: collect a batch, apply it, repeat — forever,
/// until shutdown.
pub async fn run(
    consumer: std::sync::Arc<Consumer>,
    ch: ClickHouseClient,
    cfg: IngestLoopConfig,
    mut shutdown: watch::Receiver<bool>,
) {
    loop {
        if crate::shutdown::requested(&shutdown) {
            return;
        }
        let batch = collect_batch(&consumer, cfg.batch_max, cfg.batch_wait, &mut shutdown).await;
        if batch.is_empty() {
            continue;
        }
        apply_batch(&consumer, &ch, batch, &cfg.instance_id, &mut shutdown).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(topic: &str, partition: i32, offset: i64, payload: &[u8]) -> OwnedRecord {
        OwnedRecord {
            topic: topic.to_string(),
            partition,
            offset,
            key: None,
            payload: payload.to_vec(),
        }
    }

    fn wire_json(event_type: &str) -> Vec<u8> {
        serde_json::json!({
            "event_id": Uuid::from_u128(1),
            "payment_id": Uuid::from_u128(2),
            "merchant_id": 7,
            "seq": 1,
            "event_type": event_type,
            "payload": {"amount": 1000},
            "occurred_at": Utc::now(),
        })
        .to_string()
        .into_bytes()
    }

    #[test]
    fn a_well_formed_batch_parses_into_audit_rows() {
        let batch = vec![record("t", 0, 1, &wire_json("PaymentCreated"))];
        let rows = parse_batch(&batch).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].event_type, "PaymentCreated");
        assert_eq!(rows[0].merchant_id, 7);
    }

    #[test]
    fn a_malformed_record_fails_parsing_and_names_its_position() {
        let batch = vec![
            record("t", 0, 1, &wire_json("PaymentCreated")),
            record("t", 0, 2, b"not json"),
        ];
        let err = parse_batch(&batch).unwrap_err();
        assert!(err.contains("t[0]@2"));
    }

    #[test]
    fn a_projected_row_maps_to_a_report_row_the_way_infra_documents() {
        let batch = vec![record("t", 0, 1, &wire_json("PaymentSucceeded"))];
        let rows = parse_batch(&batch).unwrap();
        let mapped = map_event_to_report_row(&rows[0], "ing-1").unwrap();
        assert_eq!(mapped.event_type, "PaymentSucceeded");
        assert_eq!(mapped.ingester_id, "ing-1");
    }

    #[test]
    fn a_non_projected_row_maps_to_nothing_and_is_still_committed() {
        let batch = vec![record("t", 0, 1, &wire_json("PaymentCreated"))];
        let rows = parse_batch(&batch).unwrap();
        assert!(map_event_to_report_row(&rows[0], "ing-1").is_none());
    }

    #[test]
    fn only_the_highest_offset_per_partition_is_a_commit_point() {
        let batch = vec![
            record("t", 0, 5, b"{}"),
            record("t", 0, 7, b"{}"),
            record("t", 1, 3, b"{}"),
            record("t", 0, 6, b"{}"),
        ];
        let mut points = select_commit_points(&batch);
        points.sort_by_key(|r| (r.partition, r.offset));
        assert_eq!(points.len(), 2);
        assert_eq!(points[0].partition, 0);
        assert_eq!(points[0].offset, 7);
        assert_eq!(points[1].partition, 1);
        assert_eq!(points[1].offset, 3);
    }

    #[test]
    fn an_empty_batch_has_no_commit_points() {
        assert!(select_commit_points(&[]).is_empty());
    }
}
