package minimart.infrastructure.security;

import jakarta.servlet.FilterChain;
import jakarta.servlet.ServletException;
import jakarta.servlet.http.HttpServletRequest;
import jakarta.servlet.http.HttpServletResponse;
import java.io.IOException;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;
import org.springframework.web.filter.OncePerRequestFilter;

/**
 * Turns an exception that escapes the rest of the security chain into the contract's 500.
 *
 * <p>Exceptions thrown by controllers never get here — {@code ApiExceptionHandler} renders them
 * inside Spring MVC. What does get here is a failure in the chain itself, in practice the role
 * lookup's database read in {@link RoleAuthorizationManager}: Spring Security's exception
 * translation handles only authentication and access-denied exceptions and lets everything else
 * through, and without this filter the servlet container would answer with its own error page.
 *
 * <p>Anything that is not one of the contract's codes is a defect in this service, not a message
 * for the caller: it is logged in full and answered with a 500 that discloses nothing about the
 * failure.
 */
final class InternalErrorFilter extends OncePerRequestFilter {

  private static final Logger log = LoggerFactory.getLogger(InternalErrorFilter.class);

  @Override
  protected void doFilterInternal(
      HttpServletRequest request, HttpServletResponse response, FilterChain chain)
      throws ServletException, IOException {
    try {
      chain.doFilter(request, response);
    } catch (RuntimeException | ServletException e) {
      log.error("unhandled error on {} {}", request.getMethod(), request.getRequestURI(), e);
      if (response.isCommitted()) {
        // Too late to change the status; the log line is all that can be done.
        return;
      }
      // Only the body is discarded. Headers already set stay — the CORS ones in particular, which
      // the browser needs in order to show the caller this 500 at all.
      response.resetBuffer();
      ErrorBodies.write(response, HttpServletResponse.SC_INTERNAL_SERVER_ERROR, "INTERNAL_ERROR");
    }
  }
}
