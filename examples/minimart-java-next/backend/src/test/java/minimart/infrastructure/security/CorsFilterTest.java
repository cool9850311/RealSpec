package minimart.infrastructure.security;

import static org.assertj.core.api.Assertions.assertThat;

import org.junit.jupiter.api.Test;
import org.springframework.mock.web.MockFilterChain;
import org.springframework.mock.web.MockHttpServletRequest;
import org.springframework.mock.web.MockHttpServletResponse;

class CorsFilterTest {

  private static final String ORIGIN = "https://shop.example";

  @Test
  void anEmptySettingSendsNoCorsHeaders() throws Exception {
    Exchange exchange = run("", "GET", ORIGIN);

    assertThat(exchange.response.getHeaderNames())
        .noneMatch(name -> name.startsWith("Access-Control-"))
        .doesNotContain("Vary");
    assertThat(exchange.passedOn()).isTrue();
  }

  @Test
  void aWildcardReflectsTheOriginWithCredentials() throws Exception {
    Exchange exchange = run("*", "GET", ORIGIN);

    assertAllowed(exchange, ORIGIN);
    assertThat(exchange.passedOn()).isTrue();
  }

  @Test
  void aNamedOriginIsAllowedWhenItMatches() throws Exception {
    Exchange exchange = run(ORIGIN, "GET", ORIGIN);

    assertAllowed(exchange, ORIGIN);
    assertThat(exchange.passedOn()).isTrue();
  }

  @Test
  void aNamedOriginThatDoesNotMatchGetsOnlyVary() throws Exception {
    Exchange exchange = run(ORIGIN, "GET", "https://evil.example");

    assertThat(exchange.response.getHeaders("Vary")).containsExactly("Origin");
    assertThat(exchange.response.getHeaderNames())
        .noneMatch(name -> name.startsWith("Access-Control-"));
    assertThat(exchange.passedOn()).isTrue();
  }

  @Test
  void optionsIsAnsweredWith204AndGoesNoFurther() throws Exception {
    Exchange exchange = run(ORIGIN, "OPTIONS", ORIGIN);

    assertThat(exchange.response.getStatus()).isEqualTo(204);
    assertAllowed(exchange, ORIGIN);
    assertThat(exchange.passedOn()).as("a preflight reached the handler").isFalse();
    assertThat(exchange.response.getContentAsByteArray()).isEmpty();
  }

  private static void assertAllowed(Exchange exchange, String origin) {
    MockHttpServletResponse response = exchange.response;
    assertThat(response.getHeader("Access-Control-Allow-Origin")).isEqualTo(origin);
    assertThat(response.getHeader("Access-Control-Allow-Credentials")).isEqualTo("true");
    assertThat(response.getHeader("Access-Control-Allow-Methods")).isEqualTo("GET, POST, OPTIONS");
    assertThat(response.getHeader("Access-Control-Allow-Headers")).isEqualTo("Content-Type");
    assertThat(response.getHeader("Access-Control-Max-Age")).isEqualTo("600");
    assertThat(response.getHeaders("Vary")).containsExactly("Origin");
  }

  private static Exchange run(String allowedOrigin, String method, String origin) throws Exception {
    MockHttpServletRequest request = new MockHttpServletRequest(method, "/api/v1/products");
    request.addHeader("Origin", origin);
    MockHttpServletResponse response = new MockHttpServletResponse();
    MockFilterChain chain = new MockFilterChain();
    new CorsFilter(allowedOrigin).doFilter(request, response, chain);
    return new Exchange(response, chain);
  }

  private record Exchange(MockHttpServletResponse response, MockFilterChain chain) {
    boolean passedOn() {
      return chain.getRequest() != null;
    }
  }
}
