//! Every environment variable this service reads, validated once at startup
//! (`spec.md` → "Environment variables"; "Credentials in this repository":
//! every secret is read from the environment at startup, with no default and
//! no fallback).
//!
//! `GATEWAY_URL`, `DEMO_MERCHANT_API_KEY`, `MERCHANT_HASH_KEY` and
//! `MERCHANT_HASH_IV` are spec.md's own `demo-merchant` row. `PORT`,
//! `INSTANCE_ID`, `LOG_LEVEL` and `SHUTDOWN_GRACE_SECONDS` are the row shared
//! by every service. `NOTIFY_TIMEOUT_MS` is spec.md's own notifier setting,
//! read here too — undocumented for this service specifically, but
//! unavoidable: `/notify-slow` must hold the connection open past the exact
//! duration the notifier is configured to wait, or the scenario that depends
//! on it (`notify.feature`, "An answer that arrives after the notifier gave
//! up waiting is not an answer") is timing a value nobody told this process.
//! This is an addition beyond this service's own row, the same way
//! `crates/infra/src/config.rs`'s `KAFKA_TOPIC_PARTITIONS` is one.

use std::env;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct Config {
    pub port: u16,
    pub instance_id: String,
    pub log_level: String,
    pub shutdown_grace: Duration,
    pub gateway_url: String,
    pub api_key: String,
    pub merchant_hash_key: String,
    pub merchant_hash_iv: String,
    /// See the module doc: shared with the notifier's own setting so
    /// `/notify-slow` can answer just past it.
    pub notify_timeout: Duration,
}

#[derive(Debug)]
pub struct ConfigError(pub String);

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for ConfigError {}

fn required(name: &str) -> Result<String, ConfigError> {
    match env::var(name) {
        Ok(value) if !value.is_empty() => Ok(value),
        _ => Err(ConfigError(format!("{name} is required"))),
    }
}

fn parse_or_default<T: std::str::FromStr>(name: &str, default: T) -> Result<T, ConfigError> {
    match env::var(name) {
        Ok(value) if !value.is_empty() => value
            .parse::<T>()
            .map_err(|_| ConfigError(format!("{name} is invalid"))),
        _ => Ok(default),
    }
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        let port = parse_or_default("PORT", 8080u16)?;
        let instance_id = env::var("INSTANCE_ID")
            .ok()
            .filter(|v| !v.is_empty())
            .or_else(|| env::var("HOSTNAME").ok())
            .unwrap_or_else(|| "demo-merchant".to_string());
        let log_level = env::var("LOG_LEVEL")
            .ok()
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| "info".to_string());
        let shutdown_grace_secs = parse_or_default("SHUTDOWN_GRACE_SECONDS", 10u64)?;

        let gateway_url = required("GATEWAY_URL")?;
        let api_key = required("DEMO_MERCHANT_API_KEY")?;
        let merchant_hash_key = required("MERCHANT_HASH_KEY")?;
        let merchant_hash_iv = required("MERCHANT_HASH_IV")?;

        let notify_timeout_ms = parse_or_default("NOTIFY_TIMEOUT_MS", 2000u64)?;

        Ok(Config {
            port,
            instance_id,
            log_level,
            shutdown_grace: Duration::from_secs(shutdown_grace_secs),
            gateway_url: gateway_url.trim_end_matches('/').to_string(),
            api_key,
            merchant_hash_key,
            merchant_hash_iv,
            notify_timeout: Duration::from_millis(notify_timeout_ms),
        })
    }
}
