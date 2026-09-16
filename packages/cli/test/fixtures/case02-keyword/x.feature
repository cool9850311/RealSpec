Feature: keyword

  Scenario: s
    When run migration
    Then in PostgreSQL:
      """sql
      INSERT INTO t VALUES (1);
      """
