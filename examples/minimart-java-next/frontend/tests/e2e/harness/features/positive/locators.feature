@harness
Feature: The three locator forms

  A harness fixture: PLAN.md §6.B cases 2 and 3. One product is seeded on
  purpose — a second would make "button:Redeem" match twice, and a singular step
  is strict.

  The two locale scenarios are written out rather than folded into a Scenario
  Outline: `realspec validate` normalises every `<param>` to one placeholder
  before matching (that is `validate.py`'s behaviour, which the CLI is a
  byte-compatible port of), so a step whose capture is as narrow as
  `[a-z]{2}_[A-Z]{2}` cannot be parameterised by an Examples column. Two
  scenarios are four lines longer and they validate.

  Background:
    Given run migration
    And in PostgreSQL:
      """sql
      INSERT INTO users (id, username, password_hash, role, points) VALUES
        (1, 'alice', '$2a$10$MwKxT13/lPxFMjvrkm0dL.QJuNll1zljDU.eOoRVvgrWF.kf/hyC2', 'guest', 100);
      INSERT INTO products (id, name, cost_points, stock, active) VALUES
        (1, 'Sticker Pack', 50, 3, TRUE);
      """
    And locale is "en_US"

  @stack:auth
  Scenario: A button is found by ARIA role and a literal accessible name
    When visit "/login"
    And fill "textbox:@auth.username" with "alice"
    And fill "textbox:@auth.password" with "secret123"
    And click "button:@auth.signIn"
    And visit "/products"
    And click "button:Redeem"
    Then network request "POST /api/v1/orders" responded 201
    And text of "testid:points-balance" is "50"
    And console has no errors

  @stack:auth
  Scenario: An @key locator resolves against the English catalogue
    When visit "/login"
    And fill "textbox:@auth.username" with "alice"
    And fill "textbox:@auth.password" with "secret123"
    And click "button:@auth.signIn"
    And visit "/products"
    And click "button:@shop.redeem"
    Then network request "POST /api/v1/orders" responded 201
    And text of "testid:points-balance" is "50"
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM orders WHERE product_id = 1 AND cost_points = 50;
      """
    And console has no errors

  @stack:auth
  Scenario: The same @key locator resolves against the Chinese catalogue
    Given locale is "zh_TW"
    When visit "/login"
    And fill "textbox:@auth.username" with "alice"
    And fill "textbox:@auth.password" with "secret123"
    And click "button:@auth.signIn"
    And visit "/products"
    And click "button:@shop.redeem"
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
      SELECT 1 FROM orders WHERE product_id = 1 AND cost_points = 50;
      """
    And console has no errors
