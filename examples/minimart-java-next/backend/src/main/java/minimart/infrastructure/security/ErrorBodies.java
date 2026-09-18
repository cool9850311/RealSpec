package minimart.infrastructure.security;

import jakarta.servlet.http.HttpServletResponse;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import org.springframework.http.MediaType;

/**
 * Writes the contract's flat error body from code that runs before, or outside, Spring MVC — the
 * filters and the security handlers — where there is no message converter to do it.
 *
 * <p>The body is a constant per code, so it is written as literal bytes rather than through a JSON
 * library: there is nothing to escape, and nothing in the failure path that could itself fail.
 */
final class ErrorBodies {

  private ErrorBodies() {}

  static void write(HttpServletResponse response, int status, String code) throws IOException {
    byte[] body = ("{\"error\":\"" + code + "\"}").getBytes(StandardCharsets.UTF_8);
    response.setStatus(status);
    response.setContentType(MediaType.APPLICATION_JSON_VALUE);
    response.setContentLength(body.length);
    response.getOutputStream().write(body);
    response.flushBuffer();
  }
}
