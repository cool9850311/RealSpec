package minimart.infrastructure.web;

import static org.mockito.ArgumentMatchers.anyLong;
import static org.mockito.ArgumentMatchers.anyString;
import static org.mockito.Mockito.never;
import static org.mockito.Mockito.verify;
import static org.mockito.Mockito.when;
import static org.springframework.test.web.servlet.request.MockMvcRequestBuilders.post;
import static org.springframework.test.web.servlet.result.MockMvcResultMatchers.content;
import static org.springframework.test.web.servlet.result.MockMvcResultMatchers.status;

import jakarta.servlet.http.Cookie;
import java.time.Instant;
import minimart.application.dto.CreateOrderResponse;
import minimart.application.usecase.AuthUsecase;
import minimart.application.usecase.OrderUsecase;
import minimart.application.usecase.ProductUsecase;
import minimart.domain.User;
import minimart.infrastructure.security.JwtTokens;
import minimart.infrastructure.security.SecurityConfig;
import minimart.infrastructure.security.SessionCookies;
import minimart.testsupport.WebSliceTestConfig;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.params.ParameterizedTest;
import org.junit.jupiter.params.provider.ValueSource;
import org.springframework.beans.factory.annotation.Autowired;
import org.springframework.boot.webmvc.test.autoconfigure.WebMvcTest;
import org.springframework.context.annotation.Import;
import org.springframework.http.MediaType;
import org.springframework.test.context.bean.override.mockito.MockitoBean;
import org.springframework.test.web.servlet.MockMvc;

/**
 * Request parsing, through the real controllers: every body the Go service's decoder refuses is
 * refused here with the same 400, before any use case runs.
 */
@WebMvcTest
@Import({SecurityConfig.class, WebSliceTestConfig.class})
class JsonBodyTest {

  static final String INVALID_REQUEST = "{\"error\":\"INVALID_REQUEST\"}";

  @Autowired MockMvc mvc;
  @Autowired JwtTokens tokens;

  @MockitoBean AuthUsecase auth;
  @MockitoBean ProductUsecase products;
  @MockitoBean OrderUsecase orders;

  @ParameterizedTest(name = "{0}")
  @ValueSource(
      strings = {
        // an unknown field
        "{\"product_id\":1,\"quantity\":2}",
        // content after the JSON value
        "{\"product_id\":1} {\"product_id\":2}",
        "{\"product_id\":1}garbage",
        // a number written as a string
        "{\"product_id\":\"1\"}",
        // a float, including one with no fractional part
        "{\"product_id\":1.5}",
        "{\"product_id\":1.0}",
        // null, as the body and as the field
        "null",
        "{\"product_id\":null}",
        // absent
        "{}",
        // zero and negative
        "{\"product_id\":0}",
        "{\"product_id\":-1}",
      })
  void aMalformedOrderBodyIs400(String body) throws Exception {
    mvc.perform(
            post("/api/v1/orders")
                .cookie(session())
                .contentType(MediaType.APPLICATION_JSON)
                .content(body))
        .andExpect(status().isBadRequest())
        .andExpect(content().string(INVALID_REQUEST));

    verify(orders, never()).redeem(anyLong(), anyLong());
  }

  @Test
  void anEmptyBodyIs400() throws Exception {
    mvc.perform(post("/api/v1/orders").cookie(session()).contentType(MediaType.APPLICATION_JSON))
        .andExpect(status().isBadRequest())
        .andExpect(content().string(INVALID_REQUEST));

    verify(orders, never()).redeem(anyLong(), anyLong());
  }

  /** The Go service never reads Content-Type, so a body without one is still a body. */
  @Test
  void aBodyWithoutContentTypeIsAccepted() throws Exception {
    when(orders.redeem(1, 1)).thenReturn(new CreateOrderResponse(7, 50, 50));

    mvc.perform(post("/api/v1/orders").cookie(session()).content("{\"product_id\":1}"))
        .andExpect(status().isCreated())
        .andExpect(content().string("{\"id\":7,\"cost_points\":50,\"balance_after\":50}"));

    verify(orders).redeem(1, 1);
  }

  /**
   * Nor does any other Content-Type change what the body is. A form type is the case that matters:
   * a servlet container parses a form POST's body into request parameters, and anything that reads
   * the body through them instead of the raw stream sees something other than the bytes sent.
   * (multipart/form-data is held by {@link MultipartResolutionTest}: MockMvc cannot reproduce how a
   * container's multipart parsing consumes the body.)
   */
  @ParameterizedTest(name = "{0}")
  @ValueSource(
      strings = {
        MediaType.APPLICATION_FORM_URLENCODED_VALUE,
        MediaType.TEXT_PLAIN_VALUE,
        MediaType.APPLICATION_OCTET_STREAM_VALUE
      })
  void aBodyIsReadAsSentWhateverItsContentType(String contentType) throws Exception {
    when(orders.redeem(1, 1)).thenReturn(new CreateOrderResponse(7, 50, 50));
    when(auth.authenticate("alice", "secret123"))
        .thenReturn(new User(1, "alice", "", User.ROLE_GUEST, 100));

    mvc.perform(
            post("/api/v1/orders")
                .cookie(session())
                .contentType(contentType)
                .content("{\"product_id\":1}"))
        .andExpect(status().isCreated())
        .andExpect(content().string("{\"id\":7,\"cost_points\":50,\"balance_after\":50}"));
    mvc.perform(
            post("/api/v1/auth/login")
                .contentType(contentType)
                .content("{\"username\":\"alice\",\"password\":\"secret123\"}"))
        .andExpect(status().isOk())
        .andExpect(content().string("{\"username\":\"alice\",\"role\":\"guest\"}"));

    verify(orders).redeem(1, 1);
    verify(auth).authenticate("alice", "secret123");
  }

  @ParameterizedTest(name = "{0}")
  @ValueSource(
      strings = {
        "{\"username\":\"\",\"password\":\"secret123\"}",
        "{\"username\":\"alice\",\"password\":\"\"}",
      })
  void anEmptyLoginFieldIs400(String body) throws Exception {
    mvc.perform(post("/api/v1/auth/login").contentType(MediaType.APPLICATION_JSON).content(body))
        .andExpect(status().isBadRequest())
        .andExpect(content().string(INVALID_REQUEST));

    verify(auth, never()).authenticate(anyString(), anyString());
  }

  private Cookie session() {
    return new Cookie(
        SessionCookies.TOKEN,
        tokens.issue(new User(1, "alice", "", User.ROLE_GUEST, 100), Instant.now()));
  }
}
