package minimart.infrastructure.security;

import java.io.Serial;
import org.springframework.security.core.AuthenticationException;

/**
 * A correctly signed, unexpired token whose subject names no users row.
 *
 * <p>It is an {@link AuthenticationException} on purpose: thrown from the authorization step, it is
 * what makes Spring Security's exception translation answer 401 through the same entry point as
 * every other broken session (clearing the hint), rather than 403. There is nobody to forbid, and
 * the caller's remedy is a new session rather than different permissions.
 */
final class UnknownSubjectException extends AuthenticationException {

  @Serial private static final long serialVersionUID = 1L;

  UnknownSubjectException(long userId) {
    super("token subject " + userId + " names no user");
  }
}
