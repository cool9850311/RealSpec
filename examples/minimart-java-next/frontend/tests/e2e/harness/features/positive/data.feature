@harness
Feature: Seeded rows, a real click, and the database afterwards

  A harness fixture: PLAN.md §6.B cases 4, 5, 10, 11 and 15 in one scenario,
  because they are one story — what the database held reached the screen, a real
  button was pressed, and the row the press produced is addressed on the next
  page by an id the page itself supplied.

  Background:
    Given run migration
    And in PostgreSQL:
      """sql
      INSERT INTO users (id, username, password_hash, role, points) VALUES
        (1, 'alice', '$2a$10$MwKxT13/lPxFMjvrkm0dL.QJuNll1zljDU.eOoRVvgrWF.kf/hyC2', 'guest', 100);
      INSERT INTO products (id, name, cost_points, stock, active) VALUES
        (1, 'Sticker Pack', 50, 3,  TRUE),
        (2, 'Enamel Pin',   20, -1, TRUE);
      """
    And locale is "en_US"

  @stack:auth
  Scenario: What PostgreSQL holds is what the page shows, and a click writes back
    When visit "/login"
    And fill "textbox:@auth.username" with "alice"
    And fill "textbox:@auth.password" with "secret123"
    And click "button:@auth.signIn"
    And visit "/products"
    Then list "testid:product-row" has 2 rows
    And row 1 of "testid:product-row" contains:
      """json
      {
        "testid:product-name": "Sticker Pack",
        "testid:product-cost": "50"
      }
      """
    And region "testid:product-row-2" contains:
      """json
      {
        "testid:product-name":  "Enamel Pin",
        "testid:product-stock": "Unlimited"
      }
      """
    When click "testid:redeem-1"
    Then network request "POST /api/v1/orders" responded 201
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM orders WHERE product_id = 1 AND cost_points = 50;
      """
    When click "link:@nav.orders"
    Then page URL is "/orders"
    And list "testid:order-row" has 1 row
    And save text of "testid:order-id" as "orderId"
    And region "testid:order-row-{orderId}" contains:
      """json
      {
        "testid:order-product":    "Sticker Pack",
        "testid:order-cost":       "50",
        "testid:order-created-at": "<non-null>"
      }
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM orders WHERE id = '{orderId}' AND user_id = 1;
      """
    And console has no errors
