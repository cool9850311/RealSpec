Feature: provided

  Scenario: a provided name needs no save step
    When GET /api/v1/me:
      """json
      {"headers": {"Cookie": "token={tokenExpired}"}}
      """
    Then response status is 401
