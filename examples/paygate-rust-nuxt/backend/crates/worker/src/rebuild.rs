//! `worker rebuild reports`: the one-shot operational command. Truncates the
//! projection and its checkpoint and replays `payment_events` from
//! PostgreSQL — `paygate_infra::clickhouse::rebuild_reports` does the actual
//! work; this module's only job is to log the exact line
//! `backend/apitest/src/stack.rs` waits for on stdout.

use paygate_infra::clickhouse::{rebuild_reports, ClickHouseClient};
use paygate_infra::db::Db;
use paygate_infra::Result;

/// Runs the rebuild to completion and logs `rebuild complete` — the exact
/// line `backend/apitest/src/stack.rs` waits for on stdout — as a
/// structured JSON event like every other log line this binary writes
/// (`spec.md`, "Non-functional requirements": NFR-OBS-3, "Structured logs
/// without secrets"), which still satisfies the harness's
/// `stdout.contains("rebuild complete")` check.
pub async fn run(db: &Db, ch: &ClickHouseClient, batch_size: i64, instance_id: &str) -> Result<()> {
    let rows = rebuild_reports(db, ch, batch_size, instance_id).await?;
    tracing::info!(rows_written = rows, "rebuild complete");
    Ok(())
}
