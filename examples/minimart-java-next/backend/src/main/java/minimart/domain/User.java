package minimart.domain;

import org.springframework.security.crypto.bcrypt.BCryptPasswordEncoder;

/**
 * A row of the users table. The password hash never leaves the process — {@link #toString()} does
 * not print it, so neither can a log line that prints a user.
 *
 * @param role {@link #ROLE_GUEST} or {@link #ROLE_ADMIN}; the users.role CHECK constraint admits
 *     nothing else
 */
public record User(int id, String username, String passwordHash, String role, int points) {

  /**
   * Roles. 'admin' may list every user's orders (GET /api/v1/admin/orders); 'guest' may not. The
   * role that decides is the one on the users row, read per request — never the role claim carried
   * in the token, which is a statement about the moment the token was issued and is not revocable.
   */
  public static final String ROLE_GUEST = "guest";

  /** See {@link #ROLE_GUEST}. */
  public static final String ROLE_ADMIN = "admin";

  /**
   * Stateless and thread-safe; one instance serves every comparison. The strength argument only
   * matters for encoding, which this service never does — verification reads the cost from the
   * stored hash.
   */
  private static final BCryptPasswordEncoder BCRYPT = new BCryptPasswordEncoder();

  /**
   * Compares {@code plain} against the stored bcrypt hash.
   *
   * <p>Every failure — a wrong password, a truncated hash, a column that never held a bcrypt hash
   * at all — is reported as {@code false}, and none of them throws: the encoder rejects a malformed
   * hash by returning false, and the one argument it does throw on (a password longer than bcrypt's
   * 72-byte input limit, which Spring Security refuses rather than silently truncating) is caught
   * and answered the same way. The Go service's bcrypt compares only the first 72 bytes of such a
   * password; refusing it instead can turn a would-be success into INVALID_CREDENTIALS for a
   * password nobody can have registered through this service, never the other way round.
   */
  public boolean verifyPassword(String plain) {
    if (plain == null || passwordHash == null) {
      return false;
    }
    try {
      return BCRYPT.matches(plain, passwordHash);
    } catch (IllegalArgumentException tooLongOrMalformed) {
      return false;
    }
  }

  @Override
  public String toString() {
    return "User[id="
        + id
        + ", username="
        + username
        + ", role="
        + role
        + ", points="
        + points
        + "]";
  }
}
