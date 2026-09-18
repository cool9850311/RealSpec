package minimart.infrastructure.security;

import jakarta.servlet.http.Cookie;
import jakarta.servlet.http.HttpServletRequest;
import org.springframework.security.oauth2.server.resource.web.BearerTokenResolver;
import org.springframework.security.web.util.matcher.RequestMatcher;

/**
 * Finds the credential on a request: the {@value SessionCookies#TOKEN} cookie, and nothing else.
 *
 * <p>How the credential reaches the service is one thing, stated once. The Authorization header is
 * not a second way in: a {@code Bearer} token there is ignored, exactly as the Go service ignores
 * it, so a script that obtained a token some other way cannot use it without a cookie jar.
 *
 * <p>The cookie is read only on the routes that require a session. Everywhere else it is not looked
 * at at all — which matters, because a browser sends its token cookie with every request, including
 * to the login endpoint and the public catalogue, and an expired or garbled one there must not turn
 * a public request into a 401. That is the Go service's behaviour too: its RequireAuth middleware
 * is mounted only on the protected route group.
 */
final class CookieTokenResolver implements BearerTokenResolver {

  private final RequestMatcher protectedRoutes;

  CookieTokenResolver(RequestMatcher protectedRoutes) {
    this.protectedRoutes = protectedRoutes;
  }

  /**
   * @return the token cookie's value on a protected route, or null — anonymous — when the route is
   *     public, the cookie is absent, or its value is empty ({@code Cookie: token=} is no session)
   */
  @Override
  public String resolve(HttpServletRequest request) {
    if (!protectedRoutes.matches(request)) {
      return null;
    }
    Cookie[] cookies = request.getCookies();
    if (cookies == null) {
      return null;
    }
    for (Cookie cookie : cookies) {
      if (SessionCookies.TOKEN.equals(cookie.getName())) {
        // The first one wins, as it does for Go's Request.Cookie. An empty value is no session.
        String value = cookie.getValue();
        return value == null || value.isEmpty() ? null : value;
      }
    }
    return null;
  }
}
