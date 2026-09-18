package minimart.infrastructure.security;

import static org.assertj.core.api.Assertions.assertThat;

import java.util.List;
import org.junit.jupiter.params.ParameterizedTest;
import org.junit.jupiter.params.provider.ValueSource;
import org.springframework.http.ResponseCookie;

/**
 * Pins the property the front end depends on: the token is HttpOnly and the session hint is not. If
 * the hint ever became HttpOnly the SPA could not read it, would fall back to probing /me on every
 * anonymous page load, and the browser would record that 401 as a console error.
 */
class SessionCookiesTest {

  @ParameterizedTest(name = "secure={0}")
  @ValueSource(booleans = {false, true})
  void writesBothCookies(boolean secure) {
    List<ResponseCookie> cookies = new SessionCookies(secure).session("a.b.c");
    assertThat(cookies).hasSize(2);

    ResponseCookie token = cookies.get(0);
    assertThat(token.getName()).isEqualTo("token");
    assertThat(token.getValue()).isEqualTo("a.b.c");
    assertThat(token.isHttpOnly()).as("the token cookie must be HttpOnly").isTrue();

    ResponseCookie hint = cookies.get(1);
    assertThat(hint.getName()).isEqualTo("session_hint");
    assertThat(hint.getValue()).isEqualTo("1");
    assertThat(hint.isHttpOnly()).as("the session hint must be readable by script").isFalse();

    for (ResponseCookie cookie : cookies) {
      assertThat(cookie.getPath()).as(cookie.getName() + " Path").isEqualTo("/");
      assertThat(cookie.isSecure()).as(cookie.getName() + " Secure").isEqualTo(secure);
      assertThat(cookie.getSameSite()).as(cookie.getName() + " SameSite").isEqualTo("Lax");
      // No Max-Age: the JWT's exp is the lifetime, and a session cookie leaves nothing behind
      // when the browser closes.
      assertThat(cookie.getMaxAge().isNegative()).as(cookie.getName() + " has a Max-Age").isTrue();
      String header = cookie.toString();
      assertThat(header).doesNotContain("Max-Age").doesNotContain("Expires");
      assertThat(header.contains("; Secure")).isEqualTo(secure);
      assertThat(header).contains("; Path=/").contains("; SameSite=Lax");
    }
    assertThat(token.toString()).contains("; HttpOnly");
    assertThat(hint.toString()).doesNotContain("HttpOnly");
  }
}
