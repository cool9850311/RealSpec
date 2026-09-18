package minimart.infrastructure.security;

import jakarta.servlet.FilterChain;
import jakarta.servlet.ServletException;
import jakarta.servlet.http.HttpServletRequest;
import jakarta.servlet.http.HttpServletResponse;
import java.io.IOException;
import org.springframework.http.HttpHeaders;
import org.springframework.web.filter.OncePerRequestFilter;

/**
 * Answers cross-origin requests according to FRONTEND_ORIGIN.
 *
 * <pre>
 *   ""   — send no CORS headers. Correct for the deployments of this example, where the reverse
 *          proxy puts the front end and the API on one origin and there is no cross-origin request
 *          to allow.
 *   "*"  — reflect whatever Origin asked. Credentials cannot be combined with a literal "*", so the
 *          wildcard is implemented by echoing the origin.
 *   host — allow exactly that origin and no other.
 * </pre>
 *
 * Allow-Credentials is always true when an origin is allowed at all: the session is a cookie, so a
 * cross-origin front end that cannot send credentials cannot log in.
 *
 * <p>This is a line-for-line port of the Go service's CORS middleware rather than Spring's {@code
 * CorsConfiguration}, whose rules differ in the details that are observable here: it rejects a
 * disallowed preflight with 403 where this answers 204 without headers, and it would not add {@code
 * Vary: Origin} to a response it did not allow. It is registered as a servlet filter ahead of
 * Spring Security, so — like the Go middleware, which is mounted on the whole engine — it sees
 * every request, including those for paths the API does not have.
 */
public final class CorsFilter extends OncePerRequestFilter {

  private final String allowedOrigin;

  /** A filter for FRONTEND_ORIGIN = {@code allowedOrigin} (empty for none). */
  public CorsFilter(String allowedOrigin) {
    this.allowedOrigin = allowedOrigin;
  }

  @Override
  protected void doFilterInternal(
      HttpServletRequest request, HttpServletResponse response, FilterChain chain)
      throws ServletException, IOException {
    if (!allowedOrigin.isEmpty()) {
      // The response depends on the request's Origin, so it is not cacheable without it — whether
      // or not we ended up allowing it.
      response.addHeader(HttpHeaders.VARY, HttpHeaders.ORIGIN);

      String origin = request.getHeader(HttpHeaders.ORIGIN);
      if (origin != null
          && !origin.isEmpty()
          && (allowedOrigin.equals("*") || allowedOrigin.equals(origin))) {
        response.setHeader(HttpHeaders.ACCESS_CONTROL_ALLOW_ORIGIN, origin);
        response.setHeader(HttpHeaders.ACCESS_CONTROL_ALLOW_CREDENTIALS, "true");
        response.setHeader(HttpHeaders.ACCESS_CONTROL_ALLOW_METHODS, "GET, POST, OPTIONS");
        response.setHeader(HttpHeaders.ACCESS_CONTROL_ALLOW_HEADERS, "Content-Type");
        response.setHeader(HttpHeaders.ACCESS_CONTROL_MAX_AGE, "600");
      }
    }

    if ("OPTIONS".equals(request.getMethod())) {
      // A preflight carries no body and reaches no handler. Whether the browser then proceeds is
      // decided by the headers set above.
      response.setStatus(HttpServletResponse.SC_NO_CONTENT);
      return;
    }
    chain.doFilter(request, response);
  }
}
