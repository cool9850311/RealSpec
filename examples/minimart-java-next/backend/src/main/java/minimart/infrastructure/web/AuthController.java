package minimart.infrastructure.web;

import jakarta.servlet.http.HttpServletRequest;
import jakarta.servlet.http.HttpServletResponse;
import java.time.Instant;
import minimart.application.dto.LoginRequest;
import minimart.application.dto.LoginResponse;
import minimart.application.dto.MeResponse;
import minimart.application.usecase.AuthUsecase;
import minimart.domain.DomainException;
import minimart.domain.ErrorCode;
import minimart.domain.User;
import minimart.infrastructure.security.JwtTokens;
import minimart.infrastructure.security.SessionCookies;
import org.springframework.security.core.Authentication;
import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.PostMapping;
import org.springframework.web.bind.annotation.RequestMapping;
import org.springframework.web.bind.annotation.RestController;

/** Serves POST /auth/login and GET /me. */
@RestController
@RequestMapping("/api/v1")
public class AuthController {

  private final AuthUsecase auth;
  private final JwtTokens tokens;
  private final SessionCookies cookies;

  /** A controller over {@code auth}, issuing sessions with {@code tokens} and {@code cookies}. */
  public AuthController(AuthUsecase auth, JwtTokens tokens, SessionCookies cookies) {
    this.auth = auth;
    this.tokens = tokens;
    this.cookies = cookies;
  }

  /**
   * Exchanges credentials for the session cookies.
   *
   * <p>The response body carries the username and role and nothing else: not the password hash, and
   * not the token, which exists only as an HttpOnly cookie. Alongside it goes the readable
   * session_hint flag, which carries no authority and exists so the front end can know it has a
   * session without asking a protected endpoint (see {@link SessionCookies}).
   */
  @PostMapping("/auth/login")
  public LoginResponse login(HttpServletRequest http, HttpServletResponse response) {
    LoginRequest request = JsonBody.parse(http, LoginRequest.class);
    if (isEmpty(request.username()) || isEmpty(request.password())) {
      throw new DomainException(ErrorCode.INVALID_REQUEST);
    }

    User user = auth.authenticate(request.username(), request.password());

    cookies.writeSession(response, tokens.issue(user, Instant.now()));
    return new LoginResponse(user.username(), user.role());
  }

  /** Returns the caller's own username, role and points balance. */
  @GetMapping("/me")
  public MeResponse me(Authentication caller) {
    return auth.me(Caller.id(caller));
  }

  private static boolean isEmpty(String value) {
    return value == null || value.isEmpty();
  }
}
