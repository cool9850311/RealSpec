package minimart.infrastructure.security;

import jakarta.servlet.http.HttpServletResponse;
import java.util.List;
import org.springframework.http.HttpHeaders;
import org.springframework.http.ResponseCookie;

/**
 * The two cookies a session consists of, and the single place their policy is decided.
 *
 * <ul>
 *   <li>{@value #TOKEN} is the session itself: the signed JWT, HttpOnly, which script can neither
 *       read nor forge.
 *   <li>{@value #SESSION_HINT} is a readable flag that says "this browser was given a token
 *       cookie". It carries no authority whatsoever — the server never reads it, and forging it
 *       buys nothing but a 401 — it exists so the SPA can tell "signed out" from "signed in"
 *       WITHOUT probing a protected endpoint. Without it the only way to answer that question is to
 *       call GET /api/v1/me and let it fail, and a browser records a failed request as an
 *       error-level console entry: an anonymous page load would then be indistinguishable from a
 *       broken one, both to a developer reading the console and to the {@code console has no
 *       errors} step of spec/bdd/format.yml.
 * </ul>
 *
 * Both are SameSite=Lax so they survive a top-level navigation but not a cross-site POST, Secure
 * wherever TLS is terminated in front of the service, and carry no Max-Age — the JWT's own exp is
 * the lifetime, and a session cookie leaves nothing behind when the browser closes. They are
 * written together and cleared together, so the readable flag cannot outlive the session it
 * describes by more than one request.
 */
public final class SessionCookies {

  /** The session cookie's name. */
  public static final String TOKEN = "token";

  /** The readable flag's name. */
  public static final String SESSION_HINT = "session_hint";

  /**
   * The only value the hint ever has. It is a flag, not a claim: nothing about the user is written
   * into a cookie script can read.
   */
  static final String SESSION_HINT_VALUE = "1";

  private static final String SAME_SITE = "Lax";

  private final boolean secure;

  /**
   * @param secure COOKIE_SECURE. Carried here so that the hint a 401 clears is written with the
   *     same attributes as the one login wrote: a Set-Cookie whose Secure flag disagrees does not
   *     replace the cookie it is meant to expire.
   */
  public SessionCookies(boolean secure) {
    this.secure = secure;
  }

  /** The two cookies a successful login sets: the token, then the hint. */
  public List<ResponseCookie> session(String token) {
    return List.of(
        ResponseCookie.from(TOKEN, token)
            .path("/")
            .httpOnly(true)
            .secure(secure)
            .sameSite(SAME_SITE)
            .build(),
        ResponseCookie.from(SESSION_HINT, SESSION_HINT_VALUE)
            .path("/")
            .httpOnly(false)
            .secure(secure)
            .sameSite(SAME_SITE)
            .build());
  }

  /**
   * The Set-Cookie that expires the hint. It is sent with every 401 from the filter chain, which is
   * the server's only chance to tell a browser that the token it is holding is no longer usable:
   * the token cookie itself may be expired, deleted or simply invalid, and the hint must not
   * survive it and send the SPA back to a protected endpoint on every page load.
   */
  public ResponseCookie expiredHint() {
    return ResponseCookie.from(SESSION_HINT, "")
        .path("/")
        .maxAge(0)
        .httpOnly(false)
        .secure(secure)
        .sameSite(SAME_SITE)
        .build();
  }

  /** Adds the login cookies to {@code response}. */
  public void writeSession(HttpServletResponse response, String token) {
    for (ResponseCookie cookie : session(token)) {
      response.addHeader(HttpHeaders.SET_COOKIE, cookie.toString());
    }
  }

  /** Adds the hint's expiry to {@code response}. */
  public void clearHint(HttpServletResponse response) {
    response.addHeader(HttpHeaders.SET_COOKIE, expiredHint().toString());
  }
}
