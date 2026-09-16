// The three steps spec/bdd/format.yml shares verbatim between the two surfaces.
//
// An API Background and an e2e Background seed with byte-identical text, which
// is only true because these three behave identically on both sides. Their other
// implementation is backend/src/test/scenario_ctx.go; where the two files agree
// on a detail — the sequence reset below, the row-count semantics — they agree
// on purpose, and a change to one is a change to both.

import { Given, Then } from '../fixtures'

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
  // Every SERIAL sequence is set past the largest id in its table afterwards.
  // The features seed rows with explicit ids (`INSERT INTO products (id, …)`),
  // which does not advance the sequence, so without this the first row the API
  // inserts would collide with a seeded id — a failure that says nothing about
  // the code under test.
  await world.db.query(RESET_SEQUENCES_SQL)
})

Then(/^in PostgreSQL query returns ([0-9]+) rows?:$/, async ({ world }, expected: string, sql: string) => {
  const query = world.resolveVars(sql)
  const count = await countRows(world.db.query.bind(world.db), query)
  assertRowCount(Number(expected), count, query)
})

type Query = (sql: string) => Promise<{ rowCount: number | null }>

async function countRows(query: Query, sql: string): Promise<number> {
  try {
    const result = await query(sql)
    return result.rowCount ?? 0
  } catch (error) {
    throw new Error(`query failed: ${describe(error)}\nSQL: ${sql}`)
  }
}

/**
 * The row-count semantics format.yml documents, identical to the Go side:
 * 0 asserts no matching record exists, 1 asserts exactly one, and n > 1 asserts
 * at least n. There is no ">= 1" form, because ">= 1" is the assertion that let
 * an empty table pass as a populated one.
 */
function assertRowCount(expected: number, actual: number, sql: string): void {
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

function describe(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
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
