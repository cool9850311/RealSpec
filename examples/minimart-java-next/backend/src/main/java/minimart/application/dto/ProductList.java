package minimart.application.dto;

import com.fasterxml.jackson.annotation.JsonPropertyOrder;
import java.util.List;

/**
 * The 200 body of GET /api/v1/products. The list is not paginated; {@code total} is present so the
 * shape matches a paginated list if a later example needs one. {@code items} is never null: an
 * empty catalogue is {@code []}.
 */
@JsonPropertyOrder({"items", "total"})
public record ProductList(List<ProductItem> items, int total) {

  /** A list whose total is, by construction, the number of its items. */
  public static ProductList of(List<ProductItem> items) {
    return new ProductList(List.copyOf(items), items.size());
  }
}
