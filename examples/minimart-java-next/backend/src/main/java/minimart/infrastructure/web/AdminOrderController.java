package minimart.infrastructure.web;

import minimart.application.dto.AdminOrderList;
import minimart.application.usecase.OrderUsecase;
import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.RequestMapping;
import org.springframework.web.bind.annotation.RestController;

/** Serves GET /admin/orders. */
@RestController
@RequestMapping("/api/v1/admin")
public class AdminOrderController {

  private final OrderUsecase orders;

  /** A controller over {@code orders}. */
  public AdminOrderController(OrderUsecase orders) {
    this.orders = orders;
  }

  /**
   * Returns every user's orders, newest first.
   *
   * <p>There is no role check here. The route is guarded by RoleAuthorizationManager in the
   * security filter chain and that is the only place the rule lives: an authorisation restated in
   * the handler is an authorisation that can drift from the one in front of it, and the drift is
   * invisible until someone removes the wrong one.
   */
  @GetMapping("/orders")
  public AdminOrderList listAll() {
    return orders.listAll();
  }
}
