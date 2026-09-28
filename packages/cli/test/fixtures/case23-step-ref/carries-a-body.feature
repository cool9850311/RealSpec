Feature: step_ref

  Scenario: a step that needs a docstring of its own cannot be named
    When these things happen at one instant:
      """json
      [
        {"method": "GET", "path": "/api/v1/items"},
        {"step": "the customer pays:"}
      ]
      """
