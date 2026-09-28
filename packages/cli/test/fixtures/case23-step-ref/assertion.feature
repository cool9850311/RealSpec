Feature: step_ref

  Scenario: an assertion is not an actor
    When these things happen at one instant:
      """json
      [
        {"method": "GET", "path": "/api/v1/items"},
        {"step": "response status is 200"}
      ]
      """
