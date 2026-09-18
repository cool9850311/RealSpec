package minimart.application.dto;

import com.fasterxml.jackson.annotation.JsonPropertyOrder;
import java.util.List;

/** The 200 body of GET /api/v1/admin/orders. {@code items} is never null. */
@JsonPropertyOrder({"items", "total"})
public record AdminOrderList(List<AdminOrderItem> items, int total) {

  /** A list whose total is, by construction, the number of its items. */
  public static AdminOrderList of(List<AdminOrderItem> items) {
    return new AdminOrderList(List.copyOf(items), items.size());
  }
}
