@admin
Feature: Every user's orders

  `GET /api/v1/admin/orders` is the one read in minimart that is not scoped to
  the caller, and therefore the only place a role decides anything. It is a file
  of its own rather than three more scenarios in orders.feature because its
  subject is an authorisation rule, not the redemption transaction, and it needs
  a Background that seeds two users' orders and an admin to look at them. One
  Background, one subject.

  The role that decides is the one on the `users` row, re-read per request — not
  the `role` claim in the token. auth.feature proves that half with a forged
  claim; this file proves the endpoint itself.

  Each request carries its own credential, written out in full: the scenario
  logs in with a real `POST /api/v1/auth/login`, names the cookie that answer
  set, and the request says whose it is. The third scenario carries none, which
  is how it reaches 401 instead of 403.

  Background:
    Given run migration
    And in PostgreSQL:
      """sql
      INSERT INTO users (id, username, password_hash, role, points) VALUES
        (1, 'alice', '$2a$10$MwKxT13/lPxFMjvrkm0dL.QJuNll1zljDU.eOoRVvgrWF.kf/hyC2', 'guest', 100),
        (2, 'bob',   '$2a$10$MwKxT13/lPxFMjvrkm0dL.QJuNll1zljDU.eOoRVvgrWF.kf/hyC2', 'guest', 50),
        (3, 'root',  '$2a$10$MwKxT13/lPxFMjvrkm0dL.QJuNll1zljDU.eOoRVvgrWF.kf/hyC2', 'admin', 0);
      """
    And in PostgreSQL:
      """sql
      INSERT INTO products (id, name, cost_points, stock, active) VALUES
        (1, 'Sticker Pack', 50, 3, TRUE),
        (2, 'Enamel Pin',   80, 2, TRUE);
      """
    And in PostgreSQL:
      """sql
      INSERT INTO orders (id, user_id, product_id, cost_points, created_at) VALUES
        (1, 1, 1, 50, '2026-01-01T00:00:00Z'),
        (2, 2, 2, 80, '2026-01-02T00:00:00Z'),
        (3, 1, 2, 80, '2026-01-03T00:00:00Z');
      """

  Scenario: An admin sees every user's orders, newest first, each with its owner
    When POST /api/v1/auth/login:
      """json
      {
        "body": {
          "username": "root",
          "password": "secret123"
        }
      }
      """
    Then response status is 200
    And save response cookie "token" as "rootToken"
    When GET /api/v1/admin/orders:
      """json
      {
        "headers": {
          "Cookie": "token={rootToken}"
        },
        "body": {}
      }
      """
    Then response status is 200
    And response body contains:
      """json
      {
        "items": [
          {
            "id": 3,
            "username": "alice",
            "product_name": "Enamel Pin",
            "cost_points": 80,
            "created_at": "<non-null>"
          },
          {
            "id": 2,
            "username": "bob",
            "product_name": "Enamel Pin",
            "cost_points": 80,
            "created_at": "<non-null>"
          },
          {
            "id": 1,
            "username": "alice",
            "product_name": "Sticker Pack",
            "cost_points": 50,
            "created_at": "<non-null>"
          }
        ],
        "total": 3
      }
      """
    When GET /api/v1/orders:
      """json
      {
        "headers": {
          "Cookie": "token={rootToken}"
        },
        "body": {}
      }
      """
    Then response status is 200
    And response body contains:
      """json
      {
        "items": [],
        "total": 0
      }
      """

  Scenario: A guest is refused with 403 and is shown nothing
    When POST /api/v1/auth/login:
      """json
      {
        "body": {
          "username": "alice",
          "password": "secret123"
        }
      }
      """
    Then response status is 200
    And save response cookie "token" as "aliceToken"
    When GET /api/v1/admin/orders:
      """json
      {
        "headers": {
          "Cookie": "token={aliceToken}"
        },
        "body": {}
      }
      """
    Then response status is 403
    And response body contains:
      """json
      {
        "error": "FORBIDDEN"
      }
      """
    And response body does not contain "items"
    And response body does not contain "Sticker Pack"
    And response body does not contain "bob"
    When GET /api/v1/orders:
      """json
      {
        "headers": {
          "Cookie": "token={aliceToken}"
        },
        "body": {}
      }
      """
    Then response status is 200
    And response body contains:
      """json
      {
        "total": 2
      }
      """

  Scenario: Without a session the admin listing is 401, not 403
    When GET /api/v1/admin/orders:
      """json
      {
        "body": {}
      }
      """
    Then response status is 401
    And response body contains:
      """json
      {
        "error": "UNAUTHENTICATED"
      }
      """
    And response body does not contain "Sticker Pack"
