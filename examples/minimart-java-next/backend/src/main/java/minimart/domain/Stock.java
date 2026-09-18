package minimart.domain;

/** The arithmetic of products.stock, including its one sentinel. */
public final class Stock {

  /**
   * The sentinel stored in products.stock for a product that never runs out. It is never
   * decremented and never written by any other value.
   */
  public static final int UNLIMITED = -1;

  private Stock() {}

  /**
   * Returns the value products.stock must hold after one redemption.
   *
   * <pre>
   *   -1 → -1   (unlimited, the sentinel is never written away)
   *    n → n-1  for n &gt; 0
   *    0 → OUT_OF_STOCK
   * </pre>
   *
   * Any other non-positive value is treated as out of stock as well: the only writer of the column
   * is the redemption transaction, which cannot produce one, so reaching that branch means the data
   * is already wrong and the safe answer is to refuse rather than to sell.
   *
   * @throws DomainException {@link ErrorCode#OUT_OF_STOCK} when there is nothing left to sell
   */
  public static int next(int stock) {
    if (stock == UNLIMITED) {
      return UNLIMITED;
    }
    if (stock > 0) {
      return stock - 1;
    }
    throw new DomainException(ErrorCode.OUT_OF_STOCK);
  }
}
