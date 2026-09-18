package minimart.infrastructure.security;

import jakarta.servlet.http.HttpServletRequest;
import jakarta.servlet.http.HttpServletResponse;
import java.io.IOException;
import minimart.domain.ErrorCode;
import org.springframework.security.core.AuthenticationException;
import org.springframework.security.web.AuthenticationEntryPoint;

/**
 * Every 401 the filter chain produces: no session on a protected route, a token that fails
 * verification, or a subject with no users row.
 *
 * <p>The body is the contract's {@code {"error":"UNAUTHENTICATED"}} and the hint cookie is expired
 * with it (see {@link SessionCookies#expiredHint()}). No WWW-Authenticate header is sent: the
 * resource server's default would announce a Bearer scheme this API does not accept, and the Go
 * service sends none.
 */
final class JsonAuthenticationEntryPoint implements AuthenticationEntryPoint {

  private final SessionCookies cookies;

  JsonAuthenticationEntryPoint(SessionCookies cookies) {
    this.cookies = cookies;
  }

  @Override
  public void commence(
      HttpServletRequest request, HttpServletResponse response, AuthenticationException reason)
      throws IOException {
    cookies.clearHint(response);
    ErrorBodies.write(
        response, HttpServletResponse.SC_UNAUTHORIZED, ErrorCode.UNAUTHENTICATED.name());
  }
}
