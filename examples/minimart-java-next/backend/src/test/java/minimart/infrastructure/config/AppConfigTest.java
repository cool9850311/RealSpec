package minimart.infrastructure.config;

import static org.assertj.core.api.Assertions.assertThat;
import static org.assertj.core.api.Assertions.assertThatThrownBy;

import java.util.HashMap;
import java.util.Map;
import org.junit.jupiter.api.Test;

class AppConfigTest {

  /** Exactly 32 bytes: the shortest secret HS256 accepts. */
  static final String SECRET_32 = "0123456789abcdef0123456789abcdef";

  private final Map<String, String> env = new HashMap<>();

  AppConfigTest() {
    env.put("DB_DSN", "postgres://minimart:minimart_secret@postgres:5432/minimart?sslmode=disable");
    env.put("JWT_SECRET", SECRET_32);
  }

  private AppConfig load() {
    return AppConfig.load(env::get);
  }

  @Test
  void aMissingDbDsnIsNamed() {
    env.remove("DB_DSN");
    assertInvalid("DB_DSN is required");
  }

  @Test
  void aMissingJwtSecretIsNamed() {
    env.remove("JWT_SECRET");
    assertInvalid("JWT_SECRET is required");
  }

  @Test
  void cookieSecureMustBeAGoBoolean() {
    env.put("COOKIE_SECURE", "yes");
    assertInvalid("COOKIE_SECURE must be a boolean, got \"yes\"");
  }

  @Test
  void theDsnBecomesAJdbcUrlAUserAndAPassword() {
    AppConfig config = load();

    assertThat(config.jdbcUrl())
        .isEqualTo("jdbc:postgresql://postgres:5432/minimart?sslmode=disable");
    assertThat(config.dbUser()).isEqualTo("minimart");
    assertThat(config.dbPassword()).isEqualTo("minimart_secret");
  }

  @Test
  void aThirtyOneByteSecretIsRefused() {
    env.put("JWT_SECRET", SECRET_32.substring(1));
    assertInvalid("JWT_SECRET must be at least 32 bytes for HS256, got 31");
  }

  @Test
  void aThirtyTwoByteSecretIsAccepted() {
    assertThat(load().jwtSecret()).hasSize(32);
  }

  @Test
  void thePortDefaultsTo8080() {
    assertThat(load().port()).isEqualTo(8080);
  }

  private void assertInvalid(String message) {
    assertThatThrownBy(this::load)
        .isInstanceOf(AppConfig.InvalidConfigException.class)
        .hasMessage(message);
  }
}
