@e2e @orders
Feature: Redeeming points in the browser

  The scenario that matters here is the first one: a real click in a real
  browser, followed by a SQL assertion. Every other kind of check can be
  satisfied by a page that merely looks right.

  Background:
    Given run migration
    And in PostgreSQL:
      """sql
      INSERT INTO users (id, username, password_hash, role, points) VALUES
        (1, 'alice', '$2a$10$MwKxT13/lPxFMjvrkm0dL.QJuNll1zljDU.eOoRVvgrWF.kf/hyC2', 'guest', 100),
        (2, 'bob',   '$2a$10$MwKxT13/lPxFMjvrkm0dL.QJuNll1zljDU.eOoRVvgrWF.kf/hyC2', 'guest', 10);
      """
    And in PostgreSQL:
      """sql
      INSERT INTO products (id, name, cost_points, stock, active) VALUES
        (1, 'Sticker Pack', 50, 3, TRUE);
      """
    And locale is "en_US"

  @stack:auth
  Scenario: A real click redeems the product and the order lands in the database
    When visit "/login"
    And fill "textbox:@auth.username" with "alice"
    And fill "textbox:@auth.password" with "secret123"
    And click "button:@auth.signIn"
    And visit "/products"
    And click "testid:redeem-1"
    Then network request "POST /api/v1/orders" responded 201
    And text of "testid:points-balance" is "50"
    And region "testid:product-row-1" contains:
      """json
      {
        "testid:product-stock": "2"
      }
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM orders o
        JOIN users u ON u.id = o.user_id
       WHERE u.username = 'alice'
         AND o.product_id = 1
         AND o.cost_points = 50;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM users WHERE username = 'alice' AND points = 50;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM products WHERE id = 1 AND stock = 2;
      """
    And console has no errors

  Scenario: The redeemed order appears on the orders page under its own id
    When visit "/login"
    And fill "textbox:@auth.username" with "alice"
    And fill "textbox:@auth.password" with "secret123"
    And click "button:@auth.signIn"
    And visit "/products"
    And click "testid:redeem-1"
    Then network request "POST /api/v1/orders" responded 201
    When click "link:@nav.orders"
    Then page URL is "/orders"
    And "heading:@orders.title" is visible
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

  Scenario: A redemption the user cannot afford leaves the database untouched
    When visit "/login"
    And fill "textbox:@auth.username" with "bob"
    And fill "textbox:@auth.password" with "secret123"
    And click "button:@auth.signIn"
    And visit "/products"
    And click "testid:redeem-1"
    Then network request "POST /api/v1/orders" responded 422
    And text of "testid:redeem-error" is "You do not have enough points."
    And text of "testid:points-balance" is "10"
    And region "testid:product-row-1" contains:
      """json
      {
        "testid:product-stock": "3"
      }
      """
    And in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM orders;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM users WHERE username = 'bob' AND points = 10;
      """

  Scenario: A product at zero stock is refused in the browser and writes nothing
    Given in PostgreSQL:
      """sql
      INSERT INTO products (id, name, cost_points, stock, active) VALUES
        (2, 'Enamel Pin', 80, 0, TRUE);
      """
    When visit "/login"
    And fill "textbox:@auth.username" with "alice"
    And fill "textbox:@auth.password" with "secret123"
    And click "button:@auth.signIn"
    And visit "/products"
    And click "testid:redeem-2"
    Then network request "POST /api/v1/orders" responded 422
    And text of "testid:redeem-error" is "This product is out of stock."
    And text of "testid:points-balance" is "100"
    And region "testid:product-row-2" contains:
      """json
      {
        "testid:product-stock": "0"
      }
      """
    And in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM orders;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM users WHERE username = 'alice' AND points = 100;
      """

  Scenario: A user with no orders sees the empty state, and Back returns to the catalogue
    When visit "/login"
    And fill "textbox:@auth.username" with "bob"
    And fill "textbox:@auth.password" with "secret123"
    And click "button:@auth.signIn"
    And visit "/products"
    And click "link:@nav.orders"
    Then page URL is "/orders"
    And list "testid:order-row" has 0 rows
    And text of "testid:orders-empty" is "You have not redeemed anything yet."
    When go back
    Then page URL is "/products"
    And list "testid:product-row" has 1 row
    And console has no errors
