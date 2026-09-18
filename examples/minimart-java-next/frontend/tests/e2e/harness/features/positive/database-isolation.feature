@harness
Feature: Two scenarios own the same primary keys

  A harness fixture: PLAN.md §6.B case 16.

  Both scenarios below seed `users.id = 1` and `products.id = 1` with DIFFERENT
  values, and each asserts that the page shows its own. They can only both pass
  if each scenario has its own database. One shared database would make the
  second INSERT a duplicate-key error; a shared database truncated between
  scenarios would make the two race and pass or fail by scheduling, which is
  worse than failing.

  Since the run has one Docker network rather than one per scenario, this pair is
  what still proves the databases are separate.

  Background:
    Given run migration

  Scenario: The first owner of id 1
    Given in PostgreSQL:
      """sql
      INSERT INTO users (id, username, password_hash, role, points) VALUES
        (1, 'alice', '$2a$10$MwKxT13/lPxFMjvrkm0dL.QJuNll1zljDU.eOoRVvgrWF.kf/hyC2', 'guest', 100);
      INSERT INTO products (id, name, cost_points, stock, active) VALUES
        (1, 'Sticker Pack', 50, 3, TRUE);
      """
    And locale is "en_US"
    When visit "/products"
    Then list "testid:product-row" has 1 row
    And region "testid:product-row-1" contains:
      """json
      {
        "testid:product-name": "Sticker Pack",
        "testid:product-cost": "50"
      }
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM users WHERE id = 1 AND username = 'alice';
      """
    And in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM users WHERE username = 'bob';
      """
    And console has no errors

  Scenario: The second owner of id 1
    Given in PostgreSQL:
      """sql
      INSERT INTO users (id, username, password_hash, role, points) VALUES
        (1, 'bob', '$2a$10$MwKxT13/lPxFMjvrkm0dL.QJuNll1zljDU.eOoRVvgrWF.kf/hyC2', 'admin', 7);
      INSERT INTO products (id, name, cost_points, stock, active) VALUES
        (1, 'Enamel Pin', 20, 1, TRUE);
      """
    And locale is "en_US"
    When visit "/products"
    Then list "testid:product-row" has 1 row
    And region "testid:product-row-1" contains:
      """json
      {
        "testid:product-name": "Enamel Pin",
        "testid:product-cost": "20"
      }
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM users WHERE id = 1 AND username = 'bob';
      """
    And in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM users WHERE username = 'alice';
      """
    And console has no errors
