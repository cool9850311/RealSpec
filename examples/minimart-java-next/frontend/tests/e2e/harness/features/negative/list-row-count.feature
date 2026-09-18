@harness @negative
Feature: A row count that does not match the page

  PLAN.md §6.B case 7. Three products are active; the assertion claims two.
  The count is exact and there is no "at least" form, so this MUST fail.

  Background:
    Given run migration
    And in PostgreSQL:
      """sql
      INSERT INTO products (id, name, cost_points, stock, active) VALUES
        (1, 'Sticker Pack',      50, 3,  TRUE),
        (2, 'Enamel Pin',        20, 1,  TRUE),
        (3, 'Digital Wallpaper', 20, -1, TRUE);
      """
    And locale is "en_US"

  Scenario: Two rows are claimed where the page renders three
    When visit "/products"
    Then list "testid:product-row" has 2 rows
