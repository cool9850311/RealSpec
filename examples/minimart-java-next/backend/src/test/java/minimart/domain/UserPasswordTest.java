package minimart.domain;

import static org.assertj.core.api.Assertions.assertThat;

import java.util.stream.Stream;
import org.junit.jupiter.params.ParameterizedTest;
import org.junit.jupiter.params.provider.Arguments;
import org.junit.jupiter.params.provider.MethodSource;

class UserPasswordTest {

  /** The bcrypt (cost 10) hash of "secret123" that every seeded user in spec/bdd/** carries. */
  static final String FIXTURE_HASH = "$2a$10$MwKxT13/lPxFMjvrkm0dL.QJuNll1zljDU.eOoRVvgrWF.kf/hyC2";

  static Stream<Arguments> cases() {
    return Stream.of(
        Arguments.of("the right password", FIXTURE_HASH, "secret123", true),
        Arguments.of("the wrong password", FIXTURE_HASH, "secret124", false),
        Arguments.of("an empty password", FIXTURE_HASH, "", false),
        Arguments.of("a column that never held a bcrypt hash", "secret123", "secret123", false),
        Arguments.of("an empty hash", "", "secret123", false),
        Arguments.of("a truncated hash", FIXTURE_HASH.substring(0, 20), "secret123", false));
  }

  @ParameterizedTest(name = "{0}")
  @MethodSource("cases")
  void verifyPassword(String name, String hash, String password, boolean want) {
    User user = new User(1, "alice", hash, User.ROLE_GUEST, 100);
    // A malformed hash must be a refusal, never an exception: the call is made directly, so
    // anything it throws fails the test on its own.
    boolean got = user.verifyPassword(password);
    assertThat(got).as(name).isEqualTo(want);
  }
}
