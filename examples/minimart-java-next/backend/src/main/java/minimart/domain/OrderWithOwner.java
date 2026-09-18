package minimart.domain;

import java.time.Instant;

/**
 * An {@link OrderDetail} with the username of the user who placed it, which is what GET
 * /api/v1/admin/orders renders. Like the product name it is read live: a username can change, the
 * price paid cannot.
 */
public record OrderWithOwner(
    int id,
    int userId,
    int productId,
    int costPoints,
    Instant createdAt,
    String productName,
    String username) {}
