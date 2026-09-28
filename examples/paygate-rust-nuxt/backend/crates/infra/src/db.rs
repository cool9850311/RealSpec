//! The PostgreSQL connection pool. Thin on purpose: every actual query lives
//! in [`crate::repo`], `crate::relay`, `crate::notifier` or
//! `crate::reconciler`; this module only owns getting a [`sqlx::PgPool`]
//! connected and, optionally, migrated.

use sqlx::postgres::{PgPoolOptions, Postgres};
use sqlx::{Pool, Transaction};

use crate::error::Result;

/// The database handle every repo function takes a pool or a transaction
/// from. A thin newtype rather than a bare `PgPool` alias so other crates
/// (`api`, `worker`) have one obvious type to hold and pass around.
#[derive(Clone)]
pub struct Db(pub Pool<Postgres>);

impl Db {
    /// Connect with up to `pool_max` connections. Runtime queries only (no
    /// compile-time DB), so connecting does not require the database to
    /// match any schema known at compile time.
    pub async fn connect(dsn: &str, pool_max: u32) -> Result<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(pool_max)
            .connect(dsn)
            .await?;
        Ok(Self(pool))
    }

    pub fn pool(&self) -> &Pool<Postgres> {
        &self.0
    }

    /// Runs the migrations embedded from `backend/migrations/postgres` when
    /// `SCHEMA_AUTO_MIGRATE` is set (`config::DbConfig::schema_auto_migrate`).
    /// The suites leave it false and issue `run migration` as its own step
    /// (`spec.md`, "Environment variables"); a real deployment sets it once
    /// and lets the first replica up apply the schema.
    pub async fn migrate(&self) -> Result<()> {
        sqlx::migrate!("../../migrations/postgres")
            .run(&self.0)
            .await
            .map_err(|e| crate::error::Error::Database(sqlx::Error::Migrate(Box::new(e))))
    }

    /// A `SELECT 1` used by `GET /health/ready` (`spec/openapi/openapi.yaml`).
    pub async fn ping(&self) -> bool {
        sqlx::query("SELECT 1").execute(&self.0).await.is_ok()
    }

    pub async fn begin(&self) -> Result<Transaction<'_, Postgres>> {
        Ok(self.0.begin().await?)
    }
}
