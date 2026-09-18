@harness @negative
Feature: <non-null> against an element that renders nothing

  PLAN.md §6.B case 8. The product's name is seeded as the empty string, so the
  cell exists and renders no text. "<non-null>" means "exists AND renders
  something", so this MUST fail — and the message must distinguish "present but
  empty" from "missing", because they are different bugs.

  Background:
    Given run migration
    And in PostgreSQL:
      """sql
      INSERT INTO products (id, name, cost_points, stock, active) VALUES
        (1, '', 50, 3, TRUE);
      """
    And locale is "en_US"

  Scenario: An element that renders the empty string is not non-null
    When visit "/products"
    Then list "testid:product-row" has 1 row
    And region "testid:product-row-1" contains:
      """json
      {
        "testid:product-cost": "50",
        "testid:product-name": "<non-null>"
      }
      """
