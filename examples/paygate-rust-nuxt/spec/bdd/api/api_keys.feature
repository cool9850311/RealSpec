@auth @api-keys
Feature: Merchant API keys

  A merchant's server authenticates with a secret key in the Authorization
  header. The gateway stores only the key's SHA-256 — the raw key is shown
  once, when it is created, and never again — and caches the lookup in Redis.
  A cache is a copy, so the one thing it must never do is outlive the original:
  revoking a key takes effect on the next request, cached or not.

  A key is what the merchant's SERVER carries: creating an order, asking about
  one, refunding one. It is not what a provider carries, which is a signature —
  and it is nothing the customer ever carries at all, because paygate has no
  page, token or endpoint a browser calls (spec.md, "paygate is not on the
  payment path"). Each of the two credentials works in exactly one place, and
  the scenarios present the wrong one on purpose.

  The keys below are literals, and the Background seeds the SHA-256 of each
  beside it, so neither can change without the other.

  Background:
    Given run migration
    And in PostgreSQL:
      """sql
      INSERT INTO merchants (id, name, currency, timezone, provider_code, provider_merchant_id, rate_limit_per_minute, hash_key, hash_iv) VALUES
        (1, 'Acme Coffee', 'USD', 'Asia/Taipei', 'ecpay',    '2000132',     600, 'acmehashkey0123456789abcdef01234', 'acmehashiv012345'),
        (2, 'Globex',      'EUR', 'UTC', 'newebpay',         'MS150086913', 600, 'globexhashkey0123456789abcdef012', 'globexhashiv0123');
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
        (1, 1, 'ab1ebc7221679c3334542406232b7620b2c02bd1d9a9573a53c29ae3dabcc34a', 'sk_test_acme', NULL),
        (2, 2, '6d1c6de2f515558fc0b3e248fd3469d866e2cd955e653f67c41da6ea5bbdff5f', 'sk_test_glob', NULL),
        (3, 1, 'fdfa2848570b1d9629c7083d3e1b1fe8beb959cb8f212dc9adb0e857550ccd1c', 'sk_test_acme', NOW() - INTERVAL '1 day');
      """
    And in PostgreSQL:
      """sql
      INSERT INTO dashboard_users (id, merchant_id, email, password_hash) VALUES
        (1, 1, 'owner@acme.test',   '$2a$10$MwKxT13/lPxFMjvrkm0dL.QJuNll1zljDU.eOoRVvgrWF.kf/hyC2'),
        (2, 2, 'owner@globex.test', '$2a$10$MwKxT13/lPxFMjvrkm0dL.QJuNll1zljDU.eOoRVvgrWF.kf/hyC2');
      """

  Scenario Outline: A request without a valid key is refused before anything else is looked at
    # The order named here does not exist. A `404` would mean the gateway went
    # looking before it decided who was asking; every row below must be `401`.
    # The hash lookup is exact-bytes: a key that is right except for its case
    # or an extra space is a DIFFERENT string, hashes to a different digest,
    # and matches no row — the same refusal as a key that was never real.
    # `bearer` in lowercase tests the same thing one level up: HTTP treats an
    # auth-scheme as case-insensitive (RFC 9110 §11.1), but this gateway
    # compares the literal prefix it issues, "Bearer", so a client that
    # lower-cases it (as some HTTP libraries do by convention) is refused
    # rather than silently accepted.
    When GET /api/v1/payments?merchant_trade_no=TN-UNAUTH:
      """json
      {
        "headers": {
          "Authorization": "<authorization>"
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

    Examples:
      | authorization                                       |
      | Bearer sk_test_acme_notarealkeynotarealkey00         |
      | Bearer sk_test_acme_revokedrevokedrevoked01          |
      | Basic c2tfdGVzdF9hY21lXzRlQzM5SHFMeWpXRGFyanRUMXpkcDdkYzo= |
      | sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc                |
      | Bearer                                              |
      | Bearer SK_TEST_ACME_4EC39HQLYJWDARJTT1ZDP7DC        |
      | bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc        |
      | Bearer  sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc       |

  Scenario: A request with no Authorization header at all is refused
    When GET /api/v1/payments?merchant_trade_no=TN-NO-AUTH:
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

  Scenario: A key is created once, shown once, stored only as a hash, and works immediately
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
    When POST /api/v1/dashboard/api-keys:
      """json
      {
        "headers": {
          "Cookie": "session={ownerSession}"
        }
      }
      """
    Then response status is 201
    And response body contains:
      """json
      {
        "id": "<non-null>",
        "key": "<non-null>",
        "key_prefix": "sk_test_acme",
        "created_at": "<non-null>"
      }
      """
    And save response body field "key" as "newKey"
    And save response body field "id" as "newKeyId"
    And in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM api_keys WHERE key_hash = '{newKey}' OR key_prefix = '{newKey}';
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM api_keys
       WHERE id = '{newKeyId}'
         AND merchant_id = 1
         AND key_hash = encode(sha256(convert_to('{newKey}', 'UTF8')), 'hex')
         AND revoked_at IS NULL;
      """
    # An order created the only way orders are created — by the browser, with no
    # key — and then read with the key that was just minted.
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "key-tn-new-key-1"
        },
        "body": {
          "merchant_trade_no": "TN-NEW-KEY-1",
          "amount": 1000,
          "currency": "USD",
          "item_desc": "Beans",
          "notify_url": "/demo-merchant/api/notify",
          "client_back_url": "/shop/result"
        }
      }
      """
    Then response status is 201
    And save response body field "id" as "paymentId"
    When GET /api/v1/payments/{paymentId}:
      """json
      {
        "headers": {
          "Authorization": "Bearer {newKey}"
        }
      }
      """
    Then response status is 200
    And response body contains:
      """json
      {
        "merchant_trade_no": "TN-NEW-KEY-1",
        "status": "pending"
      }
      """

  Scenario: Revoking a key refuses it on the very next request, although it was cached
    # The first read puts the key into the Redis cache. The revocation goes
    # through the dashboard, which is the only way a key is revoked, and the
    # next request must see it — a cache that served the key until its TTL ran
    # out would answer 200 here.
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "key-tn-revoke-1"
        },
        "body": {
          "merchant_trade_no": "TN-REVOKE-1",
          "amount": 1000,
          "currency": "USD",
          "item_desc": "Beans",
          "notify_url": "/demo-merchant/api/notify",
          "client_back_url": "/shop/result"
        }
      }
      """
    Then response status is 201
    And save response body field "id" as "paymentId"
    When GET /api/v1/payments/{paymentId}:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc"
        }
      }
      """
    Then response status is 200
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
    When DELETE /api/v1/dashboard/api-keys/1:
      """json
      {
        "headers": {
          "Cookie": "session={ownerSession}"
        }
      }
      """
    Then response status is 204
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM api_keys WHERE id = 1 AND revoked_at IS NOT NULL;
      """
    When GET /api/v1/payments/{paymentId}:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc"
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
    # Revoking a key takes nothing away from the order it could once read.
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments WHERE id = '{paymentId}' AND status = 'pending';
      """

  Scenario: A merchant cannot revoke another merchant's key
    When POST /api/v1/dashboard/login:
      """json
      {
        "body": {
          "email": "owner@globex.test",
          "password": "secret123"
        }
      }
      """
    Then response status is 200
    And save response cookie "session" as "globexSession"
    When DELETE /api/v1/dashboard/api-keys/1:
      """json
      {
        "headers": {
          "Cookie": "session={globexSession}"
        }
      }
      """
    Then response status is 404
    And response body contains:
      """json
      {
        "error": "API_KEY_NOT_FOUND"
      }
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM api_keys WHERE id = 1 AND revoked_at IS NULL;
      """
    When GET /api/v1/payments?merchant_trade_no=TN-STILL-VALID:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc"
        }
      }
      """
    Then response status is 404
    And response body contains:
      """json
      {
        "error": "PAYMENT_NOT_FOUND"
      }
      """

  Scenario: Keys are managed only with a dashboard session, never with a key
    When POST /api/v1/dashboard/api-keys:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc"
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
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM api_keys HAVING count(*) = 3;
      """
