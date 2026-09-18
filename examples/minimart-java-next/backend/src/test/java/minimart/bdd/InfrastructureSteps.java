package minimart.bdd;

import io.cucumber.docstring.DocString;
import io.cucumber.java.en.Given;
import io.cucumber.java.en.Then;
import java.sql.ResultSet;
import java.sql.SQLException;
import java.sql.Statement;
import minimart.infrastructure.repository.SchemaMigrator;

/**
 * The three infrastructure steps of spec/bdd/format.yml: run_migration, exec_postgresql and
 * postgresql_query_returns. They are shared verbatim with the e2e surface, which implements them in
 * frontend/tests/e2e; these are the API surface's.
 *
 * <p>Every pattern below is the registry's {@code pattern} field copied character for character:
 * the registry is the grammar, and a runner that paraphrased it would be a second, disagreeing
 * grammar. StepRegistryParityTest holds them to that.
 */
public final class InfrastructureSteps {

  /**
   * Sets every SERIAL sequence past the largest id in its table — the same statement go-nuxt runs.
   * setval(seq, n, false) makes nextval() return n.
   */
  static final String RESET_SEQUENCES_SQL =
      """

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
      END $$;""";

  private final ScenarioContext context;

  public InfrastructureSteps(ScenarioContext context) {
    this.context = context;
  }

  /**
   * {@code run migration}: applies backend/src/main/resources/db/migration to this scenario's
   * database through the very method the service calls when SCHEMA_AUTO_MIGRATE is true, so the
   * schema the features assert against is the schema production gets.
   */
  @Given("^run migration$")
  public void runMigration() {
    try {
      SchemaMigrator.migrate(context.dataSource());
    } catch (RuntimeException e) {
      throw new AssertionError("run migration: " + e.getMessage(), e);
    }
  }

  /**
   * {@code in PostgreSQL:} — the docstring is run as one raw SQL batch outside any explicit
   * transaction.
   *
   * <p>Afterwards every SERIAL sequence is set past the largest id in its table. The features seed
   * rows with explicit ids ({@code INSERT INTO products (id, …)}), which does not advance the
   * sequence, so without this the first row the API inserts would collide with a seeded id — a
   * failure that says nothing about the code under test.
   */
  @Given("^in PostgreSQL:$")
  public void execPostgres(DocString doc) {
    String sql = context.resolve(doc.getContent());
    try {
      execute(sql);
    } catch (SQLException e) {
      throw new AssertionError("exec SQL failed: " + e.getMessage() + "\nSQL: " + sql, e);
    }
    try {
      execute(RESET_SEQUENCES_SQL);
    } catch (SQLException e) {
      throw new AssertionError("reset sequences failed: " + e.getMessage(), e);
    }
  }

  private void execute(String sql) throws SQLException {
    try (Statement statement = context.db().createStatement()) {
      // The docstring is SQL, not JDBC: a brace in it is PostgreSQL's to read, not an escape
      // sequence for the driver to rewrite.
      statement.setEscapeProcessing(false);
      statement.execute(sql);
    }
  }

  /**
   * {@code in PostgreSQL query returns <n> rows:} — 0 asserts no matching record exists, 1 asserts
   * exactly one, and n > 1 asserts at least n: the semantics format.yml documents.
   */
  @Then("^in PostgreSQL query returns ([0-9]+) rows?:$")
  public void postgresqlQueryReturns(String rowCount, DocString doc) {
    int expected = Integer.parseInt(rowCount);
    String query = context.resolve(doc.getContent());
    int count = countRows(query);
    if (expected == 0 && count != 0) {
      throw new AssertionError("expected 0 rows, got " + count + "\nSQL: " + query);
    }
    if (expected == 1 && count != 1) {
      throw new AssertionError("expected exactly 1 row, got " + count + "\nSQL: " + query);
    }
    if (expected > 1 && count < expected) {
      throw new AssertionError(
          "expected at least " + expected + " rows, got " + count + "\nSQL: " + query);
    }
  }

  private int countRows(String query) {
    try (Statement statement = context.db().createStatement()) {
      statement.setEscapeProcessing(false);
      try (ResultSet rows = statement.executeQuery(query)) {
        int count = 0;
        while (rows.next()) {
          count++;
        }
        return count;
      }
    } catch (SQLException e) {
      throw new AssertionError("query failed: " + e.getMessage() + "\nSQL: " + query, e);
    }
  }
}
