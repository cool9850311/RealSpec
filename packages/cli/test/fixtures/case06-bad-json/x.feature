Feature: bad json

  Scenario: s
    Then response body contains:
      """json
      {
        "a": 1
        "b": 2
      }
      """
