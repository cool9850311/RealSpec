@auth @sessions
Feature: Dashboard sessions

  A dashboard user signs in with email and password and receives an opaque
  session token in an HttpOnly `session` cookie. The token means nothing by
  itself: it is a key into Redis, where the session lives under the token's
  SHA-256, so signing out deletes it and the same cookie is refused on the next
  request — which a signed, self-contained token could not promise.

  A readable `session_hint=1` cookie rides beside it for the front end's sake,
  exactly as in minimart (spec.md, "Authentication"): it grants nothing, and
  every 401 expires it.

  Background:
    Given run migration
    And in PostgreSQL:
      """sql
      INSERT INTO merchants (id, name, currency, timezone, provider_code, provider_merchant_id, rate_limit_per_minute, hash_key, hash_iv) VALUES
        (1, 'Acme Coffee', 'USD', 'Asia/Taipei', 'ecpay',    '2000132',     600, 'acmehashkey0123456789abcdef01234', 'acmehashiv012345');
      """
    And in PostgreSQL:
      """sql
      INSERT INTO providers (code, platform_id, hash_key, hash_iv, cashier_url, query_url, refund_url) VALUES
        ('ecpay',    '3002607',    'pwFHCqoQZGmho4w6',                 'EkRm7iFT261dpevs', '/provider/ecpay/Cashier/AioCheckOut/V5', '/provider/ecpay/Cashier/QueryTradeInfo/V5', '/provider/ecpay/CreditDetail/DoAction'),
        ('newebpay', 'MS12345678', 'Fs5cX1TGqYZ3kLmN8pQrS2tUvW4xY6zA', 'B7cD9eF1gH3iJ5kL', '/provider/newebpay/MPG/mpg_gateway',     '/provider/newebpay/API/QueryTradeInfo',     '/provider/newebpay/API/CreditCard/Close');
      """
    And in PostgreSQL:
      """sql
      INSERT INTO api_keys (id, merchant_id, key_hash, key_prefix, revoked_at) VALUES
        (1, 1, 'ab1ebc7221679c3334542406232b7620b2c02bd1d9a9573a53c29ae3dabcc34a', 'sk_test_acme', NULL);
      """
    And in PostgreSQL:
      """sql
      INSERT INTO dashboard_users (id, merchant_id, email, password_hash) VALUES
        (1, 1, 'owner@acme.test', '$2a$10$MwKxT13/lPxFMjvrkm0dL.QJuNll1zljDU.eOoRVvgrWF.kf/hyC2');
      """

  Scenario: Correct credentials open a session and the response leaks nothing
    When POST /api/v1/dashboard/login:
      """json
      {
        "body": {
          "email": "owner@acme.test",
          "password": "secret123"
        }
      }
      """
    Then response status is 200
    And response body contains:
      """json
      {
        "email": "owner@acme.test",
        "merchant": {
          "id": 1,
          "name": "Acme Coffee",
          "currency": "USD",
          "timezone": "Asia/Taipei"
        }
      }
      """
    And response body does not contain "password"
    And response body does not contain "$2a$10$"
    And response header "Set-Cookie" contains "session="
    And response header "Set-Cookie" contains "HttpOnly"
    And response header "Set-Cookie" contains "SameSite=Lax"
    And response header "Set-Cookie" contains "session_hint=1"
    And save response cookie "session" as "ownerSession"
    When GET /api/v1/dashboard/me:
      """json
      {
        "headers": {
          "Cookie": "session={ownerSession}"
        }
      }
      """
    Then response status is 200
    And response body contains:
      """json
      {
        "email": "owner@acme.test",
        "merchant": {
          "id": 1,
          "name": "Acme Coffee"
        }
      }
      """

  Scenario Outline: Wrong credentials are one answer, whichever half was wrong
    When POST /api/v1/dashboard/login:
      """json
      {
        "body": {
          "email": "<email>",
          "password": "<password>"
        }
      }
      """
    Then response status is 401
    And response body contains:
      """json
      {
        "error": "INVALID_CREDENTIALS"
      }
      """

    Examples:
      | email             | password  |
      | owner@acme.test   | wrongpass |
      | nobody@acme.test  | secret123 |

  Scenario: Signing out ends the session on the server, not just in the browser
    When POST /api/v1/dashboard/login:
      """json
      {
        "body": {
          "email": "owner@acme.test",
          "password": "secret123"
        }
      }
      """
    Then response status is 200
    And save response cookie "session" as "ownerSession"
    When POST /api/v1/dashboard/logout:
      """json
      {
        "headers": {
          "Cookie": "session={ownerSession}"
        }
      }
      """
    Then response status is 204
    And response header "Set-Cookie" contains "session=;"
    And response header "Set-Cookie" contains "Max-Age=0"
    When GET /api/v1/dashboard/me:
      """json
      {
        "headers": {
          "Cookie": "session={ownerSession}"
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
    And response header "Set-Cookie" contains "session_hint=;"

  Scenario: Two sessions of one user are independent, and ending one leaves the other
    When POST /api/v1/dashboard/login:
      """json
      {
        "body": {
          "email": "owner@acme.test",
          "password": "secret123"
        }
      }
      """
    Then response status is 200
    And save response cookie "session" as "laptopSession"
    When POST /api/v1/dashboard/login:
      """json
      {
        "body": {
          "email": "owner@acme.test",
          "password": "secret123"
        }
      }
      """
    Then response status is 200
    And save response cookie "session" as "phoneSession"
    When POST /api/v1/dashboard/logout:
      """json
      {
        "headers": {
          "Cookie": "session={laptopSession}"
        }
      }
      """
    Then response status is 204
    When GET /api/v1/dashboard/me:
      """json
      {
        "headers": {
          "Cookie": "session={phoneSession}"
        }
      }
      """
    Then response status is 200

  Scenario: A request with no session, or one nobody issued, is anonymous
    When GET /api/v1/dashboard/me:
      """json
      {}
      """
    Then response status is 401
    And response body contains:
      """json
      {
        "error": "UNAUTHENTICATED"
      }
      """
    When GET /api/v1/dashboard/me:
      """json
      {
        "headers": {
          "Cookie": "session=not-a-session"
        }
      }
      """
    Then response status is 401
    And response header "Set-Cookie" contains "session_hint=;"

  Scenario: A merchant API key is not a dashboard session, and a session is not an API key
    When GET /api/v1/dashboard/me:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc"
        }
      }
      """
    Then response status is 401
    When POST /api/v1/dashboard/login:
      """json
      {
        "body": {
          "email": "owner@acme.test",
          "password": "secret123"
        }
      }
      """
    Then response status is 200
    And save response cookie "session" as "ownerSession"
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer {ownerSession}",
          "Idempotency-Key": "key-tn-session-as-key-1"
        },
        "body": {
          "merchant_trade_no": "TN-SESSION-AS-KEY-1",
          "amount": 1000,
          "currency": "USD",
          "item_desc": "Beans",
          "notify_url": "/demo-merchant/api/notify",
          "client_back_url": "/shop/result"
        }
      }
      """
    Then response status is 401
    And payment provider received 0 checkout requests
    And in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM payments;
      """

  Scenario: Logging in never adopts a session id the client already had
    # Session fixation: if login reused whatever `session` cookie the caller
    # showed up with, an attacker could plant a known value in the victim's
    # browser before they sign in and then use that same value themselves
    # afterwards. A random token minted fresh on every login is the only
    # defence (OWASP Session Management Cheat Sheet), so the value offered
    # here must come back unauthenticated even after a real login succeeded
    # in the same request's presence.
    When POST /api/v1/dashboard/login:
      """json
      {
        "headers": {
          "Cookie": "session=attacker0chosen0session0value000"
        },
        "body": {
          "email": "owner@acme.test",
          "password": "secret123"
        }
      }
      """
    Then response status is 200
    And save response cookie "session" as "ownerSession"
    When GET /api/v1/dashboard/me:
      """json
      {
        "headers": {
          "Cookie": "session=attacker0chosen0session0value000"
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
    When GET /api/v1/dashboard/me:
      """json
      {
        "headers": {
          "Cookie": "session={ownerSession}"
        }
      }
      """
    Then response status is 200
