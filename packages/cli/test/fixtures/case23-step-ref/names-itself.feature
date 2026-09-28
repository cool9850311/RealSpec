Feature: step_ref

  Scenario: the combinator cannot name itself
    When these things happen at one instant:
      """json
      [
        {"method": "GET", "path": "/api/v1/items"},
        {"step": "these things happen at one instant:"}
      ]
      """
