Feature: marker

  Scenario: s
    Then in PostgreSQL query returns 1 rows:
      """json
      DELETE FROM users;
      """
