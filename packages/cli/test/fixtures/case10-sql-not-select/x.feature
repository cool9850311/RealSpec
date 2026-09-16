Feature: sql not select

  Scenario: s
    Then in PostgreSQL query returns 1 rows:
      """sql
      DELETE FROM users WHERE id = 1;
      """
