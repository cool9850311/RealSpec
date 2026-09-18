package minimart.infrastructure.security;

import static org.assertj.core.api.Assertions.assertThat;
import static org.mockito.ArgumentMatchers.anyLong;
import static org.mockito.Mockito.never;
import static org.mockito.Mockito.verify;
import static org.mockito.Mockito.when;
import static org.springframework.test.web.servlet.request.MockMvcRequestBuilders.get;
import static org.springframework.test.web.servlet.request.MockMvcRequestBuilders.post;
import static org.springframework.test.web.servlet.result.MockMvcResultMatchers.content;
import static org.springframework.test.web.servlet.result.MockMvcResultMatchers.header;
import static org.springframework.test.web.servlet.result.MockMvcResultMatchers.status;

import jakarta.servlet.http.Cookie;
import java.time.Instant;
import java.util.List;
import minimart.application.dto.AdminOrderList;
import minimart.application.dto.MeResponse;
import minimart.application.usecase.AuthUsecase;
import minimart.application.usecase.OrderUsecase;
import minimart.application.usecase.ProductUsecase;
import minimart.domain.User;
import minimart.testsupport.SetCookies;
import minimart.testsupport.SetCookies.SetCookie;
import minimart.testsupport.StubRoleLookup;
import minimart.testsupport.WebSliceTestConfig;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;
import org.springframework.beans.factory.annotation.Autowired;
import org.springframework.boot.webmvc.test.autoconfigure.WebMvcTest;
import org.springframework.context.annotation.Import;
import org.springframework.mock.web.MockHttpServletResponse;
import org.springframework.test.context.bean.override.mockito.MockitoBean;
import org.springframework.test.web.servlet.MockMvc;
import org.springframework.test.web.servlet.MvcResult;

/**
 * The real security filter chain, in front of the real controllers, with the use cases and the role
 * lookup replaced — so every outcome below is decided by the chain alone.
 */
@WebMvcTest
@Import({SecurityConfig.class, WebSliceTestConfig.class})
class SecurityChainTest {

  /** The user every RequireRole case's token is issued for. */
  static final int CALLER_ID = 3;

  @Autowired MockMvc mvc;
  @Autowired JwtTokens tokens;
  @Autowired StubRoleLookup roles;

  @MockitoBean AuthUsecase auth;
  @MockitoBean ProductUsecase products;
  @MockitoBean OrderUsecase orders;

  @BeforeEach
  void resetRoles() {
    roles.reset();
  }

  /**
   * A stale hint must not survive a 401, or the SPA would probe a protected endpoint on every page
   * load for the rest of the browser session — and the expiry only replaces the hint if its
   * attributes are the ones login wrote.
   */
  @Test
  void withoutACookieTheAnswerIs401AndTheHintIsExpiredWithTheLoginAttributes() throws Exception {
    when(auth.authenticate("alice", "secret123"))
        .thenReturn(new User(1, "alice", "", User.ROLE_GUEST, 100));
    MockHttpServletResponse login =
        mvc.perform(
                post("/api/v1/auth/login")
                    .content("{\"username\":\"alice\",\"password\":\"secret123\"}"))
            .andExpect(status().isOk())
            .andReturn()
            .getResponse();
    SetCookie issued = only(SetCookies.named(login, SessionCookies.SESSION_HINT));

    MockHttpServletResponse response =
        mvc.perform(get("/api/v1/me").cookie(new Cookie(SessionCookies.SESSION_HINT, "1")))
            .andExpect(status().isUnauthorized())
            .andExpect(content().string("{\"error\":\"UNAUTHENTICATED\"}"))
            .andExpect(header().doesNotExist("WWW-Authenticate"))
            .andReturn()
            .getResponse();

    SetCookie expired = only(SetCookies.named(response, SessionCookies.SESSION_HINT));
    assertThat(expired.value()).isEmpty();
    assertThat(expired.get("Max-Age")).isEqualTo("0");
    assertThat(expired.get("Path")).isEqualTo(issued.get("Path")).isEqualTo("/");
    assertThat(expired.get("SameSite")).isEqualTo(issued.get("SameSite")).isEqualTo("Lax");
    assertThat(expired.has("Secure")).isEqualTo(issued.has("Secure")).isTrue();
    assertThat(expired.has("HttpOnly")).isEqualTo(issued.has("HttpOnly")).isFalse();
    verify(auth, never()).me(anyLong());
  }

  // ── RequireRole ──────────────────────────────────────────────────────────────────────────
  //
  // Every case below presents a token that is beyond reproach — correctly signed, unexpired, and
  // claiming admin. What separates them is only what the lookup says, which is what the
  // assertions are for: if the chain ever read the role claim instead of asking, all four would
  // pass the guard.

  @Test
  void theStoredRoleMatchesSoTheHandlerRuns() throws Exception {
    when(orders.listAll()).thenReturn(AdminOrderList.of(List.of()));
    roles.answers(User.ROLE_ADMIN);

    adminOrders(tokenClaimingAdmin())
        .andExpect(status().isOk())
        .andExpect(content().string("{\"items\":[],\"total\":0}"));

    verify(orders).listAll();
    assertThat(roles.lookedUp()).containsExactly((long) CALLER_ID);
  }

  @Test
  void theStoredRoleIsGuestSoTheAnswerIs403AndTheHandlerDoesNotRun() throws Exception {
    roles.answers(User.ROLE_GUEST);

    adminOrders(tokenClaimingAdmin())
        .andExpect(status().isForbidden())
        .andExpect(content().string("{\"error\":\"FORBIDDEN\"}"));

    verify(orders, never()).listAll();
    assertThat(roles.lookedUp()).containsExactly((long) CALLER_ID);
  }

  @Test
  void theSubjectNamesNoUserSoTheAnswerIs401() throws Exception {
    roles.findsNoUser();

    adminOrders(tokenClaimingAdmin())
        .andExpect(status().isUnauthorized())
        .andExpect(content().string("{\"error\":\"UNAUTHENTICATED\"}"));

    verify(orders, never()).listAll();
    assertThat(roles.lookedUp()).containsExactly((long) CALLER_ID);
  }

  @Test
  void theLookupItselfFailsSoTheAnswerIs500() throws Exception {
    roles.fails(new IllegalStateException("the database is down"));

    adminOrders(tokenClaimingAdmin())
        .andExpect(status().isInternalServerError())
        .andExpect(content().string("{\"error\":\"INTERNAL_ERROR\"}"));

    verify(orders, never()).listAll();
    assertThat(roles.lookedUp()).containsExactly((long) CALLER_ID);
  }

  /**
   * A 403 is not a broken session. If it expired the hint the SPA would conclude it had been signed
   * out and send the user to the login page over a permission they were never going to have.
   */
  @Test
  void a403LeavesTheSessionHintAlone() throws Exception {
    roles.answers(User.ROLE_GUEST);

    MockHttpServletResponse response =
        mvc.perform(
                get("/api/v1/admin/orders")
                    .cookie(
                        new Cookie(SessionCookies.TOKEN, tokenClaimingAdmin()),
                        new Cookie(SessionCookies.SESSION_HINT, "1")))
            .andExpect(status().isForbidden())
            .andReturn()
            .getResponse();

    assertThat(SetCookies.named(response, SessionCookies.SESSION_HINT)).isEmpty();
  }

  /** Stateless means no HttpSession on any path: anonymous, logging in, or authenticated. */
  @Test
  void noSessionIsEverCreated() throws Exception {
    when(auth.authenticate("alice", "secret123"))
        .thenReturn(new User(1, "alice", "", User.ROLE_GUEST, 100));
    when(auth.me(1)).thenReturn(new MeResponse("alice", User.ROLE_GUEST, 100));

    List<MvcResult> results =
        List.of(
            mvc.perform(
                    post("/api/v1/auth/login")
                        .content("{\"username\":\"alice\",\"password\":\"secret123\"}"))
                .andExpect(status().isOk())
                .andReturn(),
            mvc.perform(get("/api/v1/me").cookie(new Cookie(SessionCookies.TOKEN, guestToken())))
                .andExpect(status().isOk())
                .andReturn(),
            mvc.perform(get("/api/v1/me")).andExpect(status().isUnauthorized()).andReturn());

    for (MvcResult result : results) {
      assertThat(result.getRequest().getSession(false)).isNull();
      assertThat(SetCookies.named(result.getResponse(), "JSESSIONID")).isEmpty();
    }
  }

  /**
   * The contract has no logout endpoint. Spring Security's would answer POST /logout with a
   * redirect; with it switched off the path is simply not part of this API.
   */
  @Test
  void logoutIsNotHandledBySpringSecurity() throws Exception {
    MockHttpServletResponse response =
        mvc.perform(post("/logout").cookie(new Cookie(SessionCookies.TOKEN, tokenClaimingAdmin())))
            .andExpect(status().isNotFound())
            .andExpect(header().doesNotExist("Location"))
            .andReturn()
            .getResponse();

    assertThat(response.getHeaders("Set-Cookie")).isEmpty();
  }

  /** The token cookie is the one way in; a perfectly good token in a Bearer header is not. */
  @Test
  void anAuthorizationBearerHeaderIsNotACredential() throws Exception {
    mvc.perform(get("/api/v1/me").header("Authorization", "Bearer " + guestToken()))
        .andExpect(status().isUnauthorized())
        .andExpect(content().string("{\"error\":\"UNAUTHENTICATED\"}"));

    verify(auth, never()).me(anyLong());
  }

  private org.springframework.test.web.servlet.ResultActions adminOrders(String token)
      throws Exception {
    return mvc.perform(get("/api/v1/admin/orders").cookie(new Cookie(SessionCookies.TOKEN, token)));
  }

  /** A token whose claim says admin in every case, so the claim cannot explain any outcome. */
  private String tokenClaimingAdmin() {
    return tokens.issue(new User(CALLER_ID, "alice", "", User.ROLE_ADMIN, 0), Instant.now());
  }

  private String guestToken() {
    return tokens.issue(new User(1, "alice", "", User.ROLE_GUEST, 100), Instant.now());
  }

  private static SetCookie only(List<SetCookie> cookies) {
    assertThat(cookies).hasSize(1);
    return cookies.get(0);
  }
}
