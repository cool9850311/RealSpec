-- paygate — ClickHouse schema.
--
-- Source of truth is spec/spec.md -> "Data model" -> "ClickHouse" and
-- "The report's projection". One wide table, `report_events`: one row per
-- reportable event (`EventType::is_projected()` in crates/domain decides
-- which audit rows the ingester forwards here at all), built by the
-- ingester from Kafka.
--
-- `ReplacingMergeTree(ingested_at)` keyed on `event_id` is what makes
-- at-least-once delivery safe to project: a duplicate row for the same
-- `event_id` (a relay retry, a re-ingested Kafka offset) is deduplicated by
-- keeping the one with the greatest `ingested_at`, which is why every read
-- in this system queries `report_events FINAL` rather than the raw table —
-- a materialised view fires before deduplication and would count the
-- duplicate the pipeline is allowed to produce.
--
-- `PARTITION BY toYYYYMM(occurred_at)` and `ORDER BY (merchant_id, event_id)`
-- are both load-bearing: the merchant is always the report's first filter,
-- and a rebuild (`worker rebuild reports`) truncates and replays this table
-- from PostgreSQL's `payment_events` — never from Kafka, whose retention is
-- a setting rather than a record.

CREATE TABLE IF NOT EXISTS report_events
(
    event_id      UUID,
    seq           UInt32,
    merchant_id   UInt64,
    payment_id    UUID,
    event_type    LowCardinality(String),
    provider_code LowCardinality(String),
    amount        Int64,
    currency      LowCardinality(String),
    card_brand    LowCardinality(String),
    failure_code  LowCardinality(String),
    reference     String,
    refund_id     UUID,
    occurred_at   DateTime64(3, 'UTC'),
    ingested_at   DateTime64(3, 'UTC'),
    ingester_id   String
)
ENGINE = ReplacingMergeTree(ingested_at)
PARTITION BY toYYYYMM(occurred_at)
ORDER BY (merchant_id, event_id);

-- "Plus whatever the checkpoint needs" (CONTRACT.md §Task 3): the ingester's
-- checkpoint itself is `projection_checkpoints` in PostgreSQL — spec.md's
-- own Data model places it there, "read by a rebuild and at startup" — not a
-- second table here. What ClickHouse's own schema needs for it is
-- `ingester_id` above: which ingester wrote a row, so a rebuild (which
-- truncates this table and PostgreSQL's checkpoint together, then replays
-- `payment_events`) can tell its own output apart from a previous run's
-- without a second store to reconcile against.
