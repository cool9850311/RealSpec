package minimart.infrastructure.repository;

import java.time.OffsetDateTime;
import java.util.List;
import minimart.domain.Order;
import minimart.domain.OrderDetail;
import minimart.domain.OrderWithOwner;
import minimart.domain.Product;
import minimart.domain.Redemption;
import org.springframework.jdbc.core.simple.JdbcClient;
import org.springframework.transaction.PlatformTransactionManager;
import org.springframework.transaction.TransactionDefinition;
import org.springframework.transaction.support.TransactionTemplate;

/**
 * Owns the orders table and the one correctness-critical operation minimart has: the redemption
 * transaction.
 */
public class OrderRepository {

  private final JdbcClient jdbc;
  private final TransactionTemplate redemptionTx;

  /**
   * A repository backed by {@code jdbc}, running redemptions on {@code transactions}.
   *
   * <p>The isolation level is fixed here rather than inherited from the pool's default, because the
   * redemption's reasoning below depends on READ COMMITTED specifically: it is what makes the loser
   * of a race re-read the winner's committed row after its FOR UPDATE wait.
   */
  public OrderRepository(JdbcClient jdbc, PlatformTransactionManager transactions) {
    this.jdbc = jdbc;
    this.redemptionTx = new TransactionTemplate(transactions);
    this.redemptionTx.setIsolationLevel(TransactionDefinition.ISOLATION_READ_COMMITTED);
    this.redemptionTx.setPropagationBehavior(TransactionDefinition.PROPAGATION_REQUIRED);
    this.redemptionTx.setName("redemption");
  }

  /**
   * Returns the orders of one user, newest first, joined to the product's current name.
   *
   * <p>The user filter is in SQL. There is no broader query that a handler then narrows down,
   * because a handler that forgets to narrow is exactly the bug this shape prevents.
   */
  public List<OrderDetail> listByUser(long userId) {
    return jdbc.sql(
            """
            SELECT o.id, o.user_id, o.product_id, o.cost_points, o.created_at, p.name
              FROM orders o
              JOIN products p ON p.id = o.product_id
             WHERE o.user_id = ?
             ORDER BY o.id DESC""")
        .param(userId)
        .query(
            (rs, rowNum) ->
                new OrderDetail(
                    rs.getInt("id"),
                    rs.getInt("user_id"),
                    rs.getInt("product_id"),
                    rs.getInt("cost_points"),
                    rs.getObject("created_at", OffsetDateTime.class).toInstant(),
                    rs.getString("name")))
        .list();
  }

  /**
   * Returns every order in the system, newest first, joined to the product's current name and to
   * the username of the user who placed it.
   *
   * <p>This is the one read in minimart that is not scoped to the caller, and it is a separate
   * statement from {@link #listByUser} rather than the same query with an optional WHERE. A filter
   * that a parameter can switch off is a filter that a bug can switch off; the authority to run
   * this query is decided in the security filter chain, before the handler that calls it is
   * reached.
   */
  public List<OrderWithOwner> listAll() {
    return jdbc.sql(
            """
            SELECT o.id, o.user_id, o.product_id, o.cost_points, o.created_at, p.name, u.username
              FROM orders o
              JOIN products p ON p.id = o.product_id
              JOIN users    u ON u.id = o.user_id
             ORDER BY o.id DESC""")
        .query(
            (rs, rowNum) ->
                new OrderWithOwner(
                    rs.getInt("id"),
                    rs.getInt("user_id"),
                    rs.getInt("product_id"),
                    rs.getInt("cost_points"),
                    rs.getObject("created_at", OffsetDateTime.class).toInstant(),
                    rs.getString("name"),
                    rs.getString("username")))
        .list();
  }

  /**
   * Performs one redemption: deduct points, decrement stock, insert the order — all three in a
   * single transaction, or none of them.
   *
   * <p>This method is the transaction: spec/spec.md points here rather than restating it, and every
   * line below is load-bearing:
   *
   * <pre>
   *   BEGIN                                           (READ COMMITTED)
   *   SELECT points      ... WHERE id = ? FOR UPDATE    lock the wallet first
   *   SELECT cost, stock ... WHERE id = ? AND active FOR UPDATE
   *      -- decide, under both locks
   *   UPDATE users    SET points = points - cost WHERE id = ?
   *   UPDATE products SET stock  = stock  - 1    WHERE id = ? AND stock &gt; 0
   *   INSERT INTO orders ... RETURNING id, created_at
   *   COMMIT
   * </pre>
   *
   * <p>Lock order is an invariant, not an accident: the wallet is locked before the product,
   * always, so two redemptions cannot take the two locks in opposite orders and deadlock. Anything
   * added to this transaction takes its locks in the same order or it is wrong, however local the
   * change looks.
   *
   * <p>Concurrency: two requests for the last unit are serialised by the FOR UPDATE on the product
   * row. Under READ COMMITTED the loser's SELECT ... FOR UPDATE re-reads the row the winner
   * committed, so it sees stock = 0 and answers OUT_OF_STOCK; there is no window in which both read
   * the same stock and both write. The decision is never taken on a value read outside the lock.
   *
   * <p>WHERE stock &gt; 0 on the UPDATE is the second line of defence, and the reason the unlimited
   * sentinel survives: for stock = -1 the statement matches no row on purpose. The affected-row
   * count is therefore checked against what the domain said the next stock must be, and a
   * disagreement aborts the transaction rather than committing an order against nothing.
   *
   * <p>Refusals are the domain's {@code DomainException} (PRODUCT_NOT_FOUND, INSUFFICIENT_POINTS,
   * OUT_OF_STOCK); like every other exception thrown inside the callback, each of them rolls back,
   * leaving users.points, products.stock and orders all untouched.
   *
   * @throws RowNotFoundException when {@code userId} names no users row
   */
  public RedemptionResult redeem(long userId, long productId) {
    RedemptionResult result =
        redemptionTx.execute(status -> redeemInTransaction(userId, productId));
    if (result == null) {
      // TransactionTemplate returns whatever the callback returned, and the callback never returns
      // null; this is here so a future edit that breaks that fails loudly rather than as an NPE
      // somewhere above.
      throw new IllegalStateException("redemption transaction produced no result");
    }
    return result;
  }

  private RedemptionResult redeemInTransaction(long userId, long productId) {
    // 1. Lock the wallet. Wallet before product, always, so two redemptions can never take the two
    //    locks in opposite orders and deadlock.
    int balance =
        jdbc.sql("SELECT points FROM users WHERE id = ? FOR UPDATE")
            .param(userId)
            .query(Integer.class)
            .optional()
            .orElseThrow(() -> new RowNotFoundException("users.id = " + userId));

    // 2. Lock the product. No row means "does not exist, or is inactive", which are the same
    //    answer to a caller who is not an admin.
    Product product =
        jdbc.sql("SELECT cost_points, stock FROM products WHERE id = ? AND active FOR UPDATE")
            .param(productId)
            .query(
                (rs, rowNum) ->
                    new Product(
                        Math.toIntExact(productId),
                        null,
                        rs.getInt("cost_points"),
                        rs.getInt("stock"),
                        true))
            .optional()
            .orElse(null);

    // 3+4. Decide, under both locks, in the fixed precedence. A refusal throws, and the throw is
    //      the rollback.
    int nextStock = Redemption.evaluate(product, balance);

    jdbc.sql("UPDATE users SET points = points - ? WHERE id = ?")
        .param(product.costPoints())
        .param(userId)
        .update();

    int affected =
        jdbc.sql("UPDATE products SET stock = stock - 1 WHERE id = ? AND stock > 0")
            .param(productId)
            .update();
    // Unlimited stock must match no row; a finite stock must match exactly one.
    int wantAffected = nextStock != product.stock() ? 1 : 0;
    if (affected != wantAffected) {
      throw new IllegalStateException(
          "stock update affected %d rows, expected %d (stock was %d)"
              .formatted(affected, wantAffected, product.stock()));
    }

    Order order =
        jdbc.sql(
                """
                INSERT INTO orders (user_id, product_id, cost_points)
                     VALUES (?, ?, ?) RETURNING id, created_at""")
            .param(userId)
            .param(productId)
            .param(product.costPoints())
            .query(
                (rs, rowNum) ->
                    new Order(
                        rs.getInt("id"),
                        Math.toIntExact(userId),
                        Math.toIntExact(productId),
                        product.costPoints(),
                        rs.getObject("created_at", OffsetDateTime.class).toInstant()))
            .single();

    return new RedemptionResult(order, balance - product.costPoints());
  }
}
