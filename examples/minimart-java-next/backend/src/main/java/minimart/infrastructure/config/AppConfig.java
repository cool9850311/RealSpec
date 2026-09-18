package minimart.infrastructure.config;

import java.io.Serial;
import java.net.URI;
import java.net.URISyntaxException;
import java.net.URLDecoder;
import java.nio.charset.StandardCharsets;
import java.util.Arrays;
import java.util.Map;
import java.util.Objects;
import java.util.function.Function;

/**
 * The whole configuration surface of the service, read from the process environment once, at boot,
 * into one immutable value. spec/spec.md points here rather than listing the names again.
 *
 * <p>There are no others, and nothing reads the environment outside this class. That includes
 * Spring itself: {@code MinimartApplication} starts the context without the system-environment
 * property source, so a stray {@code SERVER_PORT} or {@code SPRING_DATASOURCE_URL} in a deployment
 * cannot silently change what this class decided. application.properties carries no environment
 * placeholders for the same reason.
 *
 * <p>Deliberately absent:
 *
 * <ul>
 *   <li>the migrations location — the files are inside the jar (see {@code SchemaMigrator}); the
 *       layout is part of the artifact, not of the deployment;
 *   <li>the front end's API base — the front end always calls the relative {@code /api}, and the
 *       reverse proxy routes it here.
 * </ul>
 *
 * @param port the TCP port the HTTP server listens on. PORT, default 8080.
 * @param jdbcUrl the database, converted from DB_DSN (required), which takes the same {@code
 *     postgres://user:pass@host:port/db?params} URL the Go service takes. Query parameters such as
 *     {@code sslmode} are carried over unchanged; the PostgreSQL JDBC driver reads the same names.
 * @param dbUser the user part of DB_DSN, percent-decoded; empty when the DSN names none
 * @param dbPassword the password part of DB_DSN, percent-decoded; empty when the DSN names none
 * @param jwtSecret the HS256 signing key, as UTF-8 bytes. JWT_SECRET, required, at least {@value
 *     #MIN_JWT_SECRET_BYTES} bytes.
 * @param schemaAutoMigrate run the migrations at boot. SCHEMA_AUTO_MIGRATE, default false.
 * @param frontendOrigin the single origin CORS allows, or "*" to reflect whatever origin asked.
 *     FRONTEND_ORIGIN, default empty, which sends no CORS headers at all (correct for a same-origin
 *     deployment behind the reverse proxy).
 * @param cookieSecure add Secure to the session cookies. COOKIE_SECURE, default false; true
 *     everywhere the proxy terminates TLS.
 */
public record AppConfig(
    int port,
    String jdbcUrl,
    String dbUser,
    String dbPassword,
    byte[] jwtSecret,
    boolean schemaAutoMigrate,
    String frontendOrigin,
    boolean cookieSecure) {

  /**
   * The shortest JWT_SECRET the service accepts. HS256 is HMAC-SHA-256, and Nimbus — the JOSE
   * library under Spring Security — refuses an HMAC key shorter than the hash output (RFC 7518
   * §3.2). The check is made here so that a short key is a boot failure naming the variable, not a
   * 500 on the first login.
   */
  public static final int MIN_JWT_SECRET_BYTES = 32;

  static final String DEFAULT_PORT = "8080";

  /** A configuration value that cannot be used. The message names the variable. */
  public static final class InvalidConfigException extends RuntimeException {

    @Serial private static final long serialVersionUID = 1L;

    InvalidConfigException(String message) {
      super(message);
    }
  }

  /** Reads and validates the process environment. See {@link #load}. */
  public static AppConfig fromEnvironment() {
    Map<String, String> env = System.getenv();
    return load(env::get);
  }

  /**
   * Reads and validates the variables {@code env} supplies ({@code null} for an unset one). Every
   * failure is reported here, at boot, rather than as a surprise on the first request that needs
   * the value.
   *
   * @throws InvalidConfigException naming the first variable that is missing or malformed
   */
  public static AppConfig load(Function<String, String> env) {
    String dsn = env(env, "DB_DSN");
    if (dsn.isEmpty()) {
      throw new InvalidConfigException("DB_DSN is required");
    }
    String secret = env(env, "JWT_SECRET");
    if (secret.isEmpty()) {
      throw new InvalidConfigException("JWT_SECRET is required");
    }
    byte[] secretBytes = secret.getBytes(StandardCharsets.UTF_8);
    if (secretBytes.length < MIN_JWT_SECRET_BYTES) {
      throw new InvalidConfigException(
          "JWT_SECRET must be at least %d bytes for HS256, got %d"
              .formatted(MIN_JWT_SECRET_BYTES, secretBytes.length));
    }

    int port = parsePort(envOr(env, "PORT", DEFAULT_PORT));
    Dsn parsed = Dsn.parse(dsn);
    boolean schemaAutoMigrate = envBool(env, "SCHEMA_AUTO_MIGRATE", false);
    boolean cookieSecure = envBool(env, "COOKIE_SECURE", false);

    return new AppConfig(
        port,
        parsed.jdbcUrl(),
        parsed.user(),
        parsed.password(),
        secretBytes,
        schemaAutoMigrate,
        env(env, "FRONTEND_ORIGIN"),
        cookieSecure);
  }

  /** A defensive copy: the record must stay immutable even though an array is not. */
  @Override
  public byte[] jwtSecret() {
    return jwtSecret.clone();
  }

  /** Everything but the two secrets, so the configuration can be logged. */
  @Override
  public String toString() {
    return "AppConfig[port=%d, jdbcUrl=%s, dbUser=%s, schemaAutoMigrate=%s, frontendOrigin=%s, cookieSecure=%s]"
        .formatted(port, jdbcUrl, dbUser, schemaAutoMigrate, frontendOrigin, cookieSecure);
  }

  @Override
  public boolean equals(Object other) {
    return other instanceof AppConfig that
        && port == that.port
        && jdbcUrl.equals(that.jdbcUrl)
        && dbUser.equals(that.dbUser)
        && dbPassword.equals(that.dbPassword)
        && Arrays.equals(jwtSecret, that.jwtSecret)
        && schemaAutoMigrate == that.schemaAutoMigrate
        && frontendOrigin.equals(that.frontendOrigin)
        && cookieSecure == that.cookieSecure;
  }

  @Override
  public int hashCode() {
    return Objects.hash(
            port, jdbcUrl, dbUser, dbPassword, schemaAutoMigrate, frontendOrigin, cookieSecure)
        + 31 * Arrays.hashCode(jwtSecret);
  }

  private static String env(Function<String, String> env, String key) {
    String value = env.apply(key);
    return value == null ? "" : value;
  }

  private static String envOr(Function<String, String> env, String key, String fallback) {
    String value = env(env, key);
    return value.isEmpty() ? fallback : value;
  }

  private static int parsePort(String raw) {
    try {
      int port = Integer.parseInt(raw);
      if (port >= 1 && port <= 65535) {
        return port;
      }
    } catch (NumberFormatException notANumber) {
      // Reported below, with the value.
    }
    throw new InvalidConfigException("PORT must be a TCP port number, got \"%s\"".formatted(raw));
  }

  /**
   * An optional boolean. The accepted spellings are exactly those of Go's {@code strconv.ParseBool}
   * — the Go service reads the same variables, and a deployment must not mean one thing to one
   * implementation and another to the other. Anything else, "yes" included, is a boot failure.
   */
  private static boolean envBool(Function<String, String> env, String key, boolean fallback) {
    String raw = env(env, key);
    return switch (raw) {
      case "" -> fallback;
      case "1", "t", "T", "TRUE", "true", "True" -> true;
      case "0", "f", "F", "FALSE", "false", "False" -> false;
      default ->
          throw new InvalidConfigException("%s must be a boolean, got \"%s\"".formatted(key, raw));
    };
  }

  /**
   * DB_DSN, taken apart. The user and password travel to the driver as properties rather than
   * inside the JDBC URL, so the URL can be logged.
   */
  record Dsn(String jdbcUrl, String user, String password) {

    static Dsn parse(String dsn) {
      URI uri;
      try {
        uri = new URI(dsn);
      } catch (URISyntaxException e) {
        throw invalid("it is not a URL");
      }
      String scheme = uri.getScheme();
      if (!"postgres".equals(scheme) && !"postgresql".equals(scheme)) {
        throw invalid("the scheme must be postgres:// or postgresql://");
      }
      if (uri.getHost() == null || uri.getHost().isEmpty()) {
        throw invalid("it names no host");
      }
      if (uri.getRawFragment() != null) {
        throw invalid("it carries a #fragment");
      }

      String user = "";
      String password = "";
      String userInfo = uri.getRawUserInfo();
      if (userInfo != null) {
        // Split before decoding: a percent-encoded ':' belongs to the value, not to the syntax.
        int colon = userInfo.indexOf(':');
        user = percentDecode(colon < 0 ? userInfo : userInfo.substring(0, colon));
        password = colon < 0 ? "" : percentDecode(userInfo.substring(colon + 1));
      }

      String path = uri.getRawPath() == null ? "" : uri.getRawPath();
      String database = path.startsWith("/") ? path.substring(1) : path;
      if (database.contains("/")) {
        throw invalid("the path must be a single database name");
      }

      StringBuilder url = new StringBuilder("jdbc:postgresql://").append(uri.getHost());
      if (uri.getPort() != -1) {
        url.append(':').append(uri.getPort());
      }
      url.append('/').append(database);
      if (uri.getRawQuery() != null && !uri.getRawQuery().isEmpty()) {
        url.append('?').append(uri.getRawQuery());
      }
      return new Dsn(url.toString(), user, password);
    }

    /**
     * RFC 3986 percent-decoding. {@link URLDecoder} is form decoding, which would also turn '+'
     * into a space; in a URL's userinfo a '+' is a '+', so it is protected first.
     */
    private static String percentDecode(String raw) {
      try {
        return URLDecoder.decode(raw.replace("+", "%2B"), StandardCharsets.UTF_8);
      } catch (IllegalArgumentException malformedEscape) {
        throw invalid("its user or password has a malformed %-escape");
      }
    }

    private static InvalidConfigException invalid(String why) {
      // The DSN itself is not echoed: it usually carries a password.
      return new InvalidConfigException(
          "DB_DSN must be a URL of the form postgres://user:pass@host:port/db?params; " + why);
    }

    @Override
    public String toString() {
      return "Dsn[jdbcUrl=" + jdbcUrl + ", user=" + user + "]";
    }
  }
}
