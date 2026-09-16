Feature: http empty object

  Scenario: s
    When GET /api/v1/items:
      """json
      {}
      """
