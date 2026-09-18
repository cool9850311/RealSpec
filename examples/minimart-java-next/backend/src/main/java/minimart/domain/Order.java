package minimart.domain;

import java.time.Instant;

/**
 * A row of the orders table. {@code costPoints} is the price paid, copied from the product at
 * redemption time: changing products.cost_points later must not rewrite history.
 */
public record Order(int id, int userId, int productId, int costPoints, Instant createdAt) {}
