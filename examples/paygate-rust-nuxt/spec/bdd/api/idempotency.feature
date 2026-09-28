@idempotency
Feature: Idempotency keys

  The two endpoints a merchant's server calls that change something — creating
  an order and refunding one — require an `Idempotency-Key`, with the semantics
  of the IETF Idempotency-Key draft (spec.md, "Two kinds of duplicate"). The
  scenarios below use the refund, because it is the one that moves money and
  therefore the one where being wrong costs something:

    no key                          400 IDEMPOTENCY_KEY_MISSING
    first use                       processed; the answer is stored
    same key, same request, done    the stored answer, replayed, with
                                    `Idempotent-Replayed: true`
    same key, same request, running 409 IDEMPOTENCY_KEY_IN_USE
    same key, different request     422 IDEMPOTENCY_KEY_REUSED

  It answers a different question from `merchant_trade_no`, and the difference
  is the point: the key says "this is the same REQUEST, possibly sent twice by a
  flaky network", the trade number says "this is the same ORDER". A retry is
  replayed; a second order that reuses a trade number is refused
  (orders.feature).

  The customer's side has no key at all, and cannot: every hand-off gets a new
  order number at the provider, so a customer who reloads really can pay twice.
  That duplicate is caught after the fact, by its event id and by the order's own
  state, and then refunded (duplicate_payment.feature). Four questions, four
  mechanisms, and only one of them is a header.

  The guarantee is PostgreSQL's (a primary key on merchant and key); Redis only
  makes the common paths cheaper, and resilience.feature runs the race below
  again with Redis stopped to show it.

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
        (2, 2, '6d1c6de2f515558fc0b3e248fd3469d866e2cd955e653f67c41da6ea5bbdff5f', 'sk_test_glob', NULL);
      """
    # One paid order, bought the only way an order can be bought, because a
    # refund needs something to refund.
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "key-acme-idem-1"
        },
        "body": {
          "merchant_trade_no": "ACME-IDEM-1",
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
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments WHERE id = '{paymentId}' AND status = 'succeeded';
      """

  Scenario: A refund without a key is refused and writes nothing
    When POST /api/v1/payments/{paymentId}/refunds:
      """json
      {
        "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc" },
        "body": { "amount": 1000 }
      }
      """
    Then response status is 400
    And response body contains:
      """json
      {
        "error": "IDEMPOTENCY_KEY_MISSING"
      }
      """
    And payment provider received 0 refund requests
    And in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM refunds;
      """

  Scenario: A key present but outside the length this API allows is refused, and the boundary itself is not
    # Empty is not the same fact as absent: the header IS there, so this is
    # IDEMPOTENCY_KEY_MISSING's sibling, not itself — the key is invalid, not missing.
    When POST /api/v1/payments/{paymentId}/refunds:
      """json
      {
        "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc", "Idempotency-Key": "" },
        "body": { "amount": 1000 }
      }
      """
    Then response status is 400
    And response body contains:
      """json
      {
        "error": "IDEMPOTENCY_KEY_INVALID"
      }
      """
    # One character past this API's own maxLength (255, the IETF draft's own
    # advice) is refused the same way — a length rule that stopped at 255
    # would still be a rule if it silently truncated at 256 instead.
    When POST /api/v1/payments/{paymentId}/refunds:
      """json
      {
        "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc", "Idempotency-Key": "key-256-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx" },
        "body": { "amount": 1000 }
      }
      """
    Then response status is 400
    And response body contains:
      """json
      {
        "error": "IDEMPOTENCY_KEY_INVALID"
      }
      """
    And payment provider received 0 refund requests
    And in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM refunds;
      """
    # The boundary itself, 255 characters, is an ordinary key: this API's own
    # maxLength must accept exactly what it advertises, not one short of it.
    When POST /api/v1/payments/{paymentId}/refunds:
      """json
      {
        "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc", "Idempotency-Key": "key-255-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx" },
        "body": { "amount": 1000 }
      }
      """
    Then response status is 201
    And payment provider received 1 refund request
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM refunds WHERE payment_id = '{paymentId}' AND amount = 1000 HAVING count(*) = 1;
      """

  Scenario: A retry of a completed refund replays the stored answer
    When POST /api/v1/payments/{paymentId}/refunds:
      """json
      {
        "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc", "Idempotency-Key": "retry-me-1" },
        "body": { "amount": 1000, "reason": "requested_by_customer" }
      }
      """
    Then response status is 201
    And save response body field "id" as "refundId"
    # The same request with its keys in another order: canonically identical.
    When POST /api/v1/payments/{paymentId}/refunds:
      """json
      {
        "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc", "Idempotency-Key": "retry-me-1" },
        "body": { "reason": "requested_by_customer", "amount": 1000 }
      }
      """
    Then response status is 201
    And response header "Idempotent-Replayed" contains "true"
    And response body contains:
      """json
      {
        "id": "{refundId}",
        "amount": 1000
      }
      """
    And payment provider received 1 refund request
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM refunds WHERE payment_id = '{paymentId}' HAVING count(*) = 1;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments WHERE id = '{paymentId}' AND amount_refunded = 1000;
      """

  Scenario: A stored answer that is no longer an answer is a failure, not a replay
    # A replay is only ever as good as what was stored, and this scenario
    # corrupts what was stored — deliberately, because the alternative is to
    # trust that nothing ever will. What makes it worth a scenario is which way
    # the mistake goes: a status that cannot be parsed is not a neutral value,
    # and the tempting default for it is `200`. A merchant told `422` the first
    # time would then be told the opposite the second, about the same request.
    When POST /api/v1/payments/{paymentId}/refunds:
      """json
      {
        "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc", "Idempotency-Key": "corrupt-store-1" },
        "body": { "amount": 1000 }
      }
      """
    Then response status is 201
    Given in PostgreSQL:
      """sql
      UPDATE idempotency_keys SET response_status = 1000
       WHERE merchant_id = 1 AND idempotency_key = 'corrupt-store-1';
      """
    When POST /api/v1/payments/{paymentId}/refunds:
      """json
      {
        "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc", "Idempotency-Key": "corrupt-store-1" },
        "body": { "amount": 1000 }
      }
      """
    Then response status is 500
    And response body contains:
      """json
      {
        "error": "INTERNAL"
      }
      """
    # It fails, and it fails safely: the key is still spent, the refund still
    # happened exactly once, and the provider was not asked again.
    And payment provider received 1 refund request
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM refunds WHERE payment_id = '{paymentId}' HAVING count(*) = 1;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments WHERE id = '{paymentId}' AND amount_refunded = 1000;
      """

  Scenario: Reusing a key for a different request is refused and leaves the first alone
    When POST /api/v1/payments/{paymentId}/refunds:
      """json
      {
        "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc", "Idempotency-Key": "reused-1" },
        "body": { "amount": 1000 }
      }
      """
    Then response status is 201
    When POST /api/v1/payments/{paymentId}/refunds:
      """json
      {
        "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc", "Idempotency-Key": "reused-1" },
        "body": { "amount": 9000 }
      }
      """
    Then response status is 422
    And response body contains:
      """json
      {
        "error": "IDEMPOTENCY_KEY_REUSED"
      }
      """
    And payment provider received 1 refund request
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM refunds WHERE payment_id = '{paymentId}' AND amount = 1000 HAVING count(*) = 1;
      """
    And in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM refunds WHERE amount = 9000;
      """

  Scenario: A key spent on one order's refund cannot be spent on another's
    # The request fingerprint covers the path, not only the body: two refunds of
    # the same amount are still two different requests when they are refunds of
    # different orders.
    Given in PostgreSQL:
      """sql
      INSERT INTO payments
        (id, merchant_id, merchant_trade_no, amount, currency, status, item_desc,
         notify_url, client_back_url, amount_refunded)
      VALUES
        ('01931c4f-0000-7000-8000-00000000000b', 1, 'ACME-IDEM-2', 10000, 'USD', 'succeeded', 'Beans',
         '/demo-merchant/api/notify', '/shop/result', 0);
      """
    When POST /api/v1/payments/{paymentId}/refunds:
      """json
      {
        "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc", "Idempotency-Key": "one-key-two-orders" },
        "body": { "amount": 1000 }
      }
      """
    Then response status is 201
    When POST /api/v1/payments/01931c4f-0000-7000-8000-00000000000b/refunds:
      """json
      {
        "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc", "Idempotency-Key": "one-key-two-orders" },
        "body": { "amount": 1000 }
      }
      """
    Then response status is 422
    And response body contains:
      """json
      {
        "error": "IDEMPOTENCY_KEY_REUSED"
      }
      """
    And payment provider received 1 refund request
    And in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM refunds WHERE payment_id = '01931c4f-0000-7000-8000-00000000000b';
      """

  Scenario: The same key sent by two merchants is two keys
    Given in PostgreSQL:
      """sql
      INSERT INTO payments
        (id, merchant_id, merchant_trade_no, amount, currency, status, item_desc,
         notify_url, client_back_url, amount_refunded)
      VALUES
        ('01931c4f-0000-7000-8000-00000000000c', 2, 'GLOBEX-IDEM-1', 10000, 'EUR', 'succeeded', 'Widget',
         '/demo-merchant/api/notify', '/shop/result', 0);
      """
    # A paid order is not refundable on its own: a refund goes to the provider
    # against the trade number of the attempt that took the money (spec.md,
    # "Refunds"), so a seeded order needs the attempt that paid it. The sibling
    # scenario below seeds one; this one did not, and the refund was correctly
    # refused with `PAYMENT_NOT_REFUNDABLE` before the idempotency scoping this
    # scenario is about could be exercised at all.
    And in PostgreSQL:
      """sql
      INSERT INTO payment_attempts
        (id, payment_id, provider_code, provider_trade_no, status, provider_charge_id, started_at, settled_at)
      VALUES
        ('01931c4f-0000-7000-8000-00000000000f', '01931c4f-0000-7000-8000-00000000000c',
         'newebpay', 'GLBX00000002SNc3d4', 'succeeded', 'ch_seeded_c', NOW(), NOW());
      """
    When POST /api/v1/payments/{paymentId}/refunds:
      """json
      {
        "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc", "Idempotency-Key": "shared-key" },
        "body": { "amount": 1000 }
      }
      """
    Then response status is 201
    When POST /api/v1/payments/01931c4f-0000-7000-8000-00000000000c/refunds:
      """json
      {
        "headers": { "Authorization": "Bearer sk_test_globex_9kR2mQ7vX4pL8nT3wZ6yB1cF", "Idempotency-Key": "shared-key" },
        "body": { "amount": 1000 }
      }
      """
    Then response status is 201
    And payment provider received 2 refund requests
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM idempotency_keys WHERE idempotency_key = 'shared-key' HAVING count(*) = 2;
      """

  Scenario: Two merchants spending the same key at one instant are two keys
    # The sequential version of this is above; this is the one that would catch a
    # lock or a cache keyed on the key alone instead of on (merchant, key). Each
    # merchant refunds its OWN payment, so the two requests are to different
    # paths — which is why they need the general concurrent step.
    Given in PostgreSQL:
      """sql
      INSERT INTO payments
        (id, merchant_id, merchant_trade_no, amount, currency, status, item_desc,
         notify_url, client_back_url, amount_refunded)
      VALUES
        ('01931c4f-0000-7000-8000-00000000000d', 2, 'GLOBEX-RACE-1', 10000, 'EUR', 'succeeded', 'Widget',
         '/demo-merchant/api/notify', '/shop/result', 0);
      """
    And in PostgreSQL:
      """sql
      INSERT INTO payment_attempts
        (id, payment_id, provider_code, provider_trade_no, status, provider_charge_id, started_at, settled_at)
      VALUES
        ('01931c4f-0000-7000-8000-00000000000e', '01931c4f-0000-7000-8000-00000000000d',
         'newebpay', 'GLBX00000001SNa1b2', 'succeeded', 'ch_seeded_d', NOW(), NOW());
      """
    When these things happen at one instant:
      """json
      [
        {"method": "GET", "path": "/api/v1/health/ready"},
        {"method": "GET", "path": "/api/v1/health/ready"}
      ]
      """
    Then exactly 2 responses are 200
    When these things happen at one instant:
      """json
      [
        {
          "method": "POST",
          "path": "/api/v1/payments/{paymentId}/refunds",
          "headers": {
            "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
            "Idempotency-Key": "same-key-two-shops"
          },
          "body": {
            "amount": 1000
          }
        },
        {
          "method": "POST",
          "path": "/api/v1/payments/01931c4f-0000-7000-8000-00000000000d/refunds",
          "headers": {
            "Authorization": "Bearer sk_test_globex_9kR2mQ7vX4pL8nT3wZ6yB1cF",
            "Idempotency-Key": "same-key-two-shops"
          },
          "body": {
            "amount": 1000
          }
        }
      ]
      """
    Then exactly 2 responses are 201
    And payment provider received 2 refund requests
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM idempotency_keys
       WHERE idempotency_key = 'same-key-two-shops' HAVING count(*) = 2;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM refunds WHERE merchant_id = 1 AND amount = 1000 HAVING count(*) = 1;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM refunds WHERE merchant_id = 2 AND amount = 1000 HAVING count(*) = 1;
      """

  Scenario: A request refused by validation does not consume its key
    When POST /api/v1/payments/{paymentId}/refunds:
      """json
      {
        "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc", "Idempotency-Key": "fix-and-retry" },
        "body": { "amount": 0 }
      }
      """
    Then response status is 422
    # Scoped to this scenario's own key. The Background created its order with
    # `key-acme-idem-1`, and that key is legitimately still on the books as
    # completed — replaying a successful answer is what it is for. What must not
    # be here is the key of the request that was just refused.
    And in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM idempotency_keys WHERE idempotency_key = 'fix-and-retry';
      """
    When POST /api/v1/payments/{paymentId}/refunds:
      """json
      {
        "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc", "Idempotency-Key": "fix-and-retry" },
        "body": { "amount": 1000 }
      }
      """
    Then response status is 201
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM refunds HAVING count(*) = 1;
      """

  Scenario: A key whose retention has expired is a new key
    Given in PostgreSQL:
      """sql
      INSERT INTO idempotency_keys
        (merchant_id, idempotency_key, request_fingerprint, state, response_status, response_body, created_at, expires_at)
      VALUES
        (1, 'old-key-1', repeat('0', 64), 'completed', 201,
         '{"id": "00000000-0000-7000-8000-00000000abcd", "amount": 4321}',
         NOW() - INTERVAL '25 hours', NOW() - INTERVAL '1 hour');
      """
    When POST /api/v1/payments/{paymentId}/refunds:
      """json
      {
        "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc", "Idempotency-Key": "old-key-1" },
        "body": { "amount": 1000 }
      }
      """
    Then response status is 201
    And response body does not contain "00000000-0000-7000-8000-00000000abcd"
    And response body does not contain "4321"
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM idempotency_keys
       WHERE merchant_id = 1
         AND idempotency_key = 'old-key-1'
         AND state = 'completed'
         AND expires_at > NOW() + INTERVAL '23 hours';
      """

  Scenario Outline: Four refunds racing with one key produce one refund
    # The provider takes 800 ms over a refund of 777, which holds the winner
    # inside the provider call while the other three arrive — so the three are
    # refused as IN_USE rather than, by luck of timing, replayed after it
    # finished. Without that window the race would still be safe, but its
    # outcome would not be exact, and an inexact outcome is not an assertion.
    When these things happen at one instant:
      """json
      [
        {"method": "GET", "path": "/api/v1/health/ready", "body": {}},
        {"method": "GET", "path": "/api/v1/health/ready", "body": {}},
        {"method": "GET", "path": "/api/v1/health/ready", "body": {}},
        {"method": "GET", "path": "/api/v1/health/ready", "body": {}}
      ]
      """
    Then exactly 4 responses are 200
    When these things happen at one instant:
      """json
      [
        {
          "method": "POST",
          "path": "/api/v1/payments/{paymentId}/refunds",
          "headers": {
            "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
            "Idempotency-Key": "race-key-<run>"
          },
          "body": {
            "amount": 777
          }
        },
        {
          "method": "POST",
          "path": "/api/v1/payments/{paymentId}/refunds",
          "headers": {
            "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
            "Idempotency-Key": "race-key-<run>"
          },
          "body": {
            "amount": 777
          }
        },
        {
          "method": "POST",
          "path": "/api/v1/payments/{paymentId}/refunds",
          "headers": {
            "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
            "Idempotency-Key": "race-key-<run>"
          },
          "body": {
            "amount": 777
          }
        },
        {
          "method": "POST",
          "path": "/api/v1/payments/{paymentId}/refunds",
          "headers": {
            "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
            "Idempotency-Key": "race-key-<run>"
          },
          "body": {
            "amount": 777
          }
        }
      ]
      """
    Then exactly 1 response is 201
    And exactly 3 responses are 409
    And payment provider received 1 refund request
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM refunds WHERE payment_id = '{paymentId}' HAVING count(*) = 1 AND sum(amount) = 777;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments WHERE id = '{paymentId}' AND amount_refunded = 777;
      """
    When POST /api/v1/payments/{paymentId}/refunds:
      """json
      {
        "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc", "Idempotency-Key": "race-key-<run>" },
        "body": { "amount": 777 }
      }
      """
    Then response status is 201
    And response header "Idempotent-Replayed" contains "true"
    And payment provider received 1 refund request

    Examples: three runs
      | run |
      | 1   |
      | 2   |
      | 3   |
