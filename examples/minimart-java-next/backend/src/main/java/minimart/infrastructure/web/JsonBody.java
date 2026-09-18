package minimart.infrastructure.web;

import jakarta.servlet.http.HttpServletRequest;
import java.io.IOException;
import minimart.domain.DomainException;
import minimart.domain.ErrorCode;
import tools.jackson.core.JacksonException;
import tools.jackson.databind.DeserializationFeature;
import tools.jackson.databind.MapperFeature;
import tools.jackson.databind.cfg.CoercionAction;
import tools.jackson.databind.cfg.CoercionInputShape;
import tools.jackson.databind.json.JsonMapper;
import tools.jackson.databind.type.LogicalType;

/**
 * Reads a request body into a request record, as strictly as the Go service's decoder does.
 *
 * <p>Controllers hand over the request itself and the body is read here, from the servlet input
 * stream, rather than through Spring MVC's {@code @RequestBody}. Spring chooses a message converter
 * by Content-Type, so a missing or unusual header would be a 415 before the body was even looked
 * at; and even the byte-array converter does not see the bytes sent when the request is a form
 * POST, because Spring then rebuilds the body from the container's parsed request parameters. The
 * Go service never reads Content-Type, and neither does this: the body is the bytes on the wire,
 * whatever the header says.
 *
 * <p>The mapper is a dedicated one, never the application's shared ObjectMapper, and every setting
 * that decides what is accepted is stated below rather than inherited — Jackson's defaults are
 * lenient where Go's decoder is not, and differ between Jackson 2 and 3:
 *
 * <ul>
 *   <li>unknown fields are refused, because every request schema in openapi.yaml declares {@code
 *       additionalProperties: false}; a body with a misspelled key is a client bug and is far
 *       cheaper to find as a 400 than as a silently ignored field;
 *   <li>content after the JSON value is refused ({@code {...}garbage} is not a body);
 *   <li>no scalar coercion: {@code "1"} is not the integer 1, {@code 1} is not the string "1",
 *       {@code true} is neither, and {@code 1.5} (or {@code 1.0}) is not an integer — Jackson would
 *       otherwise convert or truncate each of them;
 *   <li>property names match case-insensitively, which is what Go's encoding/json does, so the two
 *       services accept exactly the same bodies;
 *   <li>an empty body and the literal {@code null} are not a request.
 * </ul>
 *
 * Every refusal is the same {@code 400 INVALID_REQUEST}; which rule it was is kept in the
 * exception's cause for the log, never sent to the caller.
 */
public final class JsonBody {

  private static final JsonMapper STRICT =
      JsonMapper.builder()
          .enable(DeserializationFeature.FAIL_ON_UNKNOWN_PROPERTIES)
          .enable(DeserializationFeature.FAIL_ON_TRAILING_TOKENS)
          .disable(DeserializationFeature.ACCEPT_FLOAT_AS_INT)
          .disable(MapperFeature.ALLOW_COERCION_OF_SCALARS)
          .enable(MapperFeature.ACCEPT_CASE_INSENSITIVE_PROPERTIES)
          .withCoercionConfig(
              LogicalType.Integer,
              c ->
                  c.setCoercion(CoercionInputShape.String, CoercionAction.Fail)
                      .setCoercion(CoercionInputShape.EmptyString, CoercionAction.Fail)
                      .setCoercion(CoercionInputShape.Float, CoercionAction.Fail)
                      .setCoercion(CoercionInputShape.Boolean, CoercionAction.Fail))
          .withCoercionConfig(
              LogicalType.Textual,
              c ->
                  c.setCoercion(CoercionInputShape.Integer, CoercionAction.Fail)
                      .setCoercion(CoercionInputShape.Float, CoercionAction.Fail)
                      .setCoercion(CoercionInputShape.Boolean, CoercionAction.Fail))
          .build();

  private JsonBody() {}

  /**
   * Reads {@code request}'s body exactly as it was sent and parses it as one {@code type}.
   *
   * <p>A body that cannot be read — the client went away half way through it — is refused like one
   * that cannot be parsed, as the Go service's decoder refuses it.
   *
   * @throws DomainException INVALID_REQUEST for anything that is not exactly one well-formed,
   *     well-typed JSON object of that shape
   */
  public static <T> T parse(HttpServletRequest request, Class<T> type) {
    byte[] body;
    try {
      body = request.getInputStream().readAllBytes();
    } catch (IOException unreadable) {
      throw new DomainException(ErrorCode.INVALID_REQUEST, unreadable);
    }
    return parse(body, type);
  }

  /**
   * Parses {@code body} as one {@code type}.
   *
   * @throws DomainException INVALID_REQUEST for anything that is not exactly one well-formed,
   *     well-typed JSON object of that shape
   */
  static <T> T parse(byte[] body, Class<T> type) {
    if (body == null || body.length == 0) {
      throw new DomainException(ErrorCode.INVALID_REQUEST);
    }
    T value;
    try {
      value = STRICT.readValue(body, type);
    } catch (JacksonException malformed) {
      throw new DomainException(ErrorCode.INVALID_REQUEST, malformed);
    }
    if (value == null) {
      throw new DomainException(ErrorCode.INVALID_REQUEST);
    }
    return value;
  }
}
