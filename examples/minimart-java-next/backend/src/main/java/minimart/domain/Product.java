package minimart.domain;

/**
 * A row of the products table.
 *
 * @param stock units left, or {@link Stock#UNLIMITED} for a product that never runs out
 * @param active false removes the product from the catalogue and makes it unredeemable
 */
public record Product(int id, String name, int costPoints, int stock, boolean active) {}
