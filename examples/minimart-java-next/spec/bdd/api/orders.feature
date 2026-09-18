@orders
Feature: Points redemption

  One redemption is three writes — deduct points, decrement stock, insert the
  order — in one transaction. Every scenario below asserts the database
  afterwards, including the ones that fail: a 422 that had already deducted
  points would pass a status-only test and cannot pass these.

  Every request that needs a session carries it: the scenario logs in with a
  real `POST /api/v1/auth/login`, names the cookie that answer set with
  `save response cookie "token" as "..."`, and each later request writes
  `"Cookie": "token={…}"` itself. That is what makes the multi-caller scenarios
  below readable — one login per caller, one token each, and every request says
  which of them it carries.

  The last three scenarios are about atomicity. Two of them race four callers
  through `is called concurrently:` — for the last unit of stock, and for a
  wallet that covers one redemption — and each runs three times, because one run
  catches a missing lock about 499 times in 500 and three runs make the gap
  vanish. The third is the case that is NOT a race: two redemptions sent one
  after the other are two orders, deliberately, and no amount of locking may
  collapse them into one.

  Background:
    Given run migration
    And in PostgreSQL:
      """sql
      INSERT INTO users (id, username, password_hash, role, points) VALUES
        (1, 'alice', '$2a$10$MwKxT13/lPxFMjvrkm0dL.QJuNll1zljDU.eOoRVvgrWF.kf/hyC2', 'guest', 100),
        (2, 'bob',   '$2a$10$MwKxT13/lPxFMjvrkm0dL.QJuNll1zljDU.eOoRVvgrWF.kf/hyC2', 'guest', 10);
      """
    And in PostgreSQL:
      """sql
      INSERT INTO products (id, name, cost_points, stock, active) VALUES
        (1, 'Sticker Pack',      50, 3,  TRUE),
        (2, 'Enamel Pin',        80, 0,  TRUE),
        (3, 'Digital Wallpaper', 20, -1, TRUE),
        (4, 'Retired Mug',       10, 5,  FALSE);
      """

  Scenario: A redemption deducts points, decrements stock and records the order
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
    When POST /api/v1/orders:
      """json
      {
        "headers": {
          "Cookie": "token={aliceToken}"
        },
        "body": {
          "product_id": 1
        }
      }
      """
    Then response status is 201
    And response body contains:
      """json
      {
        "id": "<non-null>",
        "cost_points": 50,
        "balance_after": 50
      }
      """
    And save response body field "id" as "orderId"
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM users WHERE username = 'alice' AND points = 50;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM products WHERE id = 1 AND stock = 2;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM orders
       WHERE id = '{orderId}'
         AND user_id = 1
         AND product_id = 1
         AND cost_points = 50
         AND created_at <= NOW();
      """

  Scenario: Insufficient points is refused and writes nothing
    When POST /api/v1/auth/login:
      """json
      {
        "body": {
          "username": "bob",
          "password": "secret123"
        }
      }
      """
    Then response status is 200
    And save response cookie "token" as "bobToken"
    When POST /api/v1/orders:
      """json
      {
        "headers": {
          "Cookie": "token={bobToken}"
        },
        "body": {
          "product_id": 1
        }
      }
      """
    Then response status is 422
    And response body contains:
      """json
      {
        "error": "INSUFFICIENT_POINTS"
      }
      """
    And in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM orders;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM users WHERE username = 'bob' AND points = 10;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM products WHERE id = 1 AND stock = 3;
      """

  Scenario: A product at zero stock is refused even when the caller can afford it
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
    When POST /api/v1/orders:
      """json
      {
        "headers": {
          "Cookie": "token={aliceToken}"
        },
        "body": {
          "product_id": 2
        }
      }
      """
    Then response status is 422
    And response body contains:
      """json
      {
        "error": "OUT_OF_STOCK"
      }
      """
    And in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM orders;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM users WHERE username = 'alice' AND points = 100;
      """

  Scenario: Unlimited stock is charged for but never decremented
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
    When POST /api/v1/orders:
      """json
      {
        "headers": {
          "Cookie": "token={aliceToken}"
        },
        "body": {
          "product_id": 3
        }
      }
      """
    Then response status is 201
    And response body contains:
      """json
      {
        "cost_points": 20,
        "balance_after": 80
      }
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM products WHERE id = 3 AND stock = -1;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM orders WHERE user_id = 1 AND product_id = 3 AND cost_points = 20;
      """

  Scenario: Unknown and inactive products are both 404, and neither writes an order
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
    When POST /api/v1/orders:
      """json
      {
        "headers": {
          "Cookie": "token={aliceToken}"
        },
        "body": {
          "product_id": 999
        }
      }
      """
    Then response status is 404
    And response body contains:
      """json
      {
        "error": "PRODUCT_NOT_FOUND"
      }
      """
    When POST /api/v1/orders:
      """json
      {
        "headers": {
          "Cookie": "token={aliceToken}"
        },
        "body": {
          "product_id": 4
        }
      }
      """
    Then response status is 404
    And response body contains:
      """json
      {
        "error": "PRODUCT_NOT_FOUND"
      }
      """
    And in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM orders;
      """

  Scenario: Redeeming without a session is 401 and writes nothing
    When POST /api/v1/orders:
      """json
      {
        "body": {
          "product_id": 1
        }
      }
      """
    Then response status is 401
    And response body contains:
      """json
      {
        "error": "UNAUTHENTICATED"
      }
      """
    And in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM orders;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM products WHERE id = 1 AND stock = 3;
      """

  Scenario: A caller sees only their own orders
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
    When POST /api/v1/auth/login:
      """json
      {
        "body": {
          "username": "bob",
          "password": "secret123"
        }
      }
      """
    Then response status is 200
    And save response cookie "token" as "bobToken"
    When POST /api/v1/orders:
      """json
      {
        "headers": {
          "Cookie": "token={aliceToken}"
        },
        "body": {
          "product_id": 1
        }
      }
      """
    Then response status is 201
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
        "items": [
          {
            "product_name": "Sticker Pack",
            "cost_points": 50,
            "created_at": "<non-null>"
          }
        ],
        "total": 1
      }
      """
    When GET /api/v1/orders:
      """json
      {
        "headers": {
          "Cookie": "token={bobToken}"
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
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM orders WHERE user_id = 1;
      """

  Scenario Outline: Four callers race for the last unit and exactly one wins
    Given in PostgreSQL:
      """sql
      INSERT INTO users (id, username, password_hash, role, points) VALUES
        (10, 'carol', '$2a$10$MwKxT13/lPxFMjvrkm0dL.QJuNll1zljDU.eOoRVvgrWF.kf/hyC2', 'guest', 100),
        (11, 'dave',  '$2a$10$MwKxT13/lPxFMjvrkm0dL.QJuNll1zljDU.eOoRVvgrWF.kf/hyC2', 'guest', 100),
        (12, 'erin',  '$2a$10$MwKxT13/lPxFMjvrkm0dL.QJuNll1zljDU.eOoRVvgrWF.kf/hyC2', 'guest', 100),
        (13, 'frank', '$2a$10$MwKxT13/lPxFMjvrkm0dL.QJuNll1zljDU.eOoRVvgrWF.kf/hyC2', 'guest', 100);
      """
    And in PostgreSQL:
      """sql
      INSERT INTO products (id, name, cost_points, stock, active) VALUES
        (10, 'Last Unit', 50, 1, TRUE);
      """
    When POST /api/v1/auth/login:
      """json
      {
        "body": {
          "username": "carol",
          "password": "secret123"
        }
      }
      """
    Then response status is 200
    And save response cookie "token" as "carolToken"
    When POST /api/v1/auth/login:
      """json
      {
        "body": {
          "username": "dave",
          "password": "secret123"
        }
      }
      """
    Then response status is 200
    And save response cookie "token" as "daveToken"
    When POST /api/v1/auth/login:
      """json
      {
        "body": {
          "username": "erin",
          "password": "secret123"
        }
      }
      """
    Then response status is 200
    And save response cookie "token" as "erinToken"
    When POST /api/v1/auth/login:
      """json
      {
        "body": {
          "username": "frank",
          "password": "secret123"
        }
      }
      """
    Then response status is 200
    And save response cookie "token" as "frankToken"
    # Four concurrent reads first. The service takes a database connection per
    # in-flight request and the pool starts empty, so on a cold pool three of the
    # four callers below are still opening one while the first commits — the race
    # would not be a race. This puts the four connections in the pool.
    When GET /api/v1/products is called concurrently:
      """json
      [
        { "body": {} },
        { "body": {} },
        { "body": {} },
        { "body": {} }
      ]
      """
    Then exactly 4 responses are 200
    When POST /api/v1/orders is called concurrently:
      """json
      [
        {
          "headers": { "Cookie": "token={carolToken}" },
          "body": { "product_id": 10 }
        },
        {
          "headers": { "Cookie": "token={daveToken}" },
          "body": { "product_id": 10 }
        },
        {
          "headers": { "Cookie": "token={erinToken}" },
          "body": { "product_id": 10 }
        },
        {
          "headers": { "Cookie": "token={frankToken}" },
          "body": { "product_id": 10 }
        }
      ]
      """
    Then exactly 1 response is 201
    And exactly 3 responses are 422
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM orders WHERE product_id = 10;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM products WHERE id = 10 AND stock = 0;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM users WHERE id IN (10, 11, 12, 13) HAVING SUM(points) = 350;
      """

    Examples: three runs — one run catches a removed product lock 499 times in 500, and three leave about one chance in 10^8 of a false green
      | run |
      | 1   |
      | 2   |
      | 3   |

  Scenario Outline: Four simultaneous redemptions on one wallet spend it exactly once
    Given in PostgreSQL:
      """sql
      INSERT INTO users (id, username, password_hash, role, points) VALUES
        (10, 'carol', '$2a$10$MwKxT13/lPxFMjvrkm0dL.QJuNll1zljDU.eOoRVvgrWF.kf/hyC2', 'guest', 20);
      """
    When POST /api/v1/auth/login:
      """json
      {
        "body": {
          "username": "carol",
          "password": "secret123"
        }
      }
      """
    Then response status is 200
    And save response cookie "token" as "carolToken"
    # Four concurrent reads first. The service takes a database connection per
    # in-flight request and the pool starts empty, so on a cold pool three of the
    # four callers below are still opening one while the first commits — the race
    # would not be a race. This puts the four connections in the pool.
    When GET /api/v1/products is called concurrently:
      """json
      [
        { "body": {} },
        { "body": {} },
        { "body": {} },
        { "body": {} }
      ]
      """
    Then exactly 4 responses are 200
    When POST /api/v1/orders is called concurrently:
      """json
      [
        {
          "headers": { "Cookie": "token={carolToken}" },
          "body": { "product_id": 3 }
        },
        {
          "headers": { "Cookie": "token={carolToken}" },
          "body": { "product_id": 3 }
        },
        {
          "headers": { "Cookie": "token={carolToken}" },
          "body": { "product_id": 3 }
        },
        {
          "headers": { "Cookie": "token={carolToken}" },
          "body": { "product_id": 3 }
        }
      ]
      """
    Then exactly 1 response is 201
    And exactly 3 responses are 422
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM orders WHERE user_id = 10;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM users WHERE id = 10 AND points = 0;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM products WHERE id = 3 AND stock = -1;
      """

    Examples: three runs — stock cannot be the limiter here, so the wallet lock is the only thing between one redemption and four
      | run |
      | 1   |
      | 2   |
      | 3   |

  Scenario: Two redemptions sent one after the other are two orders, not one
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
    When POST /api/v1/orders:
      """json
      {
        "headers": {
          "Cookie": "token={aliceToken}"
        },
        "body": {
          "product_id": 1
        }
      }
      """
    Then response status is 201
    And response body contains:
      """json
      {
        "cost_points": 50,
        "balance_after": 50
      }
      """
    When POST /api/v1/orders:
      """json
      {
        "headers": {
          "Cookie": "token={aliceToken}"
        },
        "body": {
          "product_id": 1
        }
      }
      """
    Then response status is 201
    And response body contains:
      """json
      {
        "cost_points": 50,
        "balance_after": 0
      }
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM orders WHERE user_id = 1 AND product_id = 1 HAVING count(*) = 2;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM users WHERE username = 'alice' AND points = 0;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM products WHERE id = 1 AND stock = 1;
      """
