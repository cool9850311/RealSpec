package minimart.application.dto;

import com.fasterxml.jackson.annotation.JsonProperty;
import com.fasterxml.jackson.annotation.JsonPropertyOrder;

/** The 201 body of POST /api/v1/orders. */
@JsonPropertyOrder({"id", "cost_points", "balance_after"})
public record CreateOrderResponse(
    int id,
    @JsonProperty("cost_points") int costPoints,
    @JsonProperty("balance_after") int balanceAfter) {}
