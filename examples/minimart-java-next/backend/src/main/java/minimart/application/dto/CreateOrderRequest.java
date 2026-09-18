package minimart.application.dto;

import com.fasterxml.jackson.annotation.JsonProperty;

/**
 * The body of POST /api/v1/orders.
 *
 * <p>{@code productId} is a {@link Long} rather than an {@code int} for two reasons. It is boxed so
 * that "absent" and "0" are distinguishable: both are 400 INVALID_REQUEST, but only because the
 * controller rejects them, not because a default silently stood in for a missing field. And it is
 * 64-bit because the Go service's is: an id beyond the int range is a well-formed request for a
 * product that does not exist (404), not a malformed one (400).
 */
public record CreateOrderRequest(@JsonProperty("product_id") Long productId) {}
