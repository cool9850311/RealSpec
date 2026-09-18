package minimart.domain;

/** The decision at the heart of a redemption, independent of how it is stored or transported. */
public final class Redemption {

  private Redemption() {}

  /**
   * Decides whether a caller holding {@code balance} points may redeem {@code product} and, if so,
   * what products.stock must become. {@code product} is null when no active product with the
   * requested id exists.
   *
   * <p>The precedence is fixed and is part of the contract:
   *
   * <pre>
   *   PRODUCT_NOT_FOUND → INSUFFICIENT_POINTS → OUT_OF_STOCK
   * </pre>
   *
   * A user who cannot afford anything is told about their points before being told about stock, so
   * the message they see does not change as other people shop.
   *
   * @return the next value of products.stock ({@link Stock#UNLIMITED} stays unlimited)
   * @throws DomainException with the first code of the precedence that applies
   */
  public static int evaluate(Product product, int balance) {
    if (product == null || !product.active()) {
      throw new DomainException(ErrorCode.PRODUCT_NOT_FOUND);
    }
    if (balance < product.costPoints()) {
      throw new DomainException(ErrorCode.INSUFFICIENT_POINTS);
    }
    return Stock.next(product.stock());
  }
}
