Feature: step_ref

  Scenario: a named step that is an action of this registry
    When these things happen at one instant:
      """json
      [
        {"method": "GET", "path": "/api/v1/items"},
        {"step": "the worker runs"}
      ]
      """
