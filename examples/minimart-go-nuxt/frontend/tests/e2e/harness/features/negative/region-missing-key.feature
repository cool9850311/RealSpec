@harness @negative
Feature: A region assertion that names a key the region does not contain

  PLAN.md §6.B case 6. This feature MUST fail, and its failure message must say
  which key was missing and that it was missing rather than empty.
  tests/e2e/harness/harness.spec.ts runs it and asserts exactly that.

  Background:
    Given run migration
    And in PostgreSQL:
      """sql
      INSERT INTO products (id, name, cost_points, stock, active) VALUES
        (1, 'Sticker Pack', 50, 3, TRUE);
      """
    And locale is "en_US"

  Scenario: A key the region does not contain is reported as missing
    When visit "/products"
    Then region "testid:product-row-1" contains:
      """json
      {
        "testid:product-name":     "Sticker Pack",
        "testid:product-discount": "0"
      }
      """
