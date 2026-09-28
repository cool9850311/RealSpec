//! Shared fixtures for `crates/infra`'s integration tests. Every test starts
//! its own container, through `testcontainers` (the library owns the
//! lifecycle end to end — start, readiness, and Ryuk's cleanup on drop; no
//! `docker run`, no hand-assigned port, no `sleep`-then-poll), the same rule
//! `spec.md`, "Isolation" states for the BDD suites.

#![allow(dead_code)]

use paygate_domain::{Amount, MerchantId};
use paygate_infra::Db;
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, ImageExt};
use testcontainers_modules::clickhouse::ClickHouse;
use testcontainers_modules::postgres::Postgres;
use testcontainers_modules::redis::Redis;

/// A running PostgreSQL container, migrated, with one merchant (id 1,
/// `rate_limit_per_minute = 600`) and its `ecpay` provider row seeded — the
/// minimum every repo function needs to have somewhere to write.
pub struct PgFixture {
    pub _container: ContainerAsync<Postgres>,
    pub db: Db,
}

pub const MERCHANT_ID: MerchantId = 1;
pub const PROVIDER_HASH_KEY: &str = "pwFHCqoQZGmho4w6";
pub const PROVIDER_HASH_IV: &str = "EkRm7iFT261dpevs";
pub const MERCHANT_HASH_KEY: &str = "acmehashkey0123456789abcdef01234";
pub const MERCHANT_HASH_IV: &str = "acmehashiv012345";

pub async fn start_postgres() -> PgFixture {
    let container = Postgres::default()
        .with_tag("17-alpine")
        .start()
        .await
        .expect("postgres container starts");
    let host = container.get_host().await.expect("host");
    let port = container
        .get_host_port_ipv4(5432)
        .await
        .expect("mapped port");
    let dsn = format!("postgres://postgres:postgres@{host}:{port}/postgres");

    let db = Db::connect(&dsn, 10).await.expect("connect to postgres");
    db.migrate().await.expect("run migrations");

    seed_merchant_and_provider(&db, MERCHANT_ID).await;

    PgFixture {
        _container: container,
        db,
    }
}

pub async fn seed_merchant_and_provider(db: &Db, merchant_id: MerchantId) {
    sqlx::query(
        "INSERT INTO providers (code, platform_id, hash_key, hash_iv, cashier_url, query_url, refund_url) \
         VALUES ('ecpay', '3002607', $1, $2, \
                 '/provider/ecpay/Cashier/AioCheckOut/V5', \
                 '/provider/ecpay/Cashier/QueryTradeInfo/V5', \
                 '/provider/ecpay/CreditDetail/DoAction') \
         ON CONFLICT (code) DO NOTHING",
    )
    .bind(PROVIDER_HASH_KEY)
    .bind(PROVIDER_HASH_IV)
    .execute(db.pool())
    .await
    .expect("seed provider");

    sqlx::query(
        "INSERT INTO merchants \
            (id, name, currency, timezone, provider_code, provider_merchant_id, \
             rate_limit_per_minute, hash_key, hash_iv) \
         VALUES ($1, 'Acme Coffee', 'USD', 'Asia/Taipei', 'ecpay', '2000132', 600, $2, $3) \
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(merchant_id)
    .bind(MERCHANT_HASH_KEY)
    .bind(MERCHANT_HASH_IV)
    .execute(db.pool())
    .await
    .expect("seed merchant");
}

pub fn merchant_context(payment_merchant_trade_no: &str) -> paygate_infra::repo::MerchantContext {
    paygate_infra::repo::MerchantContext {
        merchant_id: MERCHANT_ID,
        provider_merchant_id: "2000132".to_string(),
        merchant_trade_no: payment_merchant_trade_no.to_string(),
        notify_url: "/demo-merchant/api/notify".to_string(),
    }
}

/// A fully-formed, verified-shaped `CallbackFacts` for a `Paid` callback
/// against `attempt`'s own trade number, for `amount`.
pub fn paid_facts(
    provider_trade_no: &str,
    event_id: &str,
    amount: Amount,
) -> paygate_provider::CallbackFacts {
    paygate_provider::CallbackFacts {
        provider_trade_no: provider_trade_no.to_string(),
        provider_event_id: event_id.to_string(),
        provider_charge_id: format!("ch_{event_id}"),
        rtn_code: 1,
        rtn_msg: "Succeeded".to_string(),
        amount,
        simulate_paid: false,
        failure_code: None,
        card_brand: Some("visa".to_string()),
        card_last4: Some("4242".to_string()),
        eci: None,
        auth_code: None,
        paid_at: Some(chrono::Utc::now()),
    }
}

pub fn failed_facts(
    provider_trade_no: &str,
    event_id: &str,
    amount: Amount,
    rtn_code: i32,
) -> paygate_provider::CallbackFacts {
    paygate_provider::CallbackFacts {
        provider_trade_no: provider_trade_no.to_string(),
        provider_event_id: event_id.to_string(),
        provider_charge_id: String::new(),
        rtn_code,
        rtn_msg: "Declined".to_string(),
        amount,
        simulate_paid: false,
        failure_code: None,
        card_brand: None,
        card_last4: None,
        eci: None,
        auth_code: None,
        paid_at: None,
    }
}

/// A running Redis container, plus a connected `RedisStore`.
pub struct RedisFixture {
    pub container: ContainerAsync<Redis>,
    pub store: paygate_infra::redis_store::RedisStore,
}

pub async fn start_redis() -> RedisFixture {
    // `spec.md`, "Redis — nothing that matters": "No persistence." Disable
    // RDB snapshotting and the AOF explicitly — the default `redis` image
    // otherwise performs a SAVE on the SIGTERM `ContainerAsync::stop()`
    // sends, and reloads that snapshot on the next `start()`, which would
    // make a stopped-and-restarted container silently remember the rate
    // limiter's bucket instead of forgetting it. Production's own compose
    // file makes the same choice for the same reason; this just makes it
    // explicit here too, since `testcontainers_modules::redis::Redis`'s
    // default command does not.
    let container = Redis::default()
        .with_tag("7-alpine")
        .with_cmd(["redis-server", "--save", "", "--appendonly", "no"])
        .start()
        .await
        .expect("redis container starts");
    let store = connect_redis(&container).await;
    RedisFixture { container, store }
}

pub async fn connect_redis(
    container: &ContainerAsync<Redis>,
) -> paygate_infra::redis_store::RedisStore {
    let host = container.get_host().await.expect("host");
    let port = container
        .get_host_port_ipv4(testcontainers_modules::redis::REDIS_PORT)
        .await
        .expect("mapped port");
    let url = format!("redis://{host}:{port}");
    paygate_infra::redis_store::RedisStore::connect(&url)
        .await
        .expect("connect to redis")
}

/// A running ClickHouse container, with `report_events` created from the
/// real migration file — the same schema `worker rebuild reports` and the
/// ingester write against in production, not a hand-copied approximation.
pub struct ClickHouseFixture {
    pub _container: ContainerAsync<ClickHouse>,
    pub client: paygate_infra::clickhouse::ClickHouseClient,
}

const CLICKHOUSE_MIGRATION: &str = include_str!("../../../../migrations/clickhouse/0001_init.sql");

const CLICKHOUSE_PASSWORD: &str = "clickhouse-test";

pub async fn start_clickhouse() -> ClickHouseFixture {
    let container = ClickHouse::default()
        .with_tag("24.8-alpine")
        .with_ulimit("nofile", 262_144, Some(262_144))
        // Recent official images generate a random `default` user password
        // when none is given (and only log it), so `default`/no-password
        // is refused. Pin one explicitly, the same way `start_postgres`
        // pins its own.
        .with_env_var("CLICKHOUSE_PASSWORD", CLICKHOUSE_PASSWORD)
        .start()
        .await
        .expect("clickhouse container starts");
    let host = container.get_host().await.expect("host");
    let port = container
        .get_host_port_ipv4(testcontainers_modules::clickhouse::CLICKHOUSE_PORT)
        .await
        .expect("mapped port");
    let client = paygate_infra::clickhouse::ClickHouseClient::new(
        &format!("http://{host}:{port}"),
        "default",
        "default",
        CLICKHOUSE_PASSWORD,
    );
    client
        .execute_ddl(CLICKHOUSE_MIGRATION)
        .await
        .expect("create report_events");

    ClickHouseFixture {
        _container: container,
        client,
    }
}
