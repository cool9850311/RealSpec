package minimart.infrastructure.security;

import static org.assertj.core.api.Assertions.assertThat;
import static org.assertj.core.api.Assertions.assertThatThrownBy;

import com.nimbusds.jose.JOSEException;
import com.nimbusds.jose.JWSAlgorithm;
import com.nimbusds.jose.JWSHeader;
import com.nimbusds.jose.crypto.MACSigner;
import com.nimbusds.jwt.JWTClaimsSet;
import com.nimbusds.jwt.SignedJWT;
import java.nio.charset.StandardCharsets;
import java.time.Duration;
import java.time.Instant;
import java.util.Base64;
import java.util.Date;
import minimart.domain.User;
import org.junit.jupiter.api.Test;
import org.springframework.security.oauth2.jwt.Jwt;
import org.springframework.security.oauth2.jwt.JwtException;

class JwtTokensTest {

  static final byte[] SECRET = "unit-test-secret-0123456789abcdef".getBytes(StandardCharsets.UTF_8);
  static final byte[] OTHER_SECRET =
      "someone-else's-secret-0123456789ab".getBytes(StandardCharsets.UTF_8);

  private final JwtTokens tokens = new JwtTokens(SECRET);

  @Test
  void issueAndDecodeRoundTrip() {
    Instant issuedAt = Instant.now();
    String token = tokens.issue(new User(42, "alice", "", User.ROLE_GUEST, 0), issuedAt);

    Jwt jwt = tokens.decoder().decode(token);

    assertThat(jwt.getSubject()).isEqualTo("42");
    assertThat(jwt.getClaimAsString("username")).isEqualTo("alice");
    assertThat(jwt.getClaimAsString("role")).isEqualTo(User.ROLE_GUEST);
    assertThat(jwt.getIssuedAt()).isNotNull();
    assertThat(jwt.getExpiresAt()).isNotNull();
    // Both claims are whole seconds on the wire, so they are compared to each other rather than
    // to issuedAt, whose nanoseconds do not survive encoding.
    assertThat(Duration.between(jwt.getIssuedAt(), jwt.getExpiresAt())).isEqualTo(JwtTokens.TTL);
    assertThat(Duration.between(issuedAt, jwt.getIssuedAt()))
        .as("iat relative to the requested issue time")
        .isBetween(Duration.ofSeconds(-1), Duration.ZERO);
  }

  @Test
  void rejectsATokenThatExpiredAMinuteAgo() {
    String token =
        tokens.issue(guest(), Instant.now().minus(JwtTokens.TTL).minus(Duration.ofMinutes(1)));
    assertRejected(token);
  }

  /**
   * Five seconds is inside Spring's default 60-second clock skew; with the skew at zero, as in the
   * Go service, it is simply expired.
   */
  @Test
  void rejectsATokenThatExpiredFiveSecondsAgo() {
    String token =
        tokens.issue(guest(), Instant.now().minus(JwtTokens.TTL).minus(Duration.ofSeconds(5)));
    assertRejected(token);
  }

  @Test
  void rejectsATokenSignedWithAnotherKey() {
    String token = new JwtTokens(OTHER_SECRET).issue(guest(), Instant.now());
    assertRejected(token);
  }

  /**
   * The classic unsigned token: the same claims, an "alg": "none" header and an empty signature.
   * Accepting it would let anybody mint any session, so it is asserted rather than assumed.
   */
  @Test
  void rejectsAlgNone() {
    String header = segment("{\"alg\":\"none\",\"typ\":\"JWT\"}");
    String payload =
        segment(
            "{\"sub\":\"1\",\"username\":\"alice\",\"role\":\"admin\",\"exp\":%d}"
                .formatted(Instant.now().plusSeconds(3600).getEpochSecond()));
    assertRejected(header + "." + payload + ".");
  }

  @Test
  void rejectsATamperedToken() {
    String token = tokens.issue(guest(), Instant.now());
    assertRejected(token + "tampered");
  }

  /**
   * A well-signed token whose subject is not a user id must not reach a handler that would then
   * query with it.
   */
  @Test
  void rejectsANonNumericSubject() throws JOSEException {
    assertRejected(
        sign(
            new JWTClaimsSet.Builder()
                .subject("alice")
                .claim("username", "alice")
                .claim("role", User.ROLE_GUEST)
                .issueTime(new Date())
                .expirationTime(Date.from(Instant.now().plusSeconds(3600)))
                .build()));
  }

  /** A token with no exp would be eternal by omission. */
  @Test
  void rejectsATokenWithoutExp() throws JOSEException {
    assertRejected(
        sign(
            new JWTClaimsSet.Builder()
                .subject("1")
                .claim("username", "alice")
                .claim("role", User.ROLE_GUEST)
                .issueTime(new Date())
                .build()));
  }

  private void assertRejected(String token) {
    assertThatThrownBy(() -> tokens.decoder().decode(token)).isInstanceOf(JwtException.class);
  }

  private static User guest() {
    return new User(1, "alice", "", User.ROLE_GUEST, 0);
  }

  /** Signs {@code claims} with the right key and algorithm, bypassing JwtTokens.issue. */
  private static String sign(JWTClaimsSet claims) throws JOSEException {
    SignedJWT jwt = new SignedJWT(new JWSHeader(JWSAlgorithm.HS256), claims);
    jwt.sign(new MACSigner(SECRET));
    return jwt.serialize();
  }

  private static String segment(String json) {
    return Base64.getUrlEncoder()
        .withoutPadding()
        .encodeToString(json.getBytes(StandardCharsets.UTF_8));
  }
}
