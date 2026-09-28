//! Shared: Infrastructure Setup, PostgreSQL, and Projections.

use std::time::Duration;

use cucumber::gherkin::Step;
use cucumber::{given, then, when};

use crate::world::World;

/// Every serial sequence, bumped past `max(id)` in its table, so rows the
/// service inserts later never collide with explicitly seeded ids. Run after
/// `in PostgreSQL:` seeds rows with explicit ids, which does not itself
/// advance any sequence.
const RESET_SEQUENCES_SQL: &str = r"
DO $$
DECLARE r RECORD;
BEGIN
    FOR r IN
        SELECT c.table_name, c.column_name,
               pg_get_serial_sequence(c.table_name, c.column_name) AS seq
          FROM information_schema.columns c
         WHERE c.table_schema = 'public'
           AND c.column_default LIKE 'nextval%'
    LOOP
        CONTINUE WHEN r.seq IS NULL;
        EXECUTE format(
            'SELECT setval(%L, COALESCE((SELECT MAX(%I)+1 FROM %I), 1), false)',
            r.seq, r.column_name, r.table_name
        );
    END LOOP;
END $$;";

fn sql_files(dir: std::path::PathBuf) -> anyhow::Result<Vec<std::path::PathBuf>> {
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .map_err(|e| anyhow::anyhow!("reading {dir:?}: {e}"))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("sql"))
        .collect();
    files.sort();
    Ok(files)
}

/// Splits a file's content into individual statements on top-level `;`
/// boundaries. Good enough for the DDL these migrations carry; not a general
/// SQL parser.
/// Splits a ClickHouse migration into statements ClickHouse will accept.
///
/// ClickHouse's HTTP interface takes one statement per request, so the file has
/// to be split on `;`. The subtlety is the last piece: a migration that ends with
/// an explanatory comment leaves a fragment that is *only* comment, and
/// ClickHouse answers that with `Code: 62 … Empty query` — a failure caused
/// entirely by prose. So a statement counts only if something survives stripping
/// its `--` comment lines, and the statement itself is still sent whole, comments
/// included, because they are worth having in the server's query log.
fn split_statements(sql: &str) -> Vec<String> {
    sql.split(';')
        .map(str::trim)
        .filter(|stmt| !is_only_comment(stmt))
        .map(str::to_string)
        .collect()
}

/// True when nothing but `--` comments and whitespace is left.
fn is_only_comment(stmt: &str) -> bool {
    stmt.lines()
        .map(str::trim)
        .all(|line| line.is_empty() || line.starts_with("--"))
}

/// `run migration` — applies every migration in `backend/migrations/postgres`
/// and `backend/migrations/clickhouse`, in that order. DDL only.
#[given(regex = r"^run migration$")]
#[when(regex = r"^run migration$")]
#[then(regex = r"^run migration$")]
async fn run_migration(world: &mut World) {
    apply_migrations(world)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
}

async fn apply_migrations(world: &mut World) -> anyhow::Result<()> {
    let stack = world.stack();
    let pg_dir = crate::registry::spec_root().join("backend/migrations/postgres");
    for file in sql_files(pg_dir)? {
        let sql = std::fs::read_to_string(&file)?;
        stack
            .pg
            .batch_execute(&sql)
            .await
            .map_err(|e| anyhow::anyhow!("postgres migration {file:?} failed: {e}"))?;
    }

    let ch_dir = crate::registry::spec_root().join("backend/migrations/clickhouse");
    let ch_url = stack.clickhouse_base_url();
    for file in sql_files(ch_dir)? {
        let content = std::fs::read_to_string(&file)?;
        for stmt in split_statements(&content) {
            let resp = stack.http.post(&ch_url).body(stmt.clone()).send().await?;
            if !resp.status().is_success() {
                let body = resp.text().await.unwrap_or_default();
                anyhow::bail!(
                    "clickhouse migration {file:?} statement failed: {body}\nSQL: {stmt}"
                );
            }
        }
    }
    Ok(())
}

/// `in PostgreSQL:` — runs the docstring as one statement batch, then resets
/// every serial sequence.
#[given(regex = r"^in PostgreSQL:$")]
#[when(regex = r"^in PostgreSQL:$")]
#[then(regex = r"^in PostgreSQL:$")]
async fn exec_postgresql(world: &mut World, step: &Step) {
    exec_postgresql_impl(world, step)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
}

async fn exec_postgresql_impl(world: &mut World, step: &Step) -> anyhow::Result<()> {
    let doc = super::docstring_body(step)
        .ok_or_else(|| anyhow::anyhow!("`in PostgreSQL:` needs a docstring"))?;
    let sql = world.resolve(doc)?;
    let stack = world.stack();
    stack.pg.batch_execute(&sql).await.map_err(|e| {
        anyhow::anyhow!(
            "exec SQL failed: {}\nSQL: {sql}",
            crate::steps::pg_error(&e)
        )
    })?;
    stack
        .pg
        .batch_execute(RESET_SEQUENCES_SQL)
        .await
        .map_err(|e| anyhow::anyhow!("resetting sequences failed: {e}"))?;
    Ok(())
}

/// `in PostgreSQL query returns <n> rows:` — 0 = none, 1 = exactly one,
/// n > 1 = at least n.
#[given(regex = r"^in PostgreSQL query returns ([0-9]+) rows?:$")]
#[when(regex = r"^in PostgreSQL query returns ([0-9]+) rows?:$")]
#[then(regex = r"^in PostgreSQL query returns ([0-9]+) rows?:$")]
async fn postgresql_query_returns(world: &mut World, expected: i64, step: &Step) {
    postgresql_query_returns_impl(world, expected, step)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
}

async fn postgresql_query_returns_impl(
    world: &mut World,
    expected: i64,
    step: &Step,
) -> anyhow::Result<()> {
    let doc = super::docstring_body(step)
        .ok_or_else(|| anyhow::anyhow!("`in PostgreSQL query returns` needs a docstring"))?;
    let sql = world.resolve(doc)?;
    let trimmed = sql.trim().trim_end_matches(';');
    let rows =
        world.stack().pg.query(trimmed, &[]).await.map_err(|e| {
            anyhow::anyhow!("query failed: {}\nSQL: {sql}", crate::steps::pg_error(&e))
        })?;
    let count = rows.len() as i64;
    match expected {
        0 => anyhow::ensure!(count == 0, "expected 0 rows, got {count}\nSQL: {sql}"),
        1 => anyhow::ensure!(
            count == 1,
            "expected exactly 1 row, got {count}\nSQL: {sql}"
        ),
        n => anyhow::ensure!(
            count >= n,
            "expected at least {n} rows, got {count}\nSQL: {sql}"
        ),
    }
    Ok(())
}

/// `in ClickHouse query returns <n> rows:` — EXACT, for every n.
#[given(regex = r"^in ClickHouse query returns ([0-9]+) rows?:$")]
#[when(regex = r"^in ClickHouse query returns ([0-9]+) rows?:$")]
#[then(regex = r"^in ClickHouse query returns ([0-9]+) rows?:$")]
async fn clickhouse_query_returns(world: &mut World, expected: i64, step: &Step) {
    clickhouse_query_returns_impl(world, expected, step)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
}

async fn clickhouse_query_returns_impl(
    world: &mut World,
    expected: i64,
    step: &Step,
) -> anyhow::Result<()> {
    let doc = super::docstring_body(step)
        .ok_or_else(|| anyhow::anyhow!("`in ClickHouse query returns` needs a docstring"))?;
    let sql = world.resolve(doc)?;
    let trimmed = sql.trim().trim_end_matches(';');
    let stack = world.stack();
    let query = format!("{trimmed} FORMAT JSONEachRow");
    let resp = stack
        .http
        .post(stack.clickhouse_base_url())
        .body(query.clone())
        .send()
        .await?;
    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("clickhouse query failed: {body}\nSQL: {sql}");
    }
    let body = resp.text().await?;
    let count = body.lines().filter(|l| !l.trim().is_empty()).count() as i64;
    anyhow::ensure!(
        count == expected,
        "expected exactly {expected} row(s), got {count}\nSQL: {sql}\nRows:\n{body}"
    );
    Ok(())
}

/// `background work has settled` — polled every 100ms for up to 30s.
#[given(regex = r"^background work has settled$")]
#[when(regex = r"^background work has settled$")]
#[then(regex = r"^background work has settled$")]
async fn background_work_settled(world: &mut World) {
    background_work_settled_impl(world)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
}

async fn background_work_settled_impl(world: &mut World) -> anyhow::Result<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let stack = world.stack();

        let unpublished: i64 = stack
            .pg
            .query_one(
                "SELECT count(*) FROM payment_events WHERE published_at IS NULL",
                &[],
            )
            .await?
            .get(0);

        let mut ingesters_ready = true;
        let mut lag_report = Vec::new();
        for inst in &stack.ingester.instances {
            let url = format!("{}/status", inst.base_url());
            let ok = match stack.http.get(&url).send().await {
                Ok(resp) => match resp.json::<serde_json::Value>().await {
                    Ok(json) => {
                        let ready = json.get("ready").and_then(|v| v.as_bool()).unwrap_or(false);
                        let lag = json.get("lag").and_then(|v| v.as_i64()).unwrap_or(-1);
                        lag_report.push(format!("{}: ready={ready} lag={lag}", inst.name));
                        ready && lag == 0
                    }
                    Err(_) => false,
                },
                Err(_) => false,
            };
            if !ok {
                ingesters_ready = false;
            }
        }

        let pending_notifications: i64 = stack
            .pg
            .query_one(
                "SELECT count(*) FROM notifications WHERE delivered_at IS NULL AND exhausted_at IS NULL",
                &[],
            )
            .await?
            .get(0);

        if unpublished == 0 && ingesters_ready && pending_notifications == 0 {
            return Ok(());
        }

        if tokio::time::Instant::now() >= deadline {
            anyhow::bail!(
                "background work did not settle within 30s: {unpublished} unpublished audit row(s), \
                 ingesters [{}], {pending_notifications} notification(s) still due",
                lag_report.join(", ")
            );
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// `projection "reports" is rebuilt from the event log` — a one-shot
/// `paygate-worker rebuild reports` container, waited for with `WaitFor::Exit`.
#[given(regex = r#"^projection "(reports)" is rebuilt from the event log$"#)]
#[when(regex = r#"^projection "(reports)" is rebuilt from the event log$"#)]
#[then(regex = r#"^projection "(reports)" is rebuilt from the event log$"#)]
async fn projection_rebuilt(world: &mut World, _projection: String) {
    world
        .stack()
        .rebuild_reports()
        .await
        .unwrap_or_else(|e| panic!("rebuilding the reports projection failed: {e}"));
}

#[cfg(test)]
mod tests {
    use super::{is_only_comment, split_statements};

    #[test]
    fn a_trailing_comment_is_not_a_statement() {
        // This is the shape of backend/migrations/clickhouse/0001_init.sql: a
        // CREATE TABLE, then a closing note explaining where the checkpoint
        // lives. Before this, that note was sent to ClickHouse as a query and
        // came back as `Code: 62 … Empty query`.
        let sql = "-- a leading note\nCREATE TABLE t (a UInt8) ENGINE = Memory;\n\n-- a closing note\n-- over two lines\n";
        let stmts = split_statements(sql);
        assert_eq!(stmts.len(), 1);
        assert!(stmts[0].contains("CREATE TABLE t"));
        // The comment above the statement travels with it.
        assert!(stmts[0].contains("a leading note"));
    }

    #[test]
    fn only_comment_is_recognised() {
        assert!(is_only_comment(""));
        assert!(is_only_comment("   \n  -- just a note\n"));
        assert!(!is_only_comment("-- a note\nSELECT 1"));
    }
}
