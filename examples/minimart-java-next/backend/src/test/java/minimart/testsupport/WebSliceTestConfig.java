package minimart.testsupport;

import java.nio.charset.StandardCharsets;
import java.util.Map;
import minimart.infrastructure.config.AppConfig;
import org.springframework.boot.test.context.TestConfiguration;
import org.springframework.context.annotation.Bean;

/**
 * What a {@code @WebMvcTest} of this service needs besides the controllers and the real security
 * chain (imported by the test): a configuration, and a role lookup the test controls in place of
 * the database.
 */
@TestConfiguration(proxyBeanMethods = false)
public class WebSliceTestConfig {

  /** The key every token in these tests is signed with; 32 bytes, as AppConfig requires. */
  public static final String JWT_SECRET = "web-slice-test-secret-0123456789";

  /**
   * COOKIE_SECURE is true, so the tests can see that the hint a 401 expires carries the same Secure
   * attribute as the one login set.
   */
  @Bean
  AppConfig appConfig() {
    Map<String, String> env =
        Map.of(
            "DB_DSN", "postgres://unused:unused@localhost:5432/unused",
            "JWT_SECRET", JWT_SECRET,
            "COOKIE_SECURE", "true");
    return AppConfig.load(env::get);
  }

  @Bean
  StubRoleLookup roleLookup() {
    return new StubRoleLookup();
  }

  /** The secret's bytes, for tests that sign their own tokens. */
  public static byte[] secretBytes() {
    return JWT_SECRET.getBytes(StandardCharsets.UTF_8);
  }
}
