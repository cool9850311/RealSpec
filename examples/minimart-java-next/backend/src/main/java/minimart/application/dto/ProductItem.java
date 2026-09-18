package minimart.application.dto;

import com.fasterxml.jackson.annotation.JsonProperty;
import com.fasterxml.jackson.annotation.JsonPropertyOrder;

/**
 * One entry of the catalogue (openapi schema {@code Product}). {@code stock} is the number of units
 * left, or -1 for unlimited.
 */
@JsonPropertyOrder({"id", "name", "cost_points", "stock"})
public record ProductItem(
    int id, String name, @JsonProperty("cost_points") int costPoints, int stock) {}
