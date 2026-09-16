Feature: sql select prefix

  Scenario: s
    Then in PostgreSQL query returns 1 rows:
      """sql
      SELECT count(*) FROM users;
      """
    And in PostgreSQL query returns 1 rows:
      """sql
      -- count the users
      SELECT count(*) FROM users;
      """
