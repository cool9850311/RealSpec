//! The relay: claim unpublished `payment_events` rows with
//! `FOR UPDATE SKIP LOCKED` (so any number of `worker relay` replicas can run
//! the same query at once and never claim the same row twice — `spec.md`,
//! "Scaling"), publish each to Kafka, and mark it published.
//!
//! This crate does not depend on `paygate-kafkabus` (only `crates/kafkabus`
//! links `rdkafka`), so [`run_once`] takes the actual
//! publish step as a closure — the `worker` binary supplies one built from
//! `kafkabus::Producer::send`, keyed by [`kafka_key`], which the ONE test
//! this module can run without a broker exercises directly.

use chrono::{DateTime, Utc};
use paygate_domain::MerchantId;
use serde_json::Value;
use sqlx::Row;
use uuid::Uuid;

use crate::db::Db;
use crate::error::Result;

/// One claimed row, ready to publish.
#[derive(Debug, Clone)]
pub struct EventToPublish {
    pub id: i64,
    pub event_id: Uuid,
    pub payment_id: Uuid,
    pub merchant_id: MerchantId,
    pub seq: i32,
    pub event_type: String,
    pub payload: Value,
    pub occurred_at: DateTime<Utc>,
}

/// The Kafka key: always `payment_id` (`spec.md`, "Kafka": "Key:
/// `payment_id`"), so every event about one payment lands on the same
/// partition and is read back in order.
pub fn kafka_key(event: &EventToPublish) -> String {
    event.payment_id.to_string()
}

/// The Kafka value: the whole audit row, as JSON.
pub fn kafka_value(event: &EventToPublish) -> Vec<u8> {
    serde_json::json!({
        "event_id": event.event_id,
        "payment_id": event.payment_id,
        "merchant_id": event.merchant_id,
        "seq": event.seq,
        "event_type": event.event_type,
        "payload": event.payload,
        "occurred_at": event.occurred_at,
    })
    .to_string()
    .into_bytes()
}

/// One relay pass: claim up to `batch_size` unpublished rows, publish each
/// through `publish` in order, and mark every one that succeeded. Stops at
/// the first failure — the rows after it stay claimed only for the
/// transaction's own lifetime and are simply unpublished again once it
/// commits, ready for the next pass or another replica.
///
/// Returns how many rows were published.
pub async fn run_once<F, Fut>(db: &Db, batch_size: i64, publish: F) -> Result<usize>
where
    F: Fn(&EventToPublish) -> Fut,
    Fut: std::future::Future<Output = std::result::Result<(), String>>,
{
    let mut tx = db.begin().await?;

    let rows = sqlx::query(
        "SELECT id, event_id, payment_id, merchant_id, seq, event_type, payload, occurred_at \
           FROM payment_events \
          WHERE published_at IS NULL \
          ORDER BY id ASC \
          LIMIT $1 \
          FOR UPDATE SKIP LOCKED",
    )
    .bind(batch_size)
    .fetch_all(&mut *tx)
    .await?;

    let events: Vec<EventToPublish> = rows
        .iter()
        .map(|r| {
            Ok::<_, sqlx::Error>(EventToPublish {
                id: r.try_get("id")?,
                event_id: r.try_get("event_id")?,
                payment_id: r.try_get("payment_id")?,
                merchant_id: r.try_get("merchant_id")?,
                seq: r.try_get("seq")?,
                event_type: r.try_get("event_type")?,
                payload: r.try_get("payload")?,
                occurred_at: r.try_get("occurred_at")?,
            })
        })
        .collect::<std::result::Result<_, _>>()?;

    let mut published = 0usize;
    for event in &events {
        if publish(event).await.is_err() {
            break;
        }
        sqlx::query(
            "UPDATE payment_events SET published_at = now(), publish_count = publish_count + 1 \
              WHERE id = $1",
        )
        .bind(event.id)
        .execute(&mut *tx)
        .await?;
        published += 1;
    }

    tx.commit().await?;
    Ok(published)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event() -> EventToPublish {
        EventToPublish {
            id: 1,
            event_id: Uuid::from_u128(1),
            payment_id: Uuid::from_u128(2),
            merchant_id: 1,
            seq: 1,
            event_type: "PaymentCreated".to_string(),
            payload: serde_json::json!({"amount": 1000}),
            occurred_at: Utc::now(),
        }
    }

    #[test]
    fn the_kafka_key_is_the_payment_id() {
        let e = event();
        assert_eq!(kafka_key(&e), e.payment_id.to_string());
    }

    #[test]
    fn the_kafka_value_carries_the_whole_audit_row() {
        let e = event();
        let value = kafka_value(&e);
        let parsed: Value = serde_json::from_slice(&value).unwrap();
        assert_eq!(parsed["event_id"], e.event_id.to_string());
        assert_eq!(parsed["event_type"], "PaymentCreated");
        assert_eq!(parsed["payload"]["amount"], 1000);
    }
}
