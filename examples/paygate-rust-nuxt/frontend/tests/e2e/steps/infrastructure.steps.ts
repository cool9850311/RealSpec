// The eight steps spec/bdd/format.yml shares verbatim between the API surface
// (cucumber-rs, backend/apitest) and this one: an API Background and an e2e
// Background seed and assert with byte-identical text, which is only true
// because these eight behave identically on both sides.
//
// Three are minimart's own three infrastructure steps, unchanged
// (run_migration, exec_postgresql, postgresql_query_returns — see
// examples/minimart-go-nuxt/frontend/tests/e2e/steps/infrastructure.steps.ts).
// The other five are what a payment gateway needs and minimart did not: a
// projection that only arrives later, an external provider, and a merchant
// with its own delivery log. Their behaviour lives in stack.ts (this scenario's
// own ClickHouse, the ingester's `/status`, the mock's and the merchant's own
// logs) so it can be read in one place; these definitions are thin.

import { Given, Then } from '../fixtures'

// ── run migration / in PostgreSQL: / in PostgreSQL query returns ────────────
// (format.yml: identical wording and semantics to the API surface's Go/Rust
// implementation of the same three steps.)

Given(/^run migration$/, async ({ world }) => {
  await world.stack.migrate()
})

Given(/^in PostgreSQL:$/, async ({ world }, sql: string) => {
  const statement = world.resolveVars(sql)
  try {
    await world.db.query(statement)
  } catch (error) {
    throw new Error(`in PostgreSQL: failed: ${describe(error)}\nSQL: ${statement}`)
  }
  // Every SERIAL/BIGSERIAL sequence is set past the largest id in its table
  // afterwards, because the features seed rows with explicit ids
  // (`payment_events.id`, `notifications.id`), which does not advance the
  // sequence — without this the first row the api or a worker inserts would
  // collide with a seeded id.
  await world.db.query(RESET_SEQUENCES_SQL)
})

Then(/^in PostgreSQL query returns ([0-9]+) rows?:$/, async ({ world }, expected: string, sql: string) => {
  const query = world.resolveVars(sql)
  const count = await countRows(world, query)
  assertPostgresRowCount(Number(expected), count, query)
})

async function countRows(world: { db: { query(sql: string): Promise<{ rowCount: number | null }> } }, sql: string): Promise<number> {
  try {
    const result = await world.db.query(sql)
    return result.rowCount ?? 0
  } catch (error) {
    throw new Error(`query failed: ${describe(error)}\nSQL: ${sql}`)
  }
}

/**
 * format.yml's PostgreSQL row-count semantics: 0 asserts no matching record
 * exists, 1 asserts exactly one, and n > 1 asserts AT LEAST n.
 */
function assertPostgresRowCount(expected: number, actual: number, sql: string): void {
  if (expected === 0 && actual !== 0) {
    throw new Error(`expected 0 rows, got ${actual}\nSQL: ${sql}`)
  }
  if (expected === 1 && actual !== 1) {
    throw new Error(`expected exactly 1 row, got ${actual}\nSQL: ${sql}`)
  }
  if (expected > 1 && actual < expected) {
    throw new Error(`expected at least ${expected} rows, got ${actual}\nSQL: ${sql}`)
  }
}

const RESET_SEQUENCES_SQL = `
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
        -- setval(seq, n, false) makes nextval() return n.
        EXECUTE format(
            'SELECT setval(%L, COALESCE((SELECT MAX(%I)+1 FROM %I), 1), false)',
            r.seq, r.column_name, r.table_name
        );
    END LOOP;
END $$;`

// ── background work has settled ──────────────────────────────────────────────
//
// format.yml: three conditions at once — no unpublished payment_events, every
// ingester caught up, no notification still due — polled every 100 ms for up
// to 30 s. The condition itself, and the failure message naming which of the
// three is still in flight, live in stack.ts (`backgroundWorkSettled`) so the
// same logic backs whichever step reads it next; this definition just calls it.

Then(/^background work has settled$/, async ({ world }) => {
  await world.stack.backgroundWorkSettled()
})

// ── in ClickHouse query returns <n> rows: ────────────────────────────────────
//
// Unlike `in PostgreSQL query returns`, this count is EXACT for every n — a
// projection is exactly where "at least" hides a duplicate.

Then(/^in ClickHouse query returns ([0-9]+) rows?:$/, async ({ world }, expected: string, sql: string) => {
  const query = world.resolveVars(sql)
  let rows: unknown[]
  try {
    rows = await world.stack.clickhouseQuery(query)
  } catch (error) {
    throw new Error(`ClickHouse query failed: ${describe(error)}\nSQL: ${query}`)
  }
  const want = Number(expected)
  if (rows.length !== want) {
    throw new Error(`expected exactly ${want} rows, got ${rows.length}\nSQL: ${query}`)
  }
})

// ── projection "reports" is rebuilt from the event log ──────────────────────

Given(/^projection "(reports)" is rebuilt from the event log$/, async ({ world }, _projection: string) => {
  void _projection
  await world.stack.rebuildReportsProjection()
})

// ── payment provider received <n> checkout|refund requests ─────────────────

Then(
  /^payment provider received ([0-9]+) (checkout|refund) requests?$/,
  async ({ world }, expected: string, kind: string) => {
    const want = Number(expected)
    const actual = await world.stack.paymentProviderReceived(kind as 'checkout' | 'refund')
    if (actual !== want) {
      throw new Error(`payment provider received ${actual} ${kind} request(s), expected exactly ${want}`)
    }
  },
)

// ── merchant received <n> notifications at "<path>" ─────────────────────────

Then(
  /^merchant received ([0-9]+) notifications? at "(\/demo-merchant\/api\/[a-z0-9\/-]+)"$/,
  async ({ world }, expected: string, notifyPath: string) => {
    const want = Number(expected)
    const actual = await world.stack.merchantReceived(notifyPath)
    if (actual !== want) {
      throw new Error(
        `merchant received ${actual} notification(s) at "${notifyPath}", expected exactly ${want}`,
      )
    }
  },
)

function describe(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}
