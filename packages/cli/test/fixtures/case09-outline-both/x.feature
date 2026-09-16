Feature: outline both

  Scenario Outline: s
    When GET /api/v1/plans/<plan>:
      """json
      {"body": {"plan": "<plan>"}}
      """
    Then response status is 200

    Examples:
      | plan  |
      | free  |
      | pro   |
