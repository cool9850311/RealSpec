Feature: provided typo

  Scenario: a misspelled provided name is not in the closed set
    When GET /api/v1/me:
      """json
      {"headers": {"Cookie": "token={tokenExpried}"}}
      """
    Then response status is 401
