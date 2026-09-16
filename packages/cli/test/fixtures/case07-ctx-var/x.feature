Feature: ctx var

  Scenario: s
    Then response body contains:
      """json
      {"session_id": "{sessionId}"}
      """
    And in PostgreSQL query returns 1 rows:
      """sql
      SELECT 1 FROM sessions WHERE id = '{sessionId}';
      """
