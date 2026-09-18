package minimart.infrastructure.repository;

import javax.sql.DataSource;
import org.flywaydb.core.Flyway;

/**
 * Applies the schema migrations (DDL only, no seed data).
 *
 * <p>It is used by two callers on purpose: the service at boot when SCHEMA_AUTO_MIGRATE is true,
 * and the Cucumber-JVM runner's {@code run migration} step. The schema the tests assert against is
 * therefore produced by the same files and the same code path as the schema production runs on.
 */
public final class SchemaMigrator {

  /**
   * Where the migrations live: src/main/resources/db/migration, inside the jar. They are part of
   * the artifact rather than of the deployment, so there is no setting that could point the service
   * at a different set.
   */
  public static final String LOCATION = "classpath:db/migration";

  private SchemaMigrator() {}

  /**
   * Leaves the database at the latest version. Applying an already-current database is not an
   * error.
   *
   * <p>Clean is disabled — no code path in this service may drop a schema — and there is no
   * baseline: a database that already holds tables Flyway did not create is refused rather than
   * adopted, which is the same answer golang-migrate gives the Go service.
   *
   * <p>Flyway borrows a connection from {@code dataSource} and returns it; it never closes the
   * pool, so the service can go on serving from the same one.
   */
  public static void migrate(DataSource dataSource) {
    Flyway.configure()
        .dataSource(dataSource)
        .locations(LOCATION)
        .failOnMissingLocations(true)
        .cleanDisabled(true)
        .baselineOnMigrate(false)
        .load()
        .migrate();
  }
}
