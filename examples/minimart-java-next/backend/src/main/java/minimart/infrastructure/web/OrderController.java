package minimart.infrastructure.web;

import jakarta.servlet.http.HttpServletRequest;
import minimart.application.dto.CreateOrderRequest;
import minimart.application.dto.CreateOrderResponse;
import minimart.application.dto.OrderList;
import minimart.application.usecase.OrderUsecase;
import minimart.domain.DomainException;
import minimart.domain.ErrorCode;
import org.springframework.http.HttpStatus;
import org.springframework.security.core.Authentication;
import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.PostMapping;
import org.springframework.web.bind.annotation.RequestMapping;
import org.springframework.web.bind.annotation.ResponseStatus;
import org.springframework.web.bind.annotation.RestController;

/** Serves POST /orders and GET /orders. */
@RestController
@RequestMapping("/api/v1")
public class OrderController {

  private final OrderUsecase orders;

  /** A controller over {@code orders}. */
  public OrderController(OrderUsecase orders) {
    this.orders = orders;
  }

  /**
   * Redeems one product for points.
   *
   * <p>The endpoint is intentionally not idempotent: two requests are two redemptions, which is the
   * correct behaviour for a shop.
   */
  @PostMapping("/orders")
  @ResponseStatus(HttpStatus.CREATED)
  public CreateOrderResponse create(HttpServletRequest http, Authentication caller) {
    long userId = Caller.id(caller);

    CreateOrderRequest request = JsonBody.parse(http, CreateOrderRequest.class);
    if (request.productId() == null || request.productId() <= 0) {
      throw new DomainException(ErrorCode.INVALID_REQUEST);
    }

    return orders.redeem(userId, request.productId());
  }

  /** Returns the caller's own orders, newest first. */
  @GetMapping("/orders")
  public OrderList list(Authentication caller) {
    return orders.listMine(Caller.id(caller));
  }
}
