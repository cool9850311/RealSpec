package minimart.bdd;

import java.net.http.HttpClient;
import java.nio.charset.StandardCharsets;
import java.sql.Connection;
import java.sql.DriverManager;
import java.sql.SQLException;
import java.time.Duration;
import java.time.Instant;
import java.util.ArrayList;
import java.util.Collections;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.TreeMap;
import java.util.regex.Matcher;
import java.util.regex.Pattern;
import javax.sql.DataSource;
import org.springframework.jdbc.datasource.DriverManagerDataSource;
import org.testcontainers.containers.GenericContainer;
import org.testcontainers.containers.Network;
import org.testcontainers.containers.wait.strategy.Wait;
import org.testcontainers.postgresql.PostgreSQLContainer;
import org.testcontainers.utility.DockerImageName;

/**
 * One scenario's world: its containers, its database connection, its HTTP client and the state the
 * assertion steps read. PicoContainer creates exactly one per scenario and hands the same instance
 * to {@link Hooks} and to every step class, so nothing here is shared between scenarios except the
 * service image, which is built once in {@link Hooks#buildServiceImage()}.
 *
 * <p>Isolation, as in go-nuxt: every scenario gets its own Docker network, its own PostgreSQL and
 * its own copy of the service under test. A PostgreSQL cold start on tmpfs is well under a second,
 * which is cheaper than the class of bug that leaking one scenario's rows into the next produces.
 *
 * <p>The client keeps no cookie jar. A scenario carries identity by writing {@code "Cookie":
 * "token={someVar}"} into the request docstring, every request, with the token named by a {@code
 * save response cookie} under a login the feature wrote out, or by one of the four pre-minted
 * credentials {@link Credentials} puts in every scenario's bag. One mechanism, visible where it is
 * used.
 */
public final class ScenarioContext {

  /** The tag {@link Hooks#buildServiceImage()} builds backend/Dockerfile into. */
  static final String SERVICE_IMAGE = "minimart-java-backend:cucumber";

  /** The same image local/docker-compose.yml uses. */
  static final String POSTGRES_IMAGE = "postgres:16-alpine";

  static final String DB_NAME = "minimart";
  static final String DB_USER = "minimart";
  static final String DB_PASSWORD = "minimart_secret";

  /** How long to wait for the mapped PostgreSQL port to accept a connection. */
  static final Duration DB_READY_TIMEOUT = Duration.ofSeconds(30);

  static final Duration DB_READY_POLL_INTERVAL = Duration.ofMillis(100);

  /**
   * How long a container may take to become ready. A Spring Boot service needs a few seconds where
   * the Go binary needed milliseconds, and parallel scenarios share the CPU while they boot.
   */
  static final Duration STARTUP_TIMEOUT = Duration.ofSeconds(120);

  /** The per-request deadline, for the scenario's client and for each concurrent caller. */
  static final Duration REQUEST_TIMEOUT = Duration.ofSeconds(30);

  /**
   * Matches an unresolved {contextVar} left in a docstring after substitution. Finding one is a
   * missing {@code save response body field}, and saying so beats letting PostgreSQL report a
   * syntax error three steps later.
   */
  private static final Pattern CONTEXT_VAR = Pattern.compile("\\{[a-z][a-zA-Z0-9]*\\}");

  private Network network;
  private PostgreSQLContainer postgres;
  private GenericContainer<?> service;

  /**
   * A direct connection to this scenario's PostgreSQL, used by the seeding and assertion steps. It
   * bypasses the service on purpose: an assertion that went through the API could not catch an API
   * that lies.
   */
  private Connection db;

  /** This scenario's service, reached on its mapped port. */
  private String baseUrl;

  /**
   * Holds no cookie handler — nothing is carried from one request to the next but the context bag —
   * and does NOT follow redirects, so a 3xx is observable as a status rather than silently resolved
   * into the status of somewhere else. HTTP/1.1 because that is what the service speaks behind
   * Caddy, and what the concurrent step writes by hand.
   */
  private HttpClient httpClient;

  /**
   * What the last single-request step recorded. Exactly one of it and {@link #responseSet} is ever
   * set: a step records either one response or a set, and each clears the other, so the two
   * vocabularies cannot be mixed.
   */
  private Response lastResponse;

  private List<RecordedResponse> responseSet;

  /** The context bag: {varName} tokens resolved in paths, JSON and SQL. */
  private final Map<String, String> vars = new HashMap<>();

  /** Picocontainer's constructor. Nothing is started here; {@link Hooks} decides when. */
  public ScenarioContext() {}

  /** One response as a single-request step recorded it. Header names are case-insensitive. */
  record Response(int status, Map<String, List<String>> headers, byte[] body) {

    Response {
      Map<String, List<String>> copy = new TreeMap<>(String.CASE_INSENSITIVE_ORDER);
      headers.forEach(
          (name, values) ->
              copy.computeIfAbsent(name, k -> new ArrayList<>()).addAll(List.copyOf(values)));
      headers = Collections.unmodifiableMap(copy);
    }

    /** Every value of the header, in the order received; empty when it is absent. */
    List<String> headerValues(String name) {
      return headers.getOrDefault(name, List.of());
    }

    String bodyText() {
      return new String(body, StandardCharsets.UTF_8);
    }
  }

  /**
   * One caller's answer in a concurrent set: the status, and the body kept for the distribution a
   * failed assertion prints.
   */
  record RecordedResponse(int status, byte[] body) {}

  // ───────────────────────────────────────────────────────────────────────────
  // Lifecycle
  // ───────────────────────────────────────────────────────────────────────────

  /**
   * Brings up this scenario's credentials, network, database and service. Called from {@link
   * Hooks}' Before hook, once per scenario.
   *
   * <p>Each resource is recorded in its field before it is started, so a start that fails half way
   * still leaves {@link #stop()} everything it has to tear down.
   */
  void start() throws SQLException, InterruptedException {
    vars.clear();
    vars.putAll(Credentials.mint(Instant.now()));

    httpClient =
        HttpClient.newBuilder()
            .version(HttpClient.Version.HTTP_1_1)
            .followRedirects(HttpClient.Redirect.NEVER)
            .connectTimeout(REQUEST_TIMEOUT)
            .build();

    network = Network.newNetwork();

    // PostgreSQL with durability turned off and its data directory on tmpfs. This database lives
    // for one scenario and is thrown away; fsync buys nothing here and costs the startup time that
    // makes per-scenario isolation affordable in the first place.
    postgres =
        new PostgreSQLContainer(DockerImageName.parse(POSTGRES_IMAGE))
            .withDatabaseName(DB_NAME)
            .withUsername(DB_USER)
            .withPassword(DB_PASSWORD)
            .withCommand(
                "postgres",
                "-c",
                "fsync=off",
                "-c",
                "full_page_writes=off",
                "-c",
                "synchronous_commit=off")
            .withTmpFs(Map.of("/var/lib/postgresql/data", "rw"))
            .withNetwork(network)
            .withNetworkAliases("postgres")
            // The entrypoint starts a temporary server to initialise the cluster before the real
            // one, so the message appears twice.
            .waitingFor(
                Wait.forLogMessage(".*database system is ready to accept connections.*\\s", 2)
                    .withStartupTimeout(STARTUP_TIMEOUT));
    postgres.start();

    // Retry rather than connect once. The container's wait strategy proves the server is listening
    // INSIDE the container; it says nothing about the host port forward, which is not always
    // routable at that instant. A single attempt turns that gap into a scenario that fails before
    // its first step.
    db = connectUntilReady(postgres.getJdbcUrl());

    // The service under test, configured the way the contract names its environment.
    // SCHEMA_AUTO_MIGRATE is false because `run migration` is a step of the feature: the schema is
    // created by the Background, in the open, not as a side effect of a container booting.
    // COOKIE_SECURE is false because this suite talks plain HTTP directly to the service; the
    // Secure attribute is exercised by the e2e stack, which has a TLS-terminating proxy in front.
    service =
        new GenericContainer<>(DockerImageName.parse(SERVICE_IMAGE))
            .withNetwork(network)
            .withExposedPorts(8080)
            .withEnv("PORT", "8080")
            .withEnv(
                "DB_DSN",
                "postgres://"
                    + DB_USER
                    + ":"
                    + DB_PASSWORD
                    + "@postgres:5432/"
                    + DB_NAME
                    + "?sslmode=disable")
            .withEnv("JWT_SECRET", Credentials.JWT_SECRET)
            .withEnv("SCHEMA_AUTO_MIGRATE", "false")
            .withEnv("FRONTEND_ORIGIN", "*")
            .withEnv("COOKIE_SECURE", "false")
            // Any status proves the HTTP server is listening, which is all that is being waited
            // for: the schema does not exist yet, so a route that reads the database is entitled
            // to fail at this point.
            .waitingFor(
                Wait.forHttp("/api/v1/products")
                    .forPort(8080)
                    .forStatusCodeMatching(status -> true)
                    .withStartupTimeout(STARTUP_TIMEOUT));
    service.start();

    baseUrl = "http://" + service.getHost() + ":" + service.getMappedPort(8080);
  }

  private static Connection connectUntilReady(String jdbcUrl)
      throws SQLException, InterruptedException {
    long deadline = System.nanoTime() + DB_READY_TIMEOUT.toNanos();
    SQLException last = null;
    while (true) {
      try {
        Connection connection = DriverManager.getConnection(jdbcUrl, DB_USER, DB_PASSWORD);
        if (connection.isValid(5)) {
          return connection;
        }
        connection.close();
      } catch (SQLException e) {
        last = e;
      }
      if (System.nanoTime() - deadline > 0) {
        SQLException failure =
            new SQLException(
                "database did not accept a connection within "
                    + DB_READY_TIMEOUT.toSeconds()
                    + "s"
                    + (last == null ? "" : ": " + last.getMessage()));
        if (last != null) {
          failure.initCause(last);
        }
        throw failure;
      }
      Thread.sleep(DB_READY_POLL_INTERVAL);
    }
  }

  /**
   * Tears the scenario down, in the reverse order of {@link #start()}. Called from the After hook,
   * including after a failure — a Before that failed half way included — so a red scenario leaves
   * no containers behind either. Every step is attempted even when an earlier one throws.
   */
  void stop() {
    if (service != null) {
      quietly("stop service", service::stop);
      service = null;
    }
    if (db != null) {
      Connection connection = db;
      quietly("close database connection", connection::close);
      db = null;
    }
    if (postgres != null) {
      quietly("stop postgres", postgres::stop);
      postgres = null;
    }
    if (network != null) {
      quietly("remove network", network::close);
      network = null;
    }
    if (httpClient != null) {
      quietly("close http client", httpClient::close);
      httpClient = null;
    }
  }

  private interface Teardown {
    void run() throws Exception;
  }

  private static void quietly(String what, Teardown teardown) {
    try {
      teardown.run();
    } catch (Exception e) {
      // A teardown failure must not mask the scenario's own result, nor stop the remaining
      // teardown; Ryuk reaps whatever is left when the JVM exits.
      System.err.println("teardown: " + what + " failed: " + e);
    }
  }

  /**
   * The service container's output so far, for the report of a failed scenario, or null when there
   * is no container to read it from.
   */
  String serviceLogs() {
    if (service == null || service.getContainerId() == null) {
      return null;
    }
    try {
      return service.getLogs();
    } catch (RuntimeException e) {
      return "(could not read the service container's logs: " + e + ")";
    }
  }

  // ───────────────────────────────────────────────────────────────────────────
  // State the steps share
  // ───────────────────────────────────────────────────────────────────────────

  Connection db() {
    if (db == null) {
      throw new IllegalStateException("no database: the scenario's infrastructure is not running");
    }
    return db;
  }

  /**
   * A DataSource for this scenario's database, for the migration step — the same kind of handle the
   * service gives SchemaMigrator when SCHEMA_AUTO_MIGRATE is true.
   */
  DataSource dataSource() {
    if (postgres == null) {
      throw new IllegalStateException("no database: the scenario's infrastructure is not running");
    }
    // Spring's DriverManagerDataSource rather than the driver's own: the PostgreSQL driver is a
    // runtime-only dependency, and a plain DriverManager-backed handle is all Flyway needs.
    return new DriverManagerDataSource(postgres.getJdbcUrl(), DB_USER, DB_PASSWORD);
  }

  String baseUrl() {
    return baseUrl;
  }

  HttpClient httpClient() {
    return httpClient;
  }

  /** Records one response; the concurrent set is cleared, because one response is on record. */
  void recordResponse(Response response) {
    lastResponse = response;
    responseSet = null;
  }

  /** Records a concurrent set; the single response is cleared. */
  void recordResponseSet(List<RecordedResponse> responses) {
    lastResponse = null;
    responseSet = List.copyOf(responses);
  }

  /** Forgets both, as a concurrent step does before it fires. */
  void clearResponses() {
    lastResponse = null;
    responseSet = null;
  }

  /** The last concurrent set, or null when no concurrent step has recorded one. */
  List<RecordedResponse> responseSet() {
    return responseSet;
  }

  /**
   * The guard every single-response assertion opens with.
   *
   * <p>After a concurrent step there is no "the response" to assert, and the danger is not the
   * absence — it is the response an earlier step left behind, which would let a status-only
   * assertion pass against the wrong request entirely. So the set is named in the message and the
   * step fails.
   */
  Response requireResponse() {
    if (lastResponse != null) {
      return lastResponse;
    }
    if (responseSet != null) {
      throw new AssertionError(
          "the last HTTP step was concurrent and recorded "
              + responseSet.size()
              + " responses, not one; assert them with `exactly <n> responses are <status>`");
    }
    throw new AssertionError("no response recorded: no HTTP step has run yet");
  }

  void saveVar(String name, String value) {
    vars.put(name, value);
  }

  /**
   * Substitutes every {varName} in text from the context bag. A token that survives substitution is
   * an error: it means the feature reads a variable no step ever saved, and reporting that here
   * beats letting it reach PostgreSQL as a literal.
   */
  String resolve(String text) {
    String resolved = text;
    for (Map.Entry<String, String> var : vars.entrySet()) {
      resolved = resolved.replace("{" + var.getKey() + "}", var.getValue());
    }
    Matcher leftover = CONTEXT_VAR.matcher(resolved);
    if (leftover.find()) {
      throw new AssertionError(
          "unknown context variable "
              + leftover.group()
              + " (saved so far: "
              + String.join(", ", savedVarNames())
              + ")");
    }
    return resolved;
  }

  private List<String> savedVarNames() {
    List<String> names = new ArrayList<>(vars.keySet());
    Collections.sort(names);
    return names.isEmpty() ? List.of("none") : names;
  }
}
