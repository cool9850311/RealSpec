package minimart.application.usecase;

import static java.time.temporal.ChronoField.NANO_OF_SECOND;

import java.time.Instant;
import java.time.ZoneOffset;
import java.time.format.DateTimeFormatter;
import java.time.format.DateTimeFormatterBuilder;
import minimart.application.dto.AdminOrderItem;
import minimart.application.dto.AdminOrderList;
import minimart.application.dto.CreateOrderResponse;
import minimart.application.dto.OrderItem;
import minimart.application.dto.OrderList;
import minimart.domain.DomainException;
import minimart.domain.ErrorCode;
import minimart.infrastructure.repository.OrderRepository;
import minimart.infrastructure.repository.RedemptionResult;
import minimart.infrastructure.repository.RowNotFoundException;

/** Redeems products and lists orders. */
public class OrderUsecase {

  /**
   * created_at on the wire: RFC 3339 in UTC with as many fractional digits as the value needs and
   * no trailing zeros — {@code 2026-01-01T00:00:00Z}, {@code 2026-01-01T00:00:00.12345Z}. That is
   * byte for byte what Go's {@code time.RFC3339Nano} produces for the same instant, and it is
   * rendered in UTC so the wire format is the one the contract shows regardless of the database
   * session's time zone.
   */
  static final DateTimeFormatter CREATED_AT =
      new DateTimeFormatterBuilder()
          .appendPattern("uuuu-MM-dd'T'HH:mm:ss")
          .appendFraction(NANO_OF_SECOND, 0, 9, true)
          .appendLiteral('Z')
          .toFormatter()
          .withZone(ZoneOffset.UTC);

  private final OrderRepository orders;

  /** A use case over {@code orders}. */
  public OrderUsecase(OrderRepository orders) {
    this.orders = orders;
  }

  /**
   * Performs one redemption on behalf of {@code userId}.
   *
   * <p>The three writes and the decision all happen inside {@link OrderRepository#redeem}, in one
   * transaction: splitting the read from the write here is exactly the race the contract forbids,
   * so there is nothing to orchestrate.
   *
   * @throws DomainException PRODUCT_NOT_FOUND, INSUFFICIENT_POINTS or OUT_OF_STOCK as decided under
   *     the locks, or UNAUTHENTICATED when the token's subject no longer exists
   */
  public CreateOrderResponse redeem(long userId, long productId) {
    RedemptionResult result;
    try {
      result = orders.redeem(userId, productId);
    } catch (RowNotFoundException subjectGone) {
      throw new DomainException(ErrorCode.UNAUTHENTICATED, subjectGone);
    }
    return new CreateOrderResponse(
        result.order().id(), result.order().costPoints(), result.balanceAfter());
  }

  /**
   * Returns every user's orders, newest first, each carrying its owner's username. The authority to
   * call it is established before the handler runs; nothing here re-checks it, because a rule
   * enforced in two places is a rule that can disagree with itself.
   */
  public AdminOrderList listAll() {
    return AdminOrderList.of(
        orders.listAll().stream()
            .map(
                o ->
                    new AdminOrderItem(
                        o.id(),
                        o.username(),
                        o.productName(),
                        o.costPoints(),
                        createdAt(o.createdAt())))
            .toList());
  }

  /** Returns the caller's own orders, newest first. */
  public OrderList listMine(long userId) {
    return OrderList.of(
        orders.listByUser(userId).stream()
            .map(
                o ->
                    new OrderItem(
                        o.id(), o.productName(), o.costPoints(), createdAt(o.createdAt())))
            .toList());
  }

  static String createdAt(Instant instant) {
    return CREATED_AT.format(instant);
  }
}
