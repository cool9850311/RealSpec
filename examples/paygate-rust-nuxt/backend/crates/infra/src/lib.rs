//! `paygate-infra` — PostgreSQL, Redis, ClickHouse, and the worker logic
//! built on top of them. Everything here is real I/O; the pure decision
//! logic each module needs (a callback's outcome, the notifier's backoff,
//! the rate limiter's arithmetic, ...) is factored into its own testable
//! function rather than hidden inside a query, per this crate's own unit
//! test list in `spec.md`, "Test plan" -> "Unit tests, by crate" -> `infra`.

pub mod circuit_breaker;
pub mod clickhouse;
pub mod config;
pub mod db;
pub mod error;
pub mod log;
pub mod notifier;
pub mod rate_limiter;
pub mod reconciler;
pub mod redis_store;
pub mod relay;
pub mod repo;

pub use db::Db;
pub use error::{Error, Result};
