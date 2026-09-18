@products
Feature: Product catalogue

  The catalogue is public: it is the one endpoint a visitor can read without a
  cookie. It shows active products only, and `stock = -1` means unlimited.

  Background:
    Given run migration
    And in PostgreSQL:
      """sql
      INSERT INTO products (id, name, cost_points, stock, active) VALUES
        (1, 'Sticker Pack',      50, 3,  TRUE),
        (3, 'Digital Wallpaper', 20, -1, TRUE),
        (4, 'Retired Mug',       10, 5,  FALSE);
      """

  Scenario: Anyone can list the active catalogue without logging in
    When GET /api/v1/products:
      """json
      {
        "body": {}
      }
      """
    Then response status is 200
    And response body contains:
      """json
      {
        "items": [
          { "id": 1, "name": "Sticker Pack",      "cost_points": 50, "stock": 3 },
          { "id": 3, "name": "Digital Wallpaper", "cost_points": 20, "stock": -1 }
        ],
        "total": 2
      }
      """

  Scenario: An inactive product is absent from the catalogue
    When GET /api/v1/products:
      """json
      {
        "body": {}
      }
      """
    Then response status is 200
    And response body contains:
      """json
      {
        "total": 2
      }
      """
    And response body does not contain "Retired Mug"

  Scenario: An empty catalogue is an empty list, not an error
    Given in PostgreSQL:
      """sql
      DELETE FROM products;
      """
    When GET /api/v1/products:
      """json
      {
        "body": {}
      }
      """
    Then response status is 200
    And response body contains:
      """json
      {
        "items": [],
        "total": 0
      }
      """
