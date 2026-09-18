package minimart.bdd;

import com.nimbusds.jose.JOSEException;
import com.nimbusds.jose.JOSEObjectType;
import com.nimbusds.jose.JWSAlgorithm;
import com.nimbusds.jose.JWSHeader;
import com.nimbusds.jose.crypto.MACSigner;
import com.nimbusds.jwt.JWTClaimsSet;
import com.nimbusds.jwt.SignedJWT;
import java.nio.charset.StandardCharsets;
import java.time.Duration;
import java.time.Instant;
import java.util.Date;
import java.util.LinkedHashMap;
import java.util.Map;

/**
 * The pre-minted credentials.
 *
 * <p>Four context variables exist in every scenario before its first step runs, documented for the
 * features in spec/bdd/format.yml. Each holds a complete HS256 JWT that is what POST
 * /api/v1/auth/login would have issued for the fixture guest, deviating in exactly one field — and
 * the deviation is what the scenario carrying it tests.
 *
 * <p>The suite can sign them because the suite is what sets JWT_SECRET on the container: no
 * cooperation is needed from the service, and none is given. The alternatives all fail the same way
 * — a backdoor token, a test-only endpoint or a debug flag would move the attacker's tools inside
 * the artefact being defended, and the suite would then be evidence about a build nobody deploys.
 *
 * <p>They are values in the bag and nothing more. None of them authenticates anything, none of them
 * issues a request, and none of them moves the response the assertion steps refer to. A request is
 * made with one only when its own docstring writes {@code "Cookie": "token={tokenWhatever}"}.
 *
 * <p>There is no step that mints a fifth, on purpose: every deviation this suite tests is written
 * here, once, where it can be read alongside the other three rather than reinvented per feature.
 */
final class Credentials {

  /**
   * This suite's signing key. It is not the deployment's: a test that passes only because it shares
   * production's secret is not a test. At least 32 bytes, because the service (Nimbus, HS256)
   * refuses a shorter key at startup — go-nuxt's 21-byte secret would not boot this service.
   */
  static final String JWT_SECRET = "minimart-cucumber-secret-0123456789abcdef";

  /**
   * Signs {tokenWrongKey}: a key no container is ever given, so a token signed with it can only be
   * rejected. Its content is irrelevant as long as it differs from JWT_SECRET; its length is not,
   * because Nimbus will not sign HS256 with fewer than 32 bytes.
   */
  static final String WRONG_JWT_SECRET = "minimart-cucumber-not-the-secret-0123456789";

  /**
   * The lifetime the service itself issues, so a pre-minted token is indistinguishable from one the
   * login endpoint would have issued except in the one field it deviates on.
   */
  static final Duration SIGNED_TTL = Duration.ofHours(24);

  // The identities the pre-minted credentials speak for. They are constants rather than lookups
  // because these tokens are minted before the Background runs — there is no users table yet to
  // read. Every API Background seeds the fixture guest as users.id = 1 under this name, and none
  // of them seeds UNKNOWN_USER_ID; spec/bdd/format.yml says so where a feature's reader is, and the
  // scenarios that turn on it assert the premise in SQL.
  static final String FIXTURE_GUEST_ID = "1";
  static final String FIXTURE_GUEST_USERNAME = "alice";
  static final String FIXTURE_GUEST_ROLE = "guest";
  static final String UNKNOWN_USER_ID = "4242";
  static final String UNKNOWN_USER_USERNAME = "ghost";

  private Credentials() {}

  /**
   * Mints the four credentials as of {@code now}, keyed by the context variable each one fills.
   *
   * <p>Called as the bag is created, before the Background runs, so it cannot consult the database
   * — which is why the identities above are constants.
   */
  static Map<String, String> mint(Instant now) {
    // Expired: issued a full lifetime ago and one minute past its end. Signed correctly, so nothing
    // but the clock can be the reason it is refused.
    Instant expiredAt = now.minus(Duration.ofMinutes(1));
    JWTClaimsSet expired =
        claims(
            FIXTURE_GUEST_ID,
            FIXTURE_GUEST_USERNAME,
            FIXTURE_GUEST_ROLE,
            expiredAt.minus(SIGNED_TTL),
            expiredAt);

    // A user who is gone: a sub no Background seeds, under a username no Background seeds either,
    // so neither lookup can accidentally succeed.
    JWTClaimsSet unknown =
        claims(
            UNKNOWN_USER_ID, UNKNOWN_USER_USERNAME, FIXTURE_GUEST_ROLE, now, now.plus(SIGNED_TTL));

    // The privilege-escalation case: valid JWT, real key, unexpired, claiming a role the users row
    // does not grant.
    JWTClaimsSet claimsAdmin =
        claims(FIXTURE_GUEST_ID, FIXTURE_GUEST_USERNAME, "admin", now, now.plus(SIGNED_TTL));

    Map<String, String> bag = new LinkedHashMap<>();
    bag.put("tokenExpired", sign(expired, JWT_SECRET));
    // The genuine session, changed in nothing but the key it is signed with, which is what makes
    // the signature the only possible reason to refuse it.
    bag.put("tokenWrongKey", sign(genuine(now), WRONG_JWT_SECRET));
    bag.put("tokenUnknownUser", sign(unknown, JWT_SECRET));
    bag.put("tokenClaimsAdmin", sign(claimsAdmin, JWT_SECRET));
    return bag;
  }

  /**
   * The genuine session all four are derived from: the fixture guest, valid for as long as the
   * service's own tokens are.
   */
  static JWTClaimsSet genuine(Instant now) {
    return claims(
        FIXTURE_GUEST_ID, FIXTURE_GUEST_USERNAME, FIXTURE_GUEST_ROLE, now, now.plus(SIGNED_TTL));
  }

  private static JWTClaimsSet claims(
      String sub, String username, String role, Instant issuedAt, Instant expiresAt) {
    // Nimbus serialises Date claims as whole seconds, as jwt/v5's NumericDate does in go-nuxt.
    return new JWTClaimsSet.Builder()
        .subject(sub)
        .claim("username", username)
        .claim("role", role)
        .issueTime(Date.from(issuedAt))
        .expirationTime(Date.from(expiresAt))
        .build();
  }

  private static String sign(JWTClaimsSet claims, String secret) {
    // typ: JWT matches the header go-nuxt's jwt library writes; the service does not read it.
    JWSHeader header = new JWSHeader.Builder(JWSAlgorithm.HS256).type(JOSEObjectType.JWT).build();
    SignedJWT jwt = new SignedJWT(header, claims);
    try {
      jwt.sign(new MACSigner(secret.getBytes(StandardCharsets.UTF_8)));
    } catch (JOSEException e) {
      throw new IllegalStateException("mint pre-minted credential: " + e.getMessage(), e);
    }
    return jwt.serialize();
  }
}
