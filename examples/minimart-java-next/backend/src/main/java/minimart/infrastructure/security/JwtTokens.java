package minimart.infrastructure.security;

import com.nimbusds.jose.jwk.source.ImmutableSecret;
import java.time.Duration;
import java.time.Instant;
import java.util.List;
import javax.crypto.SecretKey;
import javax.crypto.spec.SecretKeySpec;
import minimart.domain.User;
import org.springframework.security.oauth2.core.DelegatingOAuth2TokenValidator;
import org.springframework.security.oauth2.core.OAuth2Error;
import org.springframework.security.oauth2.core.OAuth2ErrorCodes;
import org.springframework.security.oauth2.core.OAuth2TokenValidator;
import org.springframework.security.oauth2.core.OAuth2TokenValidatorResult;
import org.springframework.security.oauth2.jose.jws.MacAlgorithm;
import org.springframework.security.oauth2.jwt.JwsHeader;
import org.springframework.security.oauth2.jwt.Jwt;
import org.springframework.security.oauth2.jwt.JwtClaimNames;
import org.springframework.security.oauth2.jwt.JwtClaimsSet;
import org.springframework.security.oauth2.jwt.JwtDecoder;
import org.springframework.security.oauth2.jwt.JwtEncoder;
import org.springframework.security.oauth2.jwt.JwtEncoderParameters;
import org.springframework.security.oauth2.jwt.JwtTimestampValidator;
import org.springframework.security.oauth2.jwt.NimbusJwtDecoder;
import org.springframework.security.oauth2.jwt.NimbusJwtEncoder;

/**
 * Issues and verifies the session token: an HS256 JWT signed with JWT_SECRET.
 *
 * <p>Issuing and verifying live in one class, over one key and one algorithm, so the two cannot
 * drift apart.
 */
public final class JwtTokens {

  /**
   * How long an issued token stays valid. There is no refresh token: when it expires the user logs
   * in again.
   */
  public static final Duration TTL = Duration.ofHours(24);

  /** The one algorithm accepted, stated once so issuing and parsing cannot disagree. */
  static final MacAlgorithm ALGORITHM = MacAlgorithm.HS256;

  static final String CLAIM_USERNAME = "username";
  static final String CLAIM_ROLE = "role";

  private final JwtEncoder encoder;
  private final NimbusJwtDecoder decoder;

  /**
   * @param secret JWT_SECRET's bytes, at least 32 of them (AppConfig has already refused anything
   *     shorter; this is restated because a short key here would otherwise surface as a 500 on the
   *     first verification rather than as a construction failure)
   */
  public JwtTokens(byte[] secret) {
    if (secret.length < 32) {
      throw new IllegalArgumentException(
          "an HS256 key must be at least 32 bytes, got " + secret.length);
    }
    SecretKey key = new SecretKeySpec(secret, "HmacSHA256");

    // Built over the bare key rather than through NimbusJwtEncoder.withSecretKey, whose JWK names
    // itself by its RFC 7638 thumbprint and stamps it into every token as "kid" — a hash of the key
    // material, published in every cookie, serving nothing with a single key. The Go service's
    // tokens carry no kid either. The algorithm is still pinned: issue() names it in the header.
    this.encoder = new NimbusJwtEncoder(new ImmutableSecret<>(key));

    // Pinning the verifier to HS256 is what makes `alg: none` and an RS256 token whose "public
    // key" is our HMAC secret rejections rather than admissions: Nimbus selects the key by
    // algorithm, and there is no key for any other one.
    this.decoder = NimbusJwtDecoder.withSecretKey(key).macAlgorithm(ALGORITHM).build();
    this.decoder.setJwtValidator(validator());
  }

  /**
   * The claim checks Spring runs after the signature has been verified. They replace Spring's
   * defaults rather than adding to them, because two of the defaults are wrong for this service:
   *
   * <ul>
   *   <li>exp is REQUIRED. Spring's timestamp validator accepts a token with no exp, which would
   *       make a token eternal by omission.
   *   <li>The clock skew is ZERO, not Spring's default 60 seconds. The Go service allows none
   *       either, and the contract's {@code {tokenExpired}} credential expired exactly one minute
   *       ago — a default skew would accept it.
   *   <li>sub must be an integer, because it is a users.id: a well-signed token whose subject is
   *       not one must not reach a handler that would then query with it.
   *   <li>username and role, when present, must be strings — the shape the Go service's claims
   *       struct demands. Neither is ever used to decide anything.
   * </ul>
   */
  private static OAuth2TokenValidator<Jwt> validator() {
    JwtTimestampValidator timestamps = new JwtTimestampValidator(Duration.ZERO);
    timestamps.setAllowEmptyExpiryClaim(false);
    return new DelegatingOAuth2TokenValidator<>(
        List.of(timestamps, JwtTokens::validateSubject, JwtTokens::validateClaimTypes));
  }

  private static OAuth2TokenValidatorResult validateSubject(Jwt jwt) {
    if (jwt.getClaims().get(JwtClaimNames.SUB) instanceof String sub) {
      try {
        Long.parseLong(sub);
        return OAuth2TokenValidatorResult.success();
      } catch (NumberFormatException notAnId) {
        // Reported below.
      }
    }
    return invalid("sub is not a user id");
  }

  private static OAuth2TokenValidatorResult validateClaimTypes(Jwt jwt) {
    for (String claim : List.of(CLAIM_USERNAME, CLAIM_ROLE)) {
      Object value = jwt.getClaims().get(claim);
      if (value != null && !(value instanceof String)) {
        return invalid(claim + " is not a string");
      }
    }
    return OAuth2TokenValidatorResult.success();
  }

  private static OAuth2TokenValidatorResult invalid(String reason) {
    return OAuth2TokenValidatorResult.failure(
        new OAuth2Error(OAuth2ErrorCodes.INVALID_TOKEN, reason, null));
  }

  /**
   * Signs a token for {@code user}, valid for {@link #TTL} from {@code issuedAt}. sub is the user
   * id as a string, as the contract states; username and role ride along for the front end's
   * benefit and are never read back by this service to decide anything.
   */
  public String issue(User user, Instant issuedAt) {
    JwsHeader header = JwsHeader.with(ALGORITHM).type("JWT").build();
    JwtClaimsSet claims =
        JwtClaimsSet.builder()
            .subject(Integer.toString(user.id()))
            .claim(CLAIM_USERNAME, user.username())
            .claim(CLAIM_ROLE, user.role())
            .issuedAt(issuedAt)
            .expiresAt(issuedAt.plus(TTL))
            .build();
    return encoder.encode(JwtEncoderParameters.from(header, claims)).getTokenValue();
  }

  /** The verifier every protected request's token goes through. */
  public JwtDecoder decoder() {
    return decoder;
  }

  /**
   * The user id a verified token names. The validator has already established that sub parses, so a
   * failure here is a wiring bug — a Jwt that did not come from {@link #decoder()}.
   */
  public static long subjectOf(Jwt jwt) {
    return Long.parseLong(jwt.getSubject());
  }
}
