package minimart.infrastructure.web;

import jakarta.servlet.http.HttpServletRequest;
import jakarta.servlet.http.HttpServletResponse;
import java.io.IOException;
import java.util.EnumMap;
import java.util.Map;
import minimart.application.dto.ErrorResponse;
import minimart.domain.DomainException;
import minimart.domain.ErrorCode;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;
import org.springframework.http.HttpStatus;
import org.springframework.http.ResponseEntity;
import org.springframework.http.converter.HttpMessageNotReadableException;
import org.springframework.web.HttpRequestMethodNotSupportedException;
import org.springframework.web.bind.annotation.ExceptionHandler;
import org.springframework.web.bind.annotation.RestControllerAdvice;

/**
 * Renders every failure a handler raises as the flat {@code { "error": "<CODE>" }} body.
 *
 * <p>The failures the security filter chain decides (401 for a missing or broken session, 403 for a
 * role, 500 for a failing role lookup) never reach a handler and are rendered there; this is the
 * rest.
 */
@RestControllerAdvice
public class ApiExceptionHandler {

  private static final Logger log = LoggerFactory.getLogger(ApiExceptionHandler.class);

  /**
   * Maps every error code of the contract to its status. The table is the responses of
   * spec/openapi/openapi.yaml, and it is the only place a status is chosen.
   */
  static final Map<ErrorCode, HttpStatus> STATUS_BY_CODE = new EnumMap<>(ErrorCode.class);

  static {
    STATUS_BY_CODE.put(ErrorCode.INVALID_REQUEST, HttpStatus.BAD_REQUEST);
    STATUS_BY_CODE.put(ErrorCode.INVALID_CREDENTIALS, HttpStatus.UNAUTHORIZED);
    STATUS_BY_CODE.put(ErrorCode.UNAUTHENTICATED, HttpStatus.UNAUTHORIZED);
    STATUS_BY_CODE.put(ErrorCode.FORBIDDEN, HttpStatus.FORBIDDEN);
    STATUS_BY_CODE.put(ErrorCode.PRODUCT_NOT_FOUND, HttpStatus.NOT_FOUND);
    STATUS_BY_CODE.put(ErrorCode.INSUFFICIENT_POINTS, HttpStatus.UNPROCESSABLE_CONTENT);
    STATUS_BY_CODE.put(ErrorCode.OUT_OF_STOCK, HttpStatus.UNPROCESSABLE_CONTENT);
  }

  /** One of the contract's codes: rendered with its status, and not logged — it is an answer. */
  @ExceptionHandler(DomainException.class)
  ResponseEntity<ErrorResponse> domain(DomainException e, HttpServletRequest request) {
    HttpStatus status = STATUS_BY_CODE.get(e.code());
    if (status == null) {
      // A code added to the enum without a row above. Loud, because it is a defect.
      return internalError(e, request);
    }
    return ResponseEntity.status(status).body(new ErrorResponse(e.code().name()));
  }

  /**
   * A body Spring itself could not read. Controllers read the body themselves (see {@link
   * JsonBody}), so this is not expected in practice; it is the contract's answer for a malformed
   * request should it ever occur.
   */
  @ExceptionHandler(HttpMessageNotReadableException.class)
  ResponseEntity<ErrorResponse> unreadable() {
    return ResponseEntity.badRequest().body(new ErrorResponse(ErrorCode.INVALID_REQUEST.name()));
  }

  /**
   * A path the API has, with a method it does not. That is "no such route" — the same framework 404
   * as a path the API does not have, which is also what the Go service's router answers — rather
   * than a 405 advertising the methods that do exist.
   */
  @ExceptionHandler(HttpRequestMethodNotSupportedException.class)
  void methodNotSupported(HttpServletResponse response) throws IOException {
    response.sendError(HttpServletResponse.SC_NOT_FOUND);
  }

  /**
   * Anything else.
   *
   * <p>Spring MVC's own exceptions (a path no handler maps, and the like) carry their own status
   * and are handed back to the framework to answer as it always does — "that path is not part of
   * this API" is not one of the contract's failures, and inventing a code for it would be inventing
   * contract. Everything else is a defect in this service, not a message for the caller: it is
   * logged in full and answered with a 500 that discloses nothing about the failure.
   */
  @ExceptionHandler(Exception.class)
  ResponseEntity<ErrorResponse> unexpected(Exception e, HttpServletRequest request)
      throws Exception {
    if (e instanceof org.springframework.web.ErrorResponse) {
      throw e;
    }
    return internalError(e, request);
  }

  private static ResponseEntity<ErrorResponse> internalError(
      Exception e, HttpServletRequest request) {
    log.error("unhandled error on {} {}", request.getMethod(), request.getRequestURI(), e);
    return ResponseEntity.internalServerError().body(new ErrorResponse("INTERNAL_ERROR"));
  }
}
