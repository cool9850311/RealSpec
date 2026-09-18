package minimart.domain;

import static org.assertj.core.api.Assertions.assertThat;
import static org.assertj.core.api.Assertions.assertThatThrownBy;

import org.junit.jupiter.api.Test;

/**
 * Pins the order the contract fixes: PRODUCT_NOT_FOUND → INSUFFICIENT_POINTS → OUT_OF_STOCK. Each
 * refusal below is a case where more than one rule applies, so only the precedence distinguishes
 * the answers.
 */
class RedemptionTest {

  @Test
  void noSuchProductIsNotFound() {
    assertRefused(null, 1000, ErrorCode.PRODUCT_NOT_FOUND);
  }

  @Test
  void inactiveAndUnaffordableIsNotFoundNotInsufficient() {
    assertRefused(product(50, 5, false), 0, ErrorCode.PRODUCT_NOT_FOUND);
  }

  @Test
  void affordableButSoldOutIsOutOfStock() {
    assertRefused(product(50, 0, true), 100, ErrorCode.OUT_OF_STOCK);
  }

  @Test
  void unaffordableAndSoldOutReportsThePointsFirst() {
    assertRefused(product(50, 0, true), 10, ErrorCode.INSUFFICIENT_POINTS);
  }

  @Test
  void exactlyAffordableIsAllowed() {
    assertThat(Redemption.evaluate(product(50, 1, true), 50)).isZero();
  }

  @Test
  void unlimitedStockStaysUnlimited() {
    assertThat(Redemption.evaluate(product(20, Stock.UNLIMITED, true), 100))
        .isEqualTo(Stock.UNLIMITED);
  }

  private static Product product(int costPoints, int stock, boolean active) {
    return new Product(1, "Sticker Pack", costPoints, stock, active);
  }

  private static void assertRefused(Product product, int balance, ErrorCode want) {
    assertThatThrownBy(() -> Redemption.evaluate(product, balance))
        .isInstanceOfSatisfying(DomainException.class, e -> assertThat(e.code()).isEqualTo(want));
  }
}
