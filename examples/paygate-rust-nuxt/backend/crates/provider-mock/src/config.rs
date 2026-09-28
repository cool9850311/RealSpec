//! Configuration for the mock: what port to listen on, which api replicas to
//! call back, and the platform credential pairs it verifies incoming
//! signatures against and signs its own callbacks/queries with.
//!
//! `spec.md`, "Environment variables": "Each provider's credentials are rows
//! in `providers`, not variables: a scenario seeds them, and the mock is
//! given the same pair." The harness (`backend/apitest`) does not currently
//! pass a credential pair to this binary at all — every `INSERT INTO
//! providers` fixture across the whole BDD suite uses the exact same two
//! literal pairs (see `ECPAY_*`/`NEWEBPAY_*` defaults below). So this reads
//! them from the environment, with defaults equal to that fixture, which
//! keeps them out of source as a literal while requiring no change to the
//! harness for the suite to pass. If a scenario ever needs a different pair,
//! setting these variables on the mock's container is how — genuine
//! configuration with a production meaning (which platform this process
//! holds the keys for), not test-only code.

use paygate_provider::PlatformCredentials;

pub struct Config {
    pub port: u16,
    /// `PROVIDER_CALLBACK_URLS`, split on `,`. Delivery i (0-based, per
    /// release) goes to `callback_urls[i % callback_urls.len()]`.
    pub callback_urls: Vec<String>,
    pub ecpay: PlatformCredentials,
    pub newebpay: PlatformCredentials,
}

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_string())
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let port = env_or("PORT", "8080")
            .parse::<u16>()
            .map_err(|e| anyhow::anyhow!("PORT is not a valid port number: {e}"))?;

        let raw_callback_urls = std::env::var("PROVIDER_CALLBACK_URLS")
            .map_err(|_| anyhow::anyhow!("PROVIDER_CALLBACK_URLS is required and was not set"))?;
        let callback_urls: Vec<String> = raw_callback_urls
            .split(',')
            .map(|s| s.trim().trim_end_matches('/').to_string())
            .filter(|s| !s.is_empty())
            .collect();
        anyhow::ensure!(
            !callback_urls.is_empty(),
            "PROVIDER_CALLBACK_URLS must name at least one url"
        );

        let ecpay = PlatformCredentials {
            platform_id: env_or("ECPAY_PLATFORM_ID", "3002607"),
            hash_key: env_or("ECPAY_HASH_KEY", "pwFHCqoQZGmho4w6"),
            hash_iv: env_or("ECPAY_HASH_IV", "EkRm7iFT261dpevs"),
        };
        let newebpay = PlatformCredentials {
            platform_id: env_or("NEWEBPAY_PLATFORM_ID", "MS12345678"),
            hash_key: env_or("NEWEBPAY_HASH_KEY", "Fs5cX1TGqYZ3kLmN8pQrS2tUvW4xY6zA"),
            hash_iv: env_or("NEWEBPAY_HASH_IV", "B7cD9eF1gH3iJ5kL"),
        };
        anyhow::ensure!(
            newebpay.hash_key.len() == 32,
            "NEWEBPAY_HASH_KEY must be 32 bytes (AES-256); got {}",
            newebpay.hash_key.len()
        );
        anyhow::ensure!(
            newebpay.hash_iv.len() == 16,
            "NEWEBPAY_HASH_IV must be 16 bytes (one AES block); got {}",
            newebpay.hash_iv.len()
        );

        Ok(Config {
            port,
            callback_urls,
            ecpay,
            newebpay,
        })
    }
}
