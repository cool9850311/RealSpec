Feature: http extra key

  Scenario: s
    When GET /api/v1/items:
      """json
      {"headers": {"X-A": "1"}, "query": {"page": 1}}
      """
