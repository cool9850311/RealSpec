Feature: outline path

  Scenario Outline: s
    When GET /api/v1/items/<id>:
      """json
      {}
      """
    Then response status is 200

    Examples:
      | id |
      | 1  |
