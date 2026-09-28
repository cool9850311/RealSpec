//! The relay role: creates the Kafka topic idempotently at startup, then
//! polls [`paygate_infra::relay::run_once`], supplying the publish step from
//! `paygate_kafkabus::Producer::send` — the seam that keeps `infra` from
//! ever linking `rdkafka` (`crates/kafkabus/Cargo.toml` is the only crate
//! that does).

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;

use paygate_infra::config::KafkaConfig;
use paygate_infra::db::Db;
use paygate_infra::relay::{self, EventToPublish};
use paygate_kafkabus::{ensure_topic, KafkaError, Producer};

/// How long a single publish may take before the relay gives up on it for
/// this pass — generous compared to the producer's own `message.timeout.ms`
/// (30 s, set in `kafkabus::Producer::new`), since this is the caller's own
/// patience, not the broker's.
const SEND_TIMEOUT: Duration = Duration::from_secs(10);

/// How long to wait between retries while the topic does not exist yet or the
/// broker is not reachable — startup only, so a fixed interval is simpler
/// than a backoff and no less correct.
const STARTUP_RETRY: Duration = Duration::from_millis(500);

/// Creates the topic (tolerating `TopicAlreadyExists` so any number of relay
/// replicas may race, `spec.md`, "Kafka") and builds a producer, retrying
/// until both succeed or shutdown is requested. Returns `None` only when
/// shutdown fired first.
pub async fn ensure_topic_and_producer(
    cfg: &KafkaConfig,
    shutdown: &mut watch::Receiver<bool>,
) -> Option<Producer> {
    loop {
        if crate::shutdown::requested(shutdown) {
            return None;
        }
        match ensure_topic(&cfg.brokers, &cfg.topic, cfg.topic_partitions).await {
            Ok(()) => match Producer::new(&cfg.brokers, &cfg.topic) {
                Ok(producer) => return Some(producer),
                Err(err) => log_and_wait(err, "failed to create the kafka producer"),
            },
            Err(err) => log_and_wait(err, "ensure_topic failed"),
        }
        crate::shutdown::sleep_or_shutdown(STARTUP_RETRY, shutdown).await;
    }
}

fn log_and_wait(err: KafkaError, message: &str) {
    tracing::error!(error = %err, reason = message, "relay startup step failed; retrying");
}

/// Polls forever: publish a batch, and only sleep when a pass published
/// nothing — a full batch is drained without waiting out
/// `RELAY_POLL_INTERVAL_MS` between passes.
pub async fn poll_loop(
    db: Db,
    producer: Arc<Producer>,
    batch_size: u32,
    poll_interval: Duration,
    mut shutdown: watch::Receiver<bool>,
) {
    loop {
        if crate::shutdown::requested(&shutdown) {
            return;
        }
        let producer = producer.clone();
        let outcome = relay::run_once(&db, batch_size as i64, move |event: &EventToPublish| {
            let producer = producer.clone();
            let value = relay::kafka_value(event);
            let payment_id = event.payment_id;
            async move {
                producer
                    .send(payment_id, &value, SEND_TIMEOUT)
                    .await
                    .map_err(|e| e.to_string())
            }
        })
        .await;

        match outcome {
            Ok(n) if n > 0 => continue,
            Ok(_) => crate::shutdown::sleep_or_shutdown(poll_interval, &mut shutdown).await,
            Err(err) => {
                tracing::error!(error = %err, "relay pass failed");
                crate::shutdown::sleep_or_shutdown(poll_interval, &mut shutdown).await;
            }
        }
    }
}
