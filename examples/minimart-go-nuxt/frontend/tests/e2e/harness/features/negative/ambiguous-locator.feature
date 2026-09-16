@harness @negative
Feature: A singular step whose locator matches two elements

  PLAN.md §6.B case 9. Two rows are rendered and a singular step asks for "the"
  row's text. Acting on the first match is how a test starts asserting about
  whichever element happened to come first, so this MUST fail, and the message
  must say how many elements matched and list them.

  Background:
    Given run migration
    And in PostgreSQL:
      """sql
      INSERT INTO products (id, name, cost_points, stock, active) VALUES
        (1, 'Sticker Pack', 50, 3, TRUE),
        (2, 'Enamel Pin',   20, 1, TRUE);
      """
    And locale is "en_US"

  Scenario: An ambiguous locator is never resolved to its first match
    When visit "/products"
    Then list "testid:product-row" has 2 rows
    And text of "testid:product-row" is "Sticker Pack 50 3 Redeem"
