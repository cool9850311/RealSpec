Feature: background

  Background:
    Given run migration
    Given POST /api/v1/orders:
      """json
      {}
      """
    Given save response body field "id" as "orderId"

  Scenario: a scenario sees what the Background saved
    When GET /api/v1/orders/{orderId}:
      """json
      {}
      """
    Then response status is 200

  Scenario: and so does the next one
    When DELETE /api/v1/orders/{orderId}:
      """json
      {}
      """
    Then response status is 204
