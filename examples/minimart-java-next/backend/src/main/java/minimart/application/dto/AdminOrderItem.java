package minimart.application.dto;

import com.fasterxml.jackson.annotation.JsonProperty;
import com.fasterxml.jackson.annotation.JsonPropertyOrder;

/**
 * One entry of the all-users order history (openapi schema {@code AdminOrder}): an order plus the
 * username of the user who placed it.
 *
 * <p>It is a type of its own rather than an extension of {@link OrderItem}, because the two
 * responses are two contracts: a field added to the admin listing must not be able to widen what
 * GET /api/v1/orders hands to its own caller.
 */
@JsonPropertyOrder({"id", "username", "product_name", "cost_points", "created_at"})
public record AdminOrderItem(
    int id,
    String username,
    @JsonProperty("product_name") String productName,
    @JsonProperty("cost_points") int costPoints,
    @JsonProperty("created_at") String createdAt) {}
