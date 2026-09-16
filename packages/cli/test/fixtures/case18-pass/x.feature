Feature: all good

  Background:
    Given run migration

  Scenario: s
    When GET /api/v1/items:
      """json
      {"headers": {"X-A": "1"}}
      """
    Then response status is 200
    And response body contains:
      """json
      {"total": 1}
      """
