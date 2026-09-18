package minimart.application.dto;

import com.fasterxml.jackson.annotation.JsonProperty;
import com.fasterxml.jackson.annotation.JsonPropertyOrder;

/**
 * One entry of the caller's order history (openapi schema {@code Order}). {@code productName} is
 * the product's name as it is now; {@code costPoints} is the price that was actually paid.
 *
 * @param createdAt RFC 3339 in UTC, e.g. {@code 2026-01-01T00:00:00Z}
 */
@JsonPropertyOrder({"id", "product_name", "cost_points", "created_at"})
public record OrderItem(
    int id,
    @JsonProperty("product_name") String productName,
    @JsonProperty("cost_points") int costPoints,
    @JsonProperty("created_at") String createdAt) {}
