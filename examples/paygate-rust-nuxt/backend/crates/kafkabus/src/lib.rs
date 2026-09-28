//! `paygate-kafkabus` — the only crate in the workspace that links `rdkafka`.
//!
//! A thin, real wrapper over Kafka for the relay (producer) and the ingester
//! (consumer), plus idempotent topic creation so any number of `worker relay`
//! replicas may start at once (`spec.md`, "Kafka" — the topic is created by
//! the relay, not by `backend/apitest/src/stack.rs`).
//!
//! Broker-side auto-creation is off (`KAFKA_AUTO_CREATE_TOPICS_ENABLE=false`),
//! so a consumer started before the topic exists must retry rather than fail —
//! [`Consumer::status`] reports `ready: false` until it both exists and this
//! consumer holds an assignment on it.

use std::collections::HashMap;
use std::time::Duration;

use rdkafka::admin::{AdminClient, AdminOptions, NewTopic, TopicReplication};
use rdkafka::client::DefaultClientContext;
use rdkafka::config::ClientConfig;
use rdkafka::consumer::{CommitMode, Consumer as _, StreamConsumer};
use rdkafka::error::{KafkaError as RdKafkaError, RDKafkaErrorCode};
use rdkafka::message::{BorrowedMessage, Message};
use rdkafka::producer::{FutureProducer, FutureRecord};
use rdkafka::TopicPartitionList;

/// Everything this crate's errors can be. Distinct from `rdkafka`'s own error
/// type so a caller (the worker binary) can match without depending on
/// `rdkafka` directly.
#[derive(Debug, thiserror::Error)]
pub enum KafkaError {
    #[error("kafka client error: {0}")]
    Client(#[from] RdKafkaError),
    #[error("timed out waiting for kafka")]
    Timeout,
    #[error("topic creation was refused: {0}")]
    TopicCreation(String),
}

/// The key a produced message carries. Always the payment id
/// (`spec.md`, "Kafka": "Key: `payment_id`"), so every event about one
/// payment lands on the same partition and is read back in order.
///
/// Generic over `Display` rather than pinned to `uuid::Uuid` — this crate
/// does not otherwise depend on the `uuid` crate (`Cargo.toml` lists none),
/// and a payment id's `Display` output is its canonical string form either
/// way.
pub fn message_key(payment_id: impl std::fmt::Display) -> String {
    payment_id.to_string()
}

fn base_client_config(brokers: &str) -> ClientConfig {
    let mut cfg = ClientConfig::new();
    cfg.set("bootstrap.servers", brokers);
    cfg
}

/// Idempotently create `topic` with `partitions` partitions (replication
/// factor 1 — this example runs a single-node broker), tolerating
/// `TopicAlreadyExists` so any number of `worker relay` replicas may race to
/// create it at startup (spec.md, "Kafka").
pub async fn ensure_topic(brokers: &str, topic: &str, partitions: i32) -> Result<(), KafkaError> {
    let admin: AdminClient<DefaultClientContext> = base_client_config(brokers)
        .create()
        .map_err(KafkaError::Client)?;
    let new_topic = NewTopic::new(topic, partitions, TopicReplication::Fixed(1));
    let opts = AdminOptions::new().request_timeout(Some(Duration::from_secs(10)));
    let results = admin
        .create_topics(std::iter::once(&new_topic), &opts)
        .await
        .map_err(KafkaError::Client)?;
    for result in results {
        match result {
            Ok(_) => {}
            Err((_topic, RDKafkaErrorCode::TopicAlreadyExists)) => {}
            Err((topic, code)) => {
                return Err(KafkaError::TopicCreation(format!("{topic}: {code:?}")))
            }
        }
    }
    Ok(())
}

/// A producer keyed by `payment_id`. One per relay replica; cheap to hold for
/// the process lifetime (`rdkafka`'s producer is internally a handle to a
/// background thread).
pub struct Producer {
    inner: FutureProducer,
    topic: String,
}

impl Producer {
    pub fn new(brokers: &str, topic: &str) -> Result<Self, KafkaError> {
        let mut cfg = base_client_config(brokers);
        cfg.set("message.timeout.ms", "30000");
        let inner: FutureProducer = cfg.create().map_err(KafkaError::Client)?;
        Ok(Self {
            inner,
            topic: topic.to_string(),
        })
    }

    /// Publish `payload` keyed by `payment_id`. Waits for the broker's
    /// acknowledgement (or `timeout`) so the relay only marks a row published
    /// once Kafka actually has it.
    pub async fn send(
        &self,
        payment_id: impl std::fmt::Display,
        payload: &[u8],
        timeout: Duration,
    ) -> Result<(), KafkaError> {
        let key = message_key(payment_id);
        let record = FutureRecord::to(&self.topic).key(&key).payload(payload);
        self.inner
            .send(record, timeout)
            .await
            .map_err(|(err, _msg)| KafkaError::Client(err))?;
        Ok(())
    }
}

/// What the ingester's consumer can report about itself, straight into
/// `GET /status` (`crates/worker/src/admin.rs`, `worker`'s admin port).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConsumerStatus {
    /// True once this consumer holds a partition assignment. False while the
    /// topic does not exist yet, or while a rebalance is in progress.
    pub ready: bool,
    /// Sum over every assigned partition of `high_watermark - committed`.
    /// Never negative per-partition: a partition with no committed offset
    /// yet counts its whole watermark as lag.
    pub lag: i64,
}

/// A consumer joined to `group_id`, reading `topic`. One per ingester
/// replica.
pub struct Consumer {
    inner: StreamConsumer,
}

impl Consumer {
    pub fn new(brokers: &str, group_id: &str, topic: &str) -> Result<Self, KafkaError> {
        let mut cfg = base_client_config(brokers);
        cfg.set("group.id", group_id)
            .set("enable.auto.commit", "false")
            .set("auto.offset.reset", "earliest")
            .set("session.timeout.ms", "10000");
        let inner: StreamConsumer = cfg.create().map_err(KafkaError::Client)?;
        rdkafka::consumer::Consumer::subscribe(&inner, &[topic]).map_err(KafkaError::Client)?;
        Ok(Self { inner })
    }

    /// Receive the next message, or `Timeout` if none arrived. The ingester
    /// batches on this (`INGEST_BATCH_MAX`, `INGEST_BATCH_WAIT_MS`).
    pub async fn recv(&self, timeout: Duration) -> Result<OwnedRecord, KafkaError> {
        let recv = self.inner.recv();
        tokio::pin!(recv);
        match tokio::time::timeout(timeout, recv).await {
            Ok(Ok(msg)) => Ok(OwnedRecord::from_message(&msg)),
            Ok(Err(err)) => Err(KafkaError::Client(err)),
            Err(_) => Err(KafkaError::Timeout),
        }
    }

    /// Commit up to and including `record`'s offset, synchronously — the
    /// ingester only commits after ClickHouse has the batch (spec.md, "The
    /// report's projection", "Never skips").
    pub fn commit(&self, record: &OwnedRecord) -> Result<(), KafkaError> {
        let mut tpl = TopicPartitionList::new();
        tpl.add_partition_offset(
            &record.topic,
            record.partition,
            rdkafka::Offset::Offset(record.offset + 1),
        )
        .map_err(KafkaError::Client)?;
        self.inner
            .commit(&tpl, CommitMode::Sync)
            .map_err(KafkaError::Client)
    }

    /// This consumer's own readiness and lag. Blocking `rdkafka` calls
    /// (`committed`, `fetch_watermarks`) run on a blocking thread so they
    /// never stall the async runtime.
    pub async fn status(&self, timeout: Duration) -> Result<ConsumerStatus, KafkaError> {
        let assignment =
            rdkafka::consumer::Consumer::assignment(&self.inner).map_err(KafkaError::Client)?;
        let partitions: Vec<(String, i32)> = assignment
            .elements()
            .iter()
            .map(|e| (e.topic().to_string(), e.partition()))
            .collect();
        if partitions.is_empty() {
            return Ok(ConsumerStatus {
                ready: false,
                lag: 0,
            });
        }

        let committed_list = self.inner.committed(timeout).map_err(KafkaError::Client)?;
        let mut committed = HashMap::new();
        for el in committed_list.elements() {
            if let rdkafka::Offset::Offset(o) = el.offset() {
                committed.insert((el.topic().to_string(), el.partition()), o);
            }
        }

        let mut watermarks = HashMap::new();
        for (topic, partition) in &partitions {
            let (_low, high) = self
                .inner
                .fetch_watermarks(topic, *partition, timeout)
                .map_err(KafkaError::Client)?;
            watermarks.insert((topic.clone(), *partition), high);
        }

        let lag = compute_lag(&partitions, &watermarks, &committed);
        Ok(ConsumerStatus { ready: true, lag })
    }
}

/// An owned copy of a received message: `rdkafka`'s [`BorrowedMessage`]
/// borrows from the consumer's internal buffer, which does not survive past
/// the poll that produced it, so the ingester needs its own copy to batch and
/// process asynchronously.
#[derive(Debug, Clone)]
pub struct OwnedRecord {
    pub topic: String,
    pub partition: i32,
    pub offset: i64,
    pub key: Option<Vec<u8>>,
    pub payload: Vec<u8>,
}

impl OwnedRecord {
    fn from_message(msg: &BorrowedMessage<'_>) -> Self {
        Self {
            topic: msg.topic().to_string(),
            partition: msg.partition(),
            offset: msg.offset(),
            key: msg.key().map(|k| k.to_vec()),
            payload: msg.payload().map(|p| p.to_vec()).unwrap_or_default(),
        }
    }
}

/// Pure lag arithmetic, factored out of [`Consumer::status`] so it is
/// testable without a broker: sum over every assigned partition of
/// `high_watermark - committed`, treating an uncommitted partition as fully
/// behind (lag = its whole watermark) and never letting one partition go
/// negative.
pub fn compute_lag(
    partitions: &[(String, i32)],
    watermarks: &HashMap<(String, i32), i64>,
    committed: &HashMap<(String, i32), i64>,
) -> i64 {
    partitions
        .iter()
        .map(|key| {
            let high = watermarks.get(key).copied().unwrap_or(0);
            let done = committed.get(key).copied().unwrap_or(0);
            (high - done).max(0)
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_key_is_the_payment_id() {
        let id = "0192a000-0000-7000-8000-0000000000a1";
        assert_eq!(message_key(id), id);
    }

    #[test]
    fn lag_is_the_sum_of_high_watermark_minus_committed_per_partition() {
        let partitions = vec![("t".to_string(), 0), ("t".to_string(), 1)];
        let mut watermarks = HashMap::new();
        watermarks.insert(("t".to_string(), 0), 100);
        watermarks.insert(("t".to_string(), 1), 50);
        let mut committed = HashMap::new();
        committed.insert(("t".to_string(), 0), 90);
        committed.insert(("t".to_string(), 1), 50);
        assert_eq!(compute_lag(&partitions, &watermarks, &committed), 10);
    }

    #[test]
    fn an_uncommitted_partition_counts_its_whole_watermark_as_lag() {
        let partitions = vec![("t".to_string(), 0)];
        let mut watermarks = HashMap::new();
        watermarks.insert(("t".to_string(), 0), 42);
        let committed = HashMap::new();
        assert_eq!(compute_lag(&partitions, &watermarks, &committed), 42);
    }

    #[test]
    fn lag_never_goes_negative_for_one_partition() {
        // A committed offset can briefly read ahead of a stale watermark
        // fetch; that must never make the total lag negative.
        let partitions = vec![("t".to_string(), 0)];
        let mut watermarks = HashMap::new();
        watermarks.insert(("t".to_string(), 0), 10);
        let mut committed = HashMap::new();
        committed.insert(("t".to_string(), 0), 15);
        assert_eq!(compute_lag(&partitions, &watermarks, &committed), 0);
    }

    #[test]
    fn no_assigned_partitions_is_zero_lag() {
        let partitions: Vec<(String, i32)> = vec![];
        assert_eq!(
            compute_lag(&partitions, &HashMap::new(), &HashMap::new()),
            0
        );
    }
}
