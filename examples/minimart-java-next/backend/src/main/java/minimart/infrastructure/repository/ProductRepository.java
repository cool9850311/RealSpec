package minimart.infrastructure.repository;

import java.util.List;
import minimart.domain.Product;
import org.springframework.jdbc.core.simple.JdbcClient;

/** Reads the products table. */
public class ProductRepository {

  private final JdbcClient jdbc;

  /** A repository backed by {@code jdbc}. */
  public ProductRepository(JdbcClient jdbc) {
    this.jdbc = jdbc;
  }

  /**
   * Returns every active product ordered by id ascending.
   *
   * <p>The active filter is applied in SQL. An inactive product is absent from the catalogue and
   * unredeemable, so no client can ever hold the id of one.
   */
  public List<Product> listActive() {
    return jdbc.sql("SELECT id, name, cost_points, stock FROM products WHERE active ORDER BY id")
        .query(
            (rs, rowNum) ->
                new Product(
                    rs.getInt("id"),
                    rs.getString("name"),
                    rs.getInt("cost_points"),
                    rs.getInt("stock"),
                    true))
        .list();
  }
}
