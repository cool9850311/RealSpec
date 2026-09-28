Feature: step_ref

  Scenario: a body inside a raced envelope is still a body
    When these things happen at one instant:
      """json
      [
        {"method": "POST", "path": "/api/v1/items", "body": { "step": "grind" }},
        {"step": "the worker runs"}
      ]
      """
