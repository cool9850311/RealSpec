-- paygate — PostgreSQL schema.
--
-- Source of truth for this file is spec/spec.md -> "Data model" -> "PostgreSQL",
-- cross-checked column by column against every SELECT/INSERT/UPDATE in
-- spec/bdd/api/*.feature, since those queries run against this schema and a
-- name that differs from what a feature file expects is a failure there, not
-- here.
--
-- No table has a column that could hold a card number or a CVC — the
-- customer types the card at the provider's own origin and never at paygate
-- (spec.md, "The card never enters this system") — and no column name
-- starts with `checkout` (spec/bdd/api/orders.feature asserts both against
-- information_schema). No column anywhere is added "just in case"; every
-- one here is read or written somewhere in spec.md or the features.
--
-- Runnable as one statement batch, in dependency order (a table only
-- references one already created above it). No seed data: every fixture in
-- the BDD suite inserts its own merchants, providers and api_keys.

-- ---------------------------------------------------------------------------
-- A note on what is NOT here: foreign keys.
--
-- spec.md's Data model enumerates every constraint on every table and explains
-- each one; not one of them is a foreign key, and that is deliberate rather than
-- an omission. Two of the specification's own fixtures prove it:
--
--   · every Background in spec/bdd/api/*.feature seeds `merchants` BEFORE
--     `providers`, in separate statements outside any transaction, so a
--     `merchants.provider_code -> providers.code` reference cannot be satisfied
--     at insert time;
--   · spec/bdd/api/reports.feature and spec/bdd/e2e/reports.feature seed
--     `payment_events` as HISTORY — the registry's `exec_postgresql` says so in
--     as many words — with payment ids that have no `payments` row at all,
--     because what the report's projection is built from is the audit table and
--     nothing else.
--
-- So the integrity these tables actually promise is enforced where the spec puts
-- it: the UNIQUE constraints, the CHECKs, and the row locks named in
-- "Concurrency". A foreign key here would not make the system safer — every
-- write goes through one of `repo`'s transactions, which read the parent row
-- under a lock before writing the child — and it would make the specification's
-- own seed data unloadable, which is a worse failure than the one it prevents.
-- ---------------------------------------------------------------------------

-- ---------------------------------------------------------------------------
-- providers — paygate's own PLATFORM credentials at ECPay/NewebPay. This
-- pair signs for every merchant (spec.md, "Platform, not merchant of
-- record"); a merchant's own hash pair, below, is unrelated and signs only
-- what paygate sends THAT merchant.
-- ---------------------------------------------------------------------------
CREATE TABLE providers (
    code        TEXT PRIMARY KEY,
    platform_id TEXT NOT NULL,
    hash_key    TEXT NOT NULL,
    hash_iv     TEXT NOT NULL,
    cashier_url TEXT NOT NULL,
    query_url   TEXT NOT NULL,
    refund_url  TEXT NOT NULL
);

-- ---------------------------------------------------------------------------
-- merchants — provider_code and provider_merchant_id are NOT NULL on
-- purpose: a merchant that cannot be paid is not a merchant here, so
-- onboarding writes this row only once the provider account exists
-- (spec.md, "Data model").
-- ---------------------------------------------------------------------------
CREATE TABLE merchants (
    id                    BIGSERIAL PRIMARY KEY,
    name                  TEXT NOT NULL,
    currency              TEXT NOT NULL,
    timezone              TEXT NOT NULL,
    provider_code         TEXT NOT NULL,
    provider_merchant_id  TEXT NOT NULL,
    duplicate_auto_refund BOOLEAN NOT NULL DEFAULT TRUE,
    rate_limit_per_minute INTEGER NOT NULL,
    hash_key              TEXT NOT NULL,
    hash_iv               TEXT NOT NULL
);

-- ---------------------------------------------------------------------------
-- api_keys — raw keys are never stored, only their SHA-256 (spec.md,
-- "Authentication"; CONTRACT.md rule 6).
-- ---------------------------------------------------------------------------
CREATE TABLE api_keys (
    id          BIGSERIAL PRIMARY KEY,
    merchant_id BIGINT NOT NULL,
    key_hash    CHAR(64) NOT NULL,
    key_prefix  TEXT NOT NULL,
    revoked_at  TIMESTAMPTZ,
    UNIQUE (key_hash)
);

-- ---------------------------------------------------------------------------
-- dashboard_users — staff logins for the merchant's own dashboard.
-- ---------------------------------------------------------------------------
CREATE TABLE dashboard_users (
    id            BIGSERIAL PRIMARY KEY,
    merchant_id   BIGINT NOT NULL,
    email         TEXT NOT NULL,
    password_hash TEXT NOT NULL,
    UNIQUE (email)
);

-- ---------------------------------------------------------------------------
-- payments — the order. Only three states: no `processing`, because
-- paygate cannot know whether anybody is paying; that fact lives on the
-- attempt (spec.md, "Events").
-- ---------------------------------------------------------------------------
CREATE TABLE payments (
    id                UUID PRIMARY KEY,
    merchant_id       BIGINT NOT NULL,
    merchant_trade_no TEXT NOT NULL,
    amount            BIGINT NOT NULL,
    currency          TEXT NOT NULL,
    status            TEXT NOT NULL,
    item_desc         TEXT NOT NULL,
    card_brand        TEXT,
    card_last4        TEXT,
    notify_url        TEXT NOT NULL,
    client_back_url   TEXT NOT NULL,
    amount_refunded   BIGINT NOT NULL DEFAULT 0,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (merchant_id, merchant_trade_no),
    CHECK (amount BETWEEN 1 AND 99999999),
    CHECK (status IN ('pending', 'succeeded', 'refunded')),
    CHECK (amount_refunded BETWEEN 0 AND amount)
);

-- The merchant's own reads, newest first (spec.md, "Data model").
CREATE INDEX payments_merchant_created_at_idx ON payments (merchant_id, created_at DESC);

-- ---------------------------------------------------------------------------
-- payment_attempts — one per hand-off, never reused, ever. ECPay's own
-- twenty-character mixed-alphanumeric rule is enforced here too, not only
-- by the generator, so a path that forgot the check fails instead of
-- corrupting (spec.md, "Concurrency").
-- ---------------------------------------------------------------------------
CREATE TABLE payment_attempts (
    id                 UUID PRIMARY KEY,
    payment_id         UUID NOT NULL,
    provider_code      TEXT NOT NULL,
    provider_trade_no  TEXT NOT NULL,
    status             TEXT NOT NULL,
    rtn_code           INTEGER,
    simulate_paid      BOOLEAN NOT NULL DEFAULT FALSE,
    failure_code       TEXT,
    provider_charge_id TEXT,
    card_brand         TEXT,
    card_last4         TEXT,
    eci                TEXT,
    auth_code          TEXT,
    started_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    queried_at         TIMESTAMPTZ,
    settled_at         TIMESTAMPTZ,
    UNIQUE (provider_code, provider_trade_no),
    CHECK (provider_trade_no ~ '^[A-Za-z0-9]{1,20}$'),
    CHECK (status IN ('redirected', 'succeeded', 'failed', 'abandoned'))
);

CREATE INDEX payment_attempts_payment_id_idx ON payment_attempts (payment_id);

-- The reconciler's queue: only ever attempts still waiting on a callback.
CREATE INDEX payment_attempts_redirected_started_at_idx
    ON payment_attempts (started_at)
    WHERE status = 'redirected';

-- ---------------------------------------------------------------------------
-- payment_events — the append-only audit log and, via published_at, the
-- outbox the relay publishes from (spec.md, "Two tables, one transaction").
-- ---------------------------------------------------------------------------
CREATE TABLE payment_events (
    id            BIGSERIAL PRIMARY KEY,
    event_id      UUID NOT NULL,
    payment_id    UUID NOT NULL,
    merchant_id   BIGINT NOT NULL,
    seq           INTEGER NOT NULL,
    event_type    TEXT NOT NULL,
    payload       JSONB NOT NULL,
    occurred_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    published_at  TIMESTAMPTZ,
    publish_count INTEGER NOT NULL DEFAULT 0,
    UNIQUE (event_id),
    UNIQUE (payment_id, seq)
);

-- The relay's queue.
CREATE INDEX payment_events_unpublished_idx ON payment_events (id) WHERE published_at IS NULL;

-- ---------------------------------------------------------------------------
-- refunds — id is also the number sent to the provider as its own
-- deduplication key (spec.md, "Refunds").
-- ---------------------------------------------------------------------------
CREATE TABLE refunds (
    id                 UUID PRIMARY KEY,
    payment_id         UUID NOT NULL,
    attempt_id         UUID NOT NULL,
    merchant_id        BIGINT NOT NULL,
    amount             BIGINT NOT NULL,
    status             TEXT NOT NULL,
    reason             TEXT,
    provider_refund_id TEXT,
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (amount > 0),
    CHECK (status IN ('pending', 'succeeded'))
);

CREATE INDEX refunds_payment_id_idx ON refunds (payment_id);

-- ---------------------------------------------------------------------------
-- notifications — one row per outcome owed to a merchant, written in the
-- same transaction as the outcome itself (spec.md, "Telling the merchant").
-- ---------------------------------------------------------------------------
CREATE TABLE notifications (
    id              BIGSERIAL PRIMARY KEY,
    payment_id      UUID NOT NULL,
    merchant_id     BIGINT NOT NULL,
    url             TEXT NOT NULL,
    payload         JSONB NOT NULL,
    attempts        INTEGER NOT NULL DEFAULT 0,
    next_attempt_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    delivered_at    TIMESTAMPTZ,
    exhausted_at    TIMESTAMPTZ,
    last_status     INTEGER,
    last_body       TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- The notifier's queue.
CREATE INDEX notifications_due_idx
    ON notifications (next_attempt_at)
    WHERE delivered_at IS NULL AND exhausted_at IS NULL;

-- ---------------------------------------------------------------------------
-- provider_events — one row per thing that arrived from a provider,
-- whatever it turned out to be. The primary key is the callback
-- deduplication itself (spec.md, "Being told by the provider").
-- ---------------------------------------------------------------------------
CREATE TABLE provider_events (
    provider_code TEXT NOT NULL,
    event_id      TEXT NOT NULL,
    payment_id    UUID NOT NULL,
    attempt_id    UUID NOT NULL,
    rtn_code      INTEGER,
    outcome       TEXT NOT NULL,
    raw           JSONB NOT NULL,
    received_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (provider_code, event_id),
    CHECK (outcome IN ('applied', 'no_op', 'conflict', 'duplicate', 'amount_mismatch', 'unknown_code'))
);

-- ---------------------------------------------------------------------------
-- provider_queries — what the reconciler asked, and what it was told. How
-- "paygate did not guess" becomes a fact somebody can check (spec.md,
-- "Reconciling what never came back").
-- ---------------------------------------------------------------------------
CREATE TABLE provider_queries (
    id           BIGSERIAL PRIMARY KEY,
    attempt_id   UUID NOT NULL,
    asked_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    trade_status TEXT,
    raw          TEXT
);

CREATE INDEX provider_queries_attempt_id_idx ON provider_queries (attempt_id);

-- ---------------------------------------------------------------------------
-- idempotency_keys — "is this the same REQUEST, sent twice?" (spec.md,
-- "Two kinds of duplicate"). The primary key is (merchant_id,
-- idempotency_key): one merchant's key never collides with another's.
-- ---------------------------------------------------------------------------
CREATE TABLE idempotency_keys (
    merchant_id         BIGINT NOT NULL,
    idempotency_key     TEXT NOT NULL,
    request_fingerprint TEXT NOT NULL,
    state               TEXT NOT NULL,
    response_status     INTEGER,
    response_body       JSONB,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at          TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (merchant_id, idempotency_key)
);

-- ---------------------------------------------------------------------------
-- projection_checkpoints — read by `worker rebuild reports` and at startup
-- (spec.md, "Data model").
-- ---------------------------------------------------------------------------
CREATE TABLE projection_checkpoints (
    projection    TEXT PRIMARY KEY,
    last_event_id UUID,
    last_position BIGINT NOT NULL DEFAULT 0,
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);
