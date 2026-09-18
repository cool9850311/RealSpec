package minimart.infrastructure.security;

import jakarta.servlet.http.HttpServletRequest;
import jakarta.servlet.http.HttpServletResponse;
import java.io.IOException;
import minimart.domain.ErrorCode;
import org.springframework.security.access.AccessDeniedException;
import org.springframework.security.web.access.AccessDeniedHandler;

/**
 * Every 403: an authenticated caller whose stored role is not the one the route requires.
 *
 * <p>The session hint is left alone on purpose — this session is perfectly valid, and expiring the
 * hint would send the front end back to the login page over a permission it never had.
 */
final class JsonAccessDeniedHandler implements AccessDeniedHandler {

  @Override
  public void handle(
      HttpServletRequest request, HttpServletResponse response, AccessDeniedException reason)
      throws IOException {
    ErrorBodies.write(response, HttpServletResponse.SC_FORBIDDEN, ErrorCode.FORBIDDEN.name());
  }
}
