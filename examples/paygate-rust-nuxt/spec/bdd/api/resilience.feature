@resilience
Feature: Surviving the loss of Redis

  Redis holds sessions, the API key cache, the idempotency lock and the rate
  limiter's buckets — and none of the gateway's promises. Every one of those
  has an answer in PostgreSQL, or a declared degradation, when Redis is gone:

    API key lookup      falls back to PostgreSQL
    order uniqueness    guaranteed by UNIQUE (merchant_id, merchant_trade_no)
    refund idempotency  guaranteed by PostgreSQL's primary key on (merchant, key)
    rate limiting       fails OPEN (spec.md, "Failure model")
    dashboard sessions  unavailable: 503, because there is nothing else they live in

  The race below is the one idempotency.feature runs, repeated with Redis
  stopped. It has to come out the same, because that is the proof that the
  Redis lock is an optimisation and not the guarantee.

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

  Scenario: Without Redis an order is created, paid and read exactly as usual
    Given service "redis" is stopped
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "key-acme-noredis-1"
        },
        "body": {
          "merchant_trade_no": "ACME-NOREDIS-1",
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
    When the payment form is submitted to the payment provider
    Then response status is 200
    And the customer pays at the payment provider:
      """json
      {
        "card_number": "4242424242424242",
        "card_expiry": "12/30",
        "card_cvc": "123"
      }
      """
    Then response status is 303
    When payment provider delivers each pending callback 1 time
    Then exactly 1 response is 200
    And payment provider received 1 checkout request
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments WHERE id = '{paymentId}' AND status = 'succeeded';
      """
    # Redis being gone costs sessions and the rate limiter's memory, nothing
    # asynchronous: neither the notifier nor the report pipeline ever reads it,
    # so the merchant is still told and the report still fills in.
    When background work has settled
    Then merchant received 1 notification at "/demo-merchant/api/notify"
    And in ClickHouse query returns 1 row:
      """sql
      SELECT 1 FROM report_events FINAL
       WHERE payment_id = '{paymentId}' AND event_type = 'PaymentSucceeded';
      """
    # And the order the merchant could still create twice is refused by
    # PostgreSQL's unique constraint on (merchant_id, merchant_trade_no), not by
    # anything Redis remembered.
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "key-acme-noredis-1"
        },
        "body": {
          "merchant_trade_no": "ACME-NOREDIS-1",
          "amount": 1000,
          "currency": "USD",
          "item_desc": "Beans",
          "notify_url": "/demo-merchant/api/notify",
          "client_back_url": "/shop/result"
        }
      }
      """
    Then response status is 422
    And response body contains:
      """json
      {
        "error": "MERCHANT_TRADE_NO_TAKEN"
      }
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments HAVING count(*) = 1;
      """

  Scenario: A retried create request still replays its stored answer after Redis dies in between
    # The first attempt cached its answer in Redis while Redis was still up; the
    # retry finds Redis gone entirely and falls back to PostgreSQL's own copy —
    # Redis dying BETWEEN two identical requests, not before either of them,
    # which is the case the rest of this file does not reach.
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "key-acme-middie-1"
        },
        "body": {
          "merchant_trade_no": "ACME-MIDDIE-1",
          "amount": 4200,
          "currency": "USD",
          "item_desc": "Beans",
          "notify_url": "/demo-merchant/api/notify",
          "client_back_url": "/shop/result"
        }
      }
      """
    Then response status is 201
    And save response body field "id" as "paymentId"
    Given service "redis" is stopped
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "key-acme-middie-1"
        },
        "body": {
          "merchant_trade_no": "ACME-MIDDIE-1",
          "amount": 4200,
          "currency": "USD",
          "item_desc": "Beans",
          "notify_url": "/demo-merchant/api/notify",
          "client_back_url": "/shop/result"
        }
      }
      """
    Then response status is 201
    And response header "Idempotent-Replayed" contains "true"
    And response body contains:
      """json
      {
        "id": "{paymentId}"
      }
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments HAVING count(*) = 1;
      """

  Scenario Outline: Without Redis, four refunds racing with one key still produce one refund
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "key-acme-nr-<run>"
        },
        "body": {
          "merchant_trade_no": "ACME-NR-<run>",
          "amount": 10000,
          "currency": "USD",
          "item_desc": "Beans",
          "notify_url": "/demo-merchant/api/notify",
          "client_back_url": "/shop/result"
        }
      }
      """
    Then response status is 201
    And save response body field "id" as "paymentId"
    When the payment form is submitted to the payment provider
    Then response status is 200
    And the customer pays at the payment provider:
      """json
      {
        "card_number": "4242424242424242",
        "card_expiry": "12/30",
        "card_cvc": "123"
      }
      """
    Then response status is 303
    When payment provider delivers each pending callback 1 time
    Then exactly 1 response is 200
    Given service "redis" is stopped
    When GET /api/v1/health/ready is called concurrently:
      """json
      [
        { "body": {} },
        { "body": {} },
        { "body": {} },
        { "body": {} }
      ]
      """
    Then exactly 4 responses are 200
    When POST /api/v1/payments/{paymentId}/refunds is called concurrently:
      """json
      [
        { "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc", "Idempotency-Key": "nr-key-<run>" }, "body": { "amount": 777 } },
        { "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc", "Idempotency-Key": "nr-key-<run>" }, "body": { "amount": 777 } },
        { "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc", "Idempotency-Key": "nr-key-<run>" }, "body": { "amount": 777 } },
        { "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc", "Idempotency-Key": "nr-key-<run>" }, "body": { "amount": 777 } }
      ]
      """
    Then exactly 1 response is 201
    And exactly 3 responses are 409
    And payment provider received 1 refund request
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM refunds WHERE payment_id = '{paymentId}' HAVING count(*) = 1;
      """

    Examples: three runs
      | run |
      | 1   |
      | 2   |
      | 3   |

  Scenario: Without Redis the rate limiter lets traffic through rather than refusing customers
    Given in PostgreSQL:
      """sql
      UPDATE merchants SET rate_limit_per_minute = 1 WHERE id = 1;
      """
    And service "redis" is stopped
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "key-acme-open-1"
        },
        "body": {
          "merchant_trade_no": "ACME-OPEN-1",
          "amount": 100,
          "currency": "USD",
          "item_desc": "Beans",
          "notify_url": "/demo-merchant/api/notify",
          "client_back_url": "/shop/result"
        }
      }
      """
    Then response status is 201
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "key-acme-open-2"
        },
        "body": {
          "merchant_trade_no": "ACME-OPEN-2",
          "amount": 100,
          "currency": "USD",
          "item_desc": "Beans",
          "notify_url": "/demo-merchant/api/notify",
          "client_back_url": "/shop/result"
        }
      }
      """
    Then response status is 201
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments HAVING count(*) = 2;
      """

  Scenario: Redis coming back empty forgets the rate limiter's memory, and lets a burst through
    # The bucket lives only in Redis, which keeps nothing across a restart
    # (spec.md, "Redis — nothing that matters"). Failing open while Redis is
    # DOWN is a declared promise; this is its unavoidable twin, and worth
    # declaring too rather than discovering by surprise: a merchant is not
    # still over its limit once Redis is back, because nothing remembers that
    # it ever was.
    Given in PostgreSQL:
      """sql
      UPDATE merchants SET rate_limit_per_minute = 1 WHERE id = 1;
      """
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "key-acme-burst-1"
        },
        "body": {
          "merchant_trade_no": "ACME-BURST-1",
          "amount": 100,
          "currency": "USD",
          "item_desc": "Beans",
          "notify_url": "/demo-merchant/api/notify",
          "client_back_url": "/shop/result"
        }
      }
      """
    Then response status is 201
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "key-acme-burst-2"
        },
        "body": {
          "merchant_trade_no": "ACME-BURST-2",
          "amount": 100,
          "currency": "USD",
          "item_desc": "Beans",
          "notify_url": "/demo-merchant/api/notify",
          "client_back_url": "/shop/result"
        }
      }
      """
    Then response status is 429
    And response body contains:
      """json
      {
        "error": "RATE_LIMITED"
      }
      """
    Given service "redis" is stopped
    And service "redis" is started
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "key-acme-burst-3"
        },
        "body": {
          "merchant_trade_no": "ACME-BURST-3",
          "amount": 100,
          "currency": "USD",
          "item_desc": "Beans",
          "notify_url": "/demo-merchant/api/notify",
          "client_back_url": "/shop/result"
        }
      }
      """
    Then response status is 201
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments HAVING count(*) = 2;
      """

  Scenario: Without Redis the dashboard refuses to sign in, and signs in again once it is back
    Given service "redis" is stopped
    When POST /api/v1/dashboard/login:
      """json
      {
        "body": { "email": "owner@acme.test", "password": "secret123" }
      }
      """
    Then response status is 503
    And response body contains:
      """json
      {
        "error": "SESSION_STORE_UNAVAILABLE"
      }
      """
    Given service "redis" is started
    When POST /api/v1/dashboard/login:
      """json
      {
        "body": { "email": "owner@acme.test", "password": "secret123" }
      }
      """
    Then response status is 200

  Scenario: Sessions do not survive a Redis restart, and that is the whole cost
    When POST /api/v1/dashboard/login:
      """json
      {
        "body": { "email": "owner@acme.test", "password": "secret123" }
      }
      """
    Then response status is 200
    And save response cookie "session" as "ownerSession"
    Given service "redis" is stopped
    And service "redis" is started
    When GET /api/v1/dashboard/me:
      """json
      {
        "headers": { "Cookie": "session={ownerSession}" }
      }
      """
    Then response status is 401
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "key-acme-restart"
        },
        "body": {
          "merchant_trade_no": "ACME-RESTART",
          "amount": 100,
          "currency": "USD",
          "item_desc": "Beans",
          "notify_url": "/demo-merchant/api/notify",
          "client_back_url": "/shop/result"
        }
      }
      """
    Then response status is 201

  Scenario: Readiness depends on PostgreSQL alone and reports Redis as degraded
    Given service "redis" is stopped
    When GET /api/v1/health/ready:
      """json
      {}
      """
    Then response status is 200
    And response body contains:
      """json
      {
        "status": "ready",
        "checks": {
          "postgres": "up",
          "redis": "down"
        }
      }
      """
