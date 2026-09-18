@e2e @products
Feature: Product catalogue in the browser

  Background:
    Given run migration
    And in PostgreSQL:
      """sql
      INSERT INTO products (id, name, cost_points, stock, active) VALUES
        (1, 'Sticker Pack',      50, 3,  TRUE),
        (3, 'Digital Wallpaper', 20, -1, TRUE),
        (4, 'Retired Mug',       10, 5,  FALSE);
      """
    And locale is "en_US"

  Scenario: Seeded products are rendered, one row each
    When visit "/products"
    Then "heading:@shop.title" is visible
    And "link:@nav.products" is visible
    And list "testid:product-row" has 2 rows
    And row 1 of "testid:product-row" contains:
      """json
      {
        "testid:product-name":  "Sticker Pack",
        "testid:product-cost":  "50",
        "testid:product-stock": "3"
      }
      """
    And region "testid:product-row-3" contains:
      """json
      {
        "testid:product-name":  "Digital Wallpaper",
        "testid:product-cost":  "20",
        "testid:product-stock": "Unlimited"
      }
      """
    And console has no errors

  Scenario: An inactive product is not rendered at all
    When visit "/products"
    Then list "testid:product-row" has 2 rows
    And "testid:product-row-4" is hidden

  Scenario: The same page renders in Traditional Chinese
    Given locale is "zh_TW"
    When visit "/products"
    Then "heading:@shop.title" is visible
    And list "testid:product-row" has 2 rows
    And region "testid:product-row-3" contains:
      """json
      {
        "testid:product-name":  "Digital Wallpaper",
        "testid:product-stock": "無限"
      }
      """
    And console has no errors
