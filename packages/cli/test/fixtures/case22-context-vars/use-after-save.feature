Feature: ordering

  Scenario: the reference comes after the save
    When POST /api/v1/orders:
      """json
      {}
      """
    Then response status is 201
    And save response body field "id" as "orderId"
    When GET /api/v1/orders/{orderId}:
      """json
      {}
      """
    Then response status is 200
