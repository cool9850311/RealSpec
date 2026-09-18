package minimart.domain;

import java.time.Instant;

/**
 * An order joined to its product's current name, which is what GET /api/v1/orders renders. The name
 * is read live; the price is not.
 */
public record OrderDetail(
    int id, int userId, int productId, int costPoints, Instant createdAt, String productName) {}
