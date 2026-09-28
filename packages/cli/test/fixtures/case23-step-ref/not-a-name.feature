Feature: step_ref

  Scenario: a step that declares no step_ref is never scanned for names
    When POST /api/v1/items:
      """json
      {
        "body": { "step": "grind" }
      }
      """
