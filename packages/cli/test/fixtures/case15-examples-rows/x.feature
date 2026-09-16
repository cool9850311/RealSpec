Feature: examples rows

  Scenario Outline: s
    Given run migration
    When GET /api/v1/items/<id>:
      """json
      {}
      """
    Then response status is 200

    Examples:
      | id | note                     |
      | 1  | Given something not real |
      | 2  | And also not a step      |
