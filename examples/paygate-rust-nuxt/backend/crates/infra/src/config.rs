//! Every environment variable `spec.md` -> "Environment variables" lists,
//! parsed and validated once at startup.
//!
//! Startup fails loudly (non-zero exit, one log line naming the variable)
//! when a required variable is missing or invalid. So every parser here
//! returns a [`ConfigError`] that names the variable, and never
//! panics — the binary is the one that turns an `Err` into a log line and a
//! non-zero exit, but the naming happens here, once, rather than being
//! reconstructed by every caller.
//!
//! Not every field below is used by every binary (`api`, `worker <role>`,
//! `provider-mock`, `demo-merchant` each read a subset), but a single struct
//! parsed from the process environment keeps the list of variables in one
//! place, which is what NFR-CFG-1 asks for.

use std::env;
use std::time::Duration;

use paygate_domain::validate_trade_no_prefix;

/// A configuration variable was missing or failed to parse/validate. Always
/// carries the variable's name — never logged with its value, since some of
/// these are secrets (`spec.md`, "Non-functional requirements": NFR-OBS-3).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid configuration for {variable}: {reason}")]
pub struct ConfigError {
    pub variable: &'static str,
    pub reason: String,
}

impl ConfigError {
    fn missing(variable: &'static str) -> Self {
        Self {
            variable,
            reason: "required but not set".to_string(),
        }
    }

    fn invalid(variable: &'static str, reason: impl Into<String>) -> Self {
        Self {
            variable,
            reason: reason.into(),
        }
    }
}

fn required(name: &'static str) -> Result<String, ConfigError> {
    match env::var(name) {
        Ok(v) if !v.is_empty() => Ok(v),
        _ => Err(ConfigError::missing(name)),
    }
}

/// Like [`required`], but an empty string is a legitimate VALUE rather than
/// a stand-in for "not set" — only `env::var` actually failing (the
/// variable absent from the process environment at all) is missing.
/// `std::env::VarError` cannot itself distinguish "set to empty" from "not
/// set" the way most shells can, so this is the one place that distinction
/// has to be made explicitly, and it is a narrow exception rather than a
/// change to `required`'s own default: most of this module's variables
/// really do treat "" as an omission (a DSN or a URL that is empty is
/// genuinely misconfigured), and loosening `required` itself would quietly
/// accept that too. Only a caller who has confirmed the field's own domain
/// treats an empty value as meaningful should reach for this instead.
fn required_allow_empty(name: &'static str) -> Result<String, ConfigError> {
    env::var(name).map_err(|_| ConfigError::missing(name))
}

fn optional(name: &'static str, default: &str) -> String {
    match env::var(name) {
        Ok(v) if !v.is_empty() => v,
        _ => default.to_string(),
    }
}

fn optional_u64(name: &'static str, default: u64) -> Result<u64, ConfigError> {
    match env::var(name) {
        Ok(v) if !v.is_empty() => v
            .parse::<u64>()
            .map_err(|e| ConfigError::invalid(name, e.to_string())),
        _ => Ok(default),
    }
}

fn optional_u32(name: &'static str, default: u32) -> Result<u32, ConfigError> {
    match env::var(name) {
        Ok(v) if !v.is_empty() => v
            .parse::<u32>()
            .map_err(|e| ConfigError::invalid(name, e.to_string())),
        _ => Ok(default),
    }
}

fn optional_bool(name: &'static str, default: bool) -> Result<bool, ConfigError> {
    match env::var(name) {
        Ok(v) if !v.is_empty() => match v.to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" => Ok(true),
            "false" | "0" | "no" => Ok(false),
            other => Err(ConfigError::invalid(
                name,
                format!("{other:?} is not a boolean"),
            )),
        },
        _ => Ok(default),
    }
}

/// A comma-separated list of milliseconds, e.g. `NOTIFY_BACKOFF_MS`'s default
/// `"50,100,200,400"`. Every entry must parse; an empty list is invalid,
/// because the notifier's schedule needs at least one delay to mean anything.
fn parse_ms_list(name: &'static str, raw: &str) -> Result<Vec<u64>, ConfigError> {
    let parts: Result<Vec<u64>, _> = raw.split(',').map(|s| s.trim().parse::<u64>()).collect();
    let parts = parts.map_err(|e| ConfigError::invalid(name, e.to_string()))?;
    if parts.is_empty() {
        return Err(ConfigError::invalid(name, "must list at least one delay"));
    }
    Ok(parts)
}

fn parse_url_list(name: &'static str, raw: &str) -> Result<Vec<String>, ConfigError> {
    let parts: Vec<String> = raw
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if parts.is_empty() {
        return Err(ConfigError::invalid(name, "must list at least one URL"));
    }
    Ok(parts)
}

/// Settings common to every long-running binary.
#[derive(Debug, Clone)]
pub struct CommonConfig {
    pub port: u16,
    pub instance_id: String,
    pub log_level: String,
    pub shutdown_grace: Duration,
}

impl CommonConfig {
    pub fn load() -> Result<Self, ConfigError> {
        let port = optional_u32("PORT", 8080)? as u16;
        let instance_id = optional("INSTANCE_ID", &hostname_fallback());
        let log_level = optional("LOG_LEVEL", "info");
        let shutdown_grace = Duration::from_secs(optional_u64("SHUTDOWN_GRACE_SECONDS", 10)?);
        Ok(Self {
            port,
            instance_id,
            log_level,
            shutdown_grace,
        })
    }
}

fn hostname_fallback() -> String {
    env::var("HOSTNAME").unwrap_or_else(|_| "paygate".to_string())
}

/// PostgreSQL settings (`api`, `relay`, `notifier`, and by extension every
/// worker role, since they all write through the same repo functions).
#[derive(Debug, Clone)]
pub struct DbConfig {
    pub dsn: String,
    pub pool_max: u32,
    pub schema_auto_migrate: bool,
}

impl DbConfig {
    pub fn load() -> Result<Self, ConfigError> {
        Ok(Self {
            dsn: required("DB_DSN")?,
            pool_max: optional_u32("DB_POOL_MAX", 20)?,
            schema_auto_migrate: optional_bool("SCHEMA_AUTO_MIGRATE", false)?,
        })
    }
}

/// Redis settings (`api`).
#[derive(Debug, Clone)]
pub struct RedisConfig {
    pub url: String,
    pub timeout: Duration,
}

impl RedisConfig {
    pub fn load() -> Result<Self, ConfigError> {
        Ok(Self {
            url: required("REDIS_URL")?,
            timeout: Duration::from_millis(optional_u64("REDIS_TIMEOUT_MS", 100)?),
        })
    }
}

/// ClickHouse settings (`api`, the ingester).
#[derive(Debug, Clone)]
pub struct ClickHouseConfig {
    pub url: String,
    pub database: String,
    pub user: String,
    pub password: String,
}

impl ClickHouseConfig {
    pub fn load() -> Result<Self, ConfigError> {
        Ok(Self {
            url: required("CLICKHOUSE_URL")?,
            database: required("CLICKHOUSE_DATABASE")?,
            user: required("CLICKHOUSE_USER")?,
            // Empty is a real, common value here, not an omission:
            // ClickHouse's own `default` user has no password out of the
            // box, and both `local/docker-compose.yml` and
            // `backend/apitest/src/stack.rs` set this to `""` on every
            // `api`, `ingest` and `rebuild` container on exactly that
            // basis. `required` would refuse that as "not set"; the
            // variable IS set, to the value ClickHouse itself expects.
            password: required_allow_empty("CLICKHOUSE_PASSWORD")?,
        })
    }
}

/// Kafka settings (relay, ingester) plus the topic-partitions addition
/// `spec.md`, "Environment variables" documents.
#[derive(Debug, Clone)]
pub struct KafkaConfig {
    pub brokers: String,
    pub topic: String,
    pub group_id: String,
    pub topic_partitions: i32,
}

impl KafkaConfig {
    pub fn load() -> Result<Self, ConfigError> {
        Ok(Self {
            brokers: required("KAFKA_BROKERS")?,
            topic: optional("KAFKA_TOPIC", "paygate.payment-events.v1"),
            group_id: optional("KAFKA_GROUP_ID", "paygate-reports"),
            topic_partitions: optional_u32("KAFKA_TOPIC_PARTITIONS", 3)? as i32,
        })
    }
}

/// The relay's own knobs.
#[derive(Debug, Clone)]
pub struct RelayConfig {
    pub batch_size: u32,
    pub poll_interval: Duration,
}

impl RelayConfig {
    pub fn load() -> Result<Self, ConfigError> {
        Ok(Self {
            batch_size: optional_u32("RELAY_BATCH_SIZE", 100)?,
            poll_interval: Duration::from_millis(optional_u64("RELAY_POLL_INTERVAL_MS", 50)?),
        })
    }
}

/// The ingester's own knobs.
#[derive(Debug, Clone)]
pub struct IngestConfig {
    pub batch_max: u32,
    pub batch_wait: Duration,
}

impl IngestConfig {
    pub fn load() -> Result<Self, ConfigError> {
        Ok(Self {
            batch_max: optional_u32("INGEST_BATCH_MAX", 500)?,
            batch_wait: Duration::from_millis(optional_u64("INGEST_BATCH_WAIT_MS", 200)?),
        })
    }
}

/// The notifier's own knobs.
#[derive(Debug, Clone)]
pub struct NotifyConfig {
    pub max_attempts: u32,
    /// The backoff schedule, in milliseconds. `spec.md`: "5 to 15 minutes, up
    /// to four more times the same day" is the production default; the
    /// suites override it to milliseconds. The Nth retry (1-indexed) uses
    /// `backoff_ms[(N-1).min(backoff_ms.len()-1)]`.
    pub backoff_ms: Vec<u64>,
    pub timeout: Duration,
    pub poll_interval: Duration,
    /// `spec.md`, "Environment variables": the origin a payment's path-only
    /// `notify_url` is resolved against.
    pub merchant_base_url: String,
}

impl NotifyConfig {
    pub fn load() -> Result<Self, ConfigError> {
        let backoff_raw = optional("NOTIFY_BACKOFF_MS", "50,100,200,400");
        Ok(Self {
            max_attempts: optional_u32("NOTIFY_MAX_ATTEMPTS", 5)?,
            backoff_ms: parse_ms_list("NOTIFY_BACKOFF_MS", &backoff_raw)?,
            timeout: Duration::from_millis(optional_u64("NOTIFY_TIMEOUT_MS", 2000)?),
            poll_interval: Duration::from_millis(optional_u64("NOTIFY_POLL_INTERVAL_MS", 50)?),
            merchant_base_url: required("MERCHANT_BASE_URL")?,
        })
    }
}

/// The reconciler's own knobs.
#[derive(Debug, Clone)]
pub struct ReconcileConfig {
    pub after: Duration,
    pub retry: Duration,
    /// `0` means "do not poll" (`spec.md`, "Environment variables") — the
    /// suites set this so that `the reconciler runs` (an operator/harness
    /// call to `POST /run`) is the only thing that makes paygate ask.
    pub poll_interval_ms: u64,
    pub batch_size: u32,
    /// `spec.md`, "Environment variables": the origin `providers.cashier_url`
    /// / `query_url` / `refund_url` are resolved against.
    pub provider_base_url: String,
}

impl ReconcileConfig {
    pub fn load() -> Result<Self, ConfigError> {
        Ok(Self {
            after: Duration::from_secs(60 * optional_u64("RECONCILE_AFTER_MINUTES", 60)?),
            retry: Duration::from_secs(60 * optional_u64("RECONCILE_RETRY_MINUTES", 60)?),
            poll_interval_ms: optional_u64("RECONCILE_POLL_INTERVAL_MS", 1000)?,
            batch_size: optional_u32("RECONCILE_BATCH_SIZE", 50)?,
            provider_base_url: required("PROVIDER_BASE_URL")?,
        })
    }
}

/// `api`'s own knobs, beyond the shared DB/Redis/ClickHouse settings.
#[derive(Debug, Clone)]
pub struct ApiConfig {
    pub provider_trade_no_prefix: String,
    pub public_base_url: String,
    /// Refunds only; there is no synchronous charge (`spec.md`, "Environment
    /// variables").
    pub psp_timeout: Duration,
    pub session_ttl: Duration,
    pub api_key_cache_ttl: Duration,
    pub idempotency_ttl: Duration,
    pub idempotency_lock_ttl: Duration,
    pub cookie_secure: bool,
    pub frontend_origin: String,
}

impl ApiConfig {
    pub fn load() -> Result<Self, ConfigError> {
        let prefix = required("PROVIDER_TRADE_NO_PREFIX")?;
        validate_trade_no_prefix(&prefix)
            .map_err(|e| ConfigError::invalid("PROVIDER_TRADE_NO_PREFIX", e.to_string()))?;
        Ok(Self {
            provider_trade_no_prefix: prefix,
            public_base_url: required("PUBLIC_BASE_URL")?,
            psp_timeout: Duration::from_millis(optional_u64("PSP_TIMEOUT_MS", 3000)?),
            session_ttl: Duration::from_secs(optional_u64("SESSION_TTL_SECONDS", 28800)?),
            api_key_cache_ttl: Duration::from_secs(optional_u64("API_KEY_CACHE_TTL_SECONDS", 60)?),
            idempotency_ttl: Duration::from_secs(3600 * optional_u64("IDEMPOTENCY_TTL_HOURS", 24)?),
            idempotency_lock_ttl: Duration::from_millis(optional_u64(
                "IDEMPOTENCY_LOCK_TTL_MS",
                30000,
            )?),
            cookie_secure: optional_bool("COOKIE_SECURE", true)?,
            frontend_origin: required("FRONTEND_ORIGIN")?,
        })
    }
}

/// `demo-merchant`'s own settings.
#[derive(Debug, Clone)]
pub struct DemoMerchantConfig {
    pub gateway_url: String,
    pub api_key: String,
    pub hash_key: String,
    pub hash_iv: String,
}

impl DemoMerchantConfig {
    pub fn load() -> Result<Self, ConfigError> {
        Ok(Self {
            gateway_url: required("GATEWAY_URL")?,
            api_key: required("DEMO_MERCHANT_API_KEY")?,
            hash_key: required("MERCHANT_HASH_KEY")?,
            hash_iv: required("MERCHANT_HASH_IV")?,
        })
    }
}

/// `provider-mock`'s own settings.
#[derive(Debug, Clone)]
pub struct ProviderMockConfig {
    /// Delivery `i` goes to entry `i mod n` (`spec.md`, "Environment
    /// variables").
    pub callback_urls: Vec<String>,
}

impl ProviderMockConfig {
    pub fn load() -> Result<Self, ConfigError> {
        let raw = required("PROVIDER_CALLBACK_URLS")?;
        Ok(Self {
            callback_urls: parse_url_list("PROVIDER_CALLBACK_URLS", &raw)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Environment variables are process-global state; serialise the tests
    // that touch them so one test's `set_var` cannot leak into another's
    // `load()`. A single-threaded critical section per test, not per crate.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn with_env<F: FnOnce()>(vars: &[(&str, &str)], f: F) {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        for (k, _) in vars {
            std::env::remove_var(k);
        }
        for (k, v) in vars {
            std::env::set_var(k, v);
        }
        f();
        for (k, _) in vars {
            std::env::remove_var(k);
        }
    }

    #[test]
    fn common_config_uses_documented_defaults() {
        with_env(&[], || {
            let cfg = CommonConfig::load().unwrap();
            assert_eq!(cfg.port, 8080);
            assert_eq!(cfg.log_level, "info");
            assert_eq!(cfg.shutdown_grace, Duration::from_secs(10));
        });
    }

    #[test]
    fn db_config_requires_dsn() {
        with_env(&[], || {
            let err = DbConfig::load().unwrap_err();
            assert_eq!(err.variable, "DB_DSN");
        });
        with_env(&[("DB_DSN", "postgres://x")], || {
            let cfg = DbConfig::load().unwrap();
            assert_eq!(cfg.dsn, "postgres://x");
            assert_eq!(cfg.pool_max, 20);
            assert!(!cfg.schema_auto_migrate);
        });
    }

    #[test]
    fn clickhouse_password_may_be_set_to_the_empty_string() {
        // ClickHouse's own `default` user has no password out of the box,
        // and both `local/docker-compose.yml` and `backend/apitest` set
        // exactly this on every container that talks to it — an empty
        // value here is a real configuration, not an omission.
        with_env(
            &[
                ("CLICKHOUSE_URL", "http://clickhouse:8123"),
                ("CLICKHOUSE_DATABASE", "paygate"),
                ("CLICKHOUSE_USER", "default"),
                ("CLICKHOUSE_PASSWORD", ""),
            ],
            || {
                let cfg = ClickHouseConfig::load().unwrap();
                assert_eq!(cfg.password, "");
            },
        );
    }

    #[test]
    fn clickhouse_password_absent_entirely_still_fails_loudly() {
        with_env(
            &[
                ("CLICKHOUSE_URL", "http://clickhouse:8123"),
                ("CLICKHOUSE_DATABASE", "paygate"),
                ("CLICKHOUSE_USER", "default"),
            ],
            || {
                std::env::remove_var("CLICKHOUSE_PASSWORD");
                let err = ClickHouseConfig::load().unwrap_err();
                assert_eq!(err.variable, "CLICKHOUSE_PASSWORD");
            },
        );
    }

    #[test]
    fn notify_backoff_parses_the_documented_default_list() {
        with_env(&[("MERCHANT_BASE_URL", "http://merchant")], || {
            let cfg = NotifyConfig::load().unwrap();
            assert_eq!(cfg.backoff_ms, vec![50, 100, 200, 400]);
            assert_eq!(cfg.max_attempts, 5);
        });
    }

    #[test]
    fn notify_backoff_rejects_a_non_numeric_entry() {
        with_env(
            &[
                ("MERCHANT_BASE_URL", "http://merchant"),
                ("NOTIFY_BACKOFF_MS", "50,oops,400"),
            ],
            || {
                let err = NotifyConfig::load().unwrap_err();
                assert_eq!(err.variable, "NOTIFY_BACKOFF_MS");
            },
        );
    }

    #[test]
    fn notify_backoff_rejects_an_empty_list() {
        with_env(
            &[
                ("MERCHANT_BASE_URL", "http://merchant"),
                ("NOTIFY_BACKOFF_MS", ""),
            ],
            || {
                // Empty env var falls back to the default rather than being
                // treated as present-but-empty, matching `required`'s own
                // "empty means absent" rule used throughout this module.
                let cfg = NotifyConfig::load().unwrap();
                assert_eq!(cfg.backoff_ms, vec![50, 100, 200, 400]);
            },
        );
    }

    #[test]
    fn reconcile_poll_interval_zero_is_a_legal_value_meaning_do_not_poll() {
        with_env(
            &[
                ("PROVIDER_BASE_URL", "http://provider"),
                ("RECONCILE_POLL_INTERVAL_MS", "0"),
            ],
            || {
                let cfg = ReconcileConfig::load().unwrap();
                assert_eq!(cfg.poll_interval_ms, 0);
            },
        );
    }

    #[test]
    fn api_config_validates_the_trade_no_prefix_at_startup() {
        with_env(
            &[
                ("PROVIDER_TRADE_NO_PREFIX", "TOOLONG1"),
                ("PUBLIC_BASE_URL", "http://paygate"),
                ("FRONTEND_ORIGIN", "http://front"),
            ],
            || {
                let err = ApiConfig::load().unwrap_err();
                assert_eq!(err.variable, "PROVIDER_TRADE_NO_PREFIX");
            },
        );
        with_env(
            &[
                ("PROVIDER_TRADE_NO_PREFIX", "ACME1"),
                ("PUBLIC_BASE_URL", "http://paygate"),
                ("FRONTEND_ORIGIN", "http://front"),
            ],
            || {
                let cfg = ApiConfig::load().unwrap();
                assert_eq!(cfg.provider_trade_no_prefix, "ACME1");
                assert!(cfg.cookie_secure);
            },
        );
    }

    #[test]
    fn provider_mock_callback_urls_split_on_comma() {
        with_env(
            &[(
                "PROVIDER_CALLBACK_URLS",
                "http://api-1,http://api-2, http://api-3",
            )],
            || {
                let cfg = ProviderMockConfig::load().unwrap();
                assert_eq!(
                    cfg.callback_urls,
                    vec!["http://api-1", "http://api-2", "http://api-3"]
                );
            },
        );
    }

    #[test]
    fn a_boolean_variable_rejects_a_nonsense_value() {
        with_env(&[("COOKIE_SECURE", "maybe")], || {
            let err = optional_bool("COOKIE_SECURE", true).unwrap_err();
            assert_eq!(err.variable, "COOKIE_SECURE");
        });
    }
}
