@harness @negative @stack:auth
Feature: A real click that did not write to the database

  PLAN.md §6.B case 12. bob holds 10 points and the product costs 50, so the
  click is real, the button is really pressed, the request is really sent — and
  the API refuses it. The page looks plausible afterwards; only the SQL
  assertion notices that nothing landed, which is the entire reason that step
  exists. This MUST fail.

  Background:
    Given run migration
    And in PostgreSQL:
      """sql
      INSERT INTO users (id, username, password_hash, role, points) VALUES
        (1, 'bob', '$2a$10$MwKxT13/lPxFMjvrkm0dL.QJuNll1zljDU.eOoRVvgrWF.kf/hyC2', 'guest', 10);
      INSERT INTO products (id, name, cost_points, stock, active) VALUES
        (1, 'Sticker Pack', 50, 3, TRUE);
      """
    And locale is "en_US"

  Scenario: A refused redemption leaves no order row
    When visit "/login"
    And fill "textbox:@auth.username" with "bob"
    And fill "textbox:@auth.password" with "secret123"
    And click "button:@auth.signIn"
    And visit "/products"
    And click "testid:redeem-1"
    Then network request "POST /api/v1/orders" responded 422
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM orders WHERE product_id = 1;
      """
