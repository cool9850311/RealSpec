Feature: step_ref

  Scenario: a sentence no step matches
    When these things happen at one instant:
      """json
      [
        {"method": "GET", "path": "/api/v1/items"},
        {"step": "the worker sprints"}
      ]
      """
