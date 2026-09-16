Feature: forbidden docstring

  Scenario: s
    Then response status is 200
      """json
      {"a": 1}
      """
