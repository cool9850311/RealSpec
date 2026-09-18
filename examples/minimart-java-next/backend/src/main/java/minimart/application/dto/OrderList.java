package minimart.application.dto;

import com.fasterxml.jackson.annotation.JsonPropertyOrder;
import java.util.List;

/** The 200 body of GET /api/v1/orders. {@code items} is never null. */
@JsonPropertyOrder({"items", "total"})
public record OrderList(List<OrderItem> items, int total) {

  /** A list whose total is, by construction, the number of its items. */
  public static OrderList of(List<OrderItem> items) {
    return new OrderList(List.copyOf(items), items.size());
  }
}
