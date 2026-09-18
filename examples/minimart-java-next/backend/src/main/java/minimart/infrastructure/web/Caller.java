package minimart.infrastructure.web;

import minimart.domain.DomainException;
import minimart.domain.ErrorCode;
import minimart.infrastructure.security.JwtTokens;
import org.springframework.security.core.Authentication;
import org.springframework.security.oauth2.server.resource.authentication.JwtAuthenticationToken;

/** The authenticated caller of a request, as the security filter chain established it. */
final class Caller {

  private Caller() {}

  /**
   * Returns the caller's user id. It can only fail if a route was served without a session rule in
   * front of it, which is a programming error; reporting it as UNAUTHENTICATED keeps the service
   * from serving anybody's data by accident.
   */
  static long id(Authentication authentication) {
    if (authentication instanceof JwtAuthenticationToken session) {
      return JwtTokens.subjectOf(session.getToken());
    }
    throw new DomainException(ErrorCode.UNAUTHENTICATED);
  }
}
