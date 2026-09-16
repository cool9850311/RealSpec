Feature: triple quote

  Scenario: body contains a bare triple quote
    Then response body contains:
      """json
      {"a": """}
      """

  Scenario: a marker line inside the body ends the docstring early
    Then response body contains:
      """json
      {"a": 1,
      """json
      "b": 2}
      """
