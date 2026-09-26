@refunds
Feature: Refunds

  A merchant refunds a paid order through the merchant API, in full or in part,
  the way ECPay's `DoAction` with `Action=R` takes a `TotalAmount`. The rule is
  that the refunds of one payment can never add up to more than was charged,
  and it is enforced three times over: by the use case under a row lock on the
  payment, by a CHECK constraint on `payments.amount_refunded`, and by the race
  at the end of this file that tries to break it.

  A refund is final at the provider when the provider answers, so a refund is
  `succeeded` or it does not exist. Each one is reported to the merchant by the
  same callback machinery a payment uses (notify.feature).

  One thing to know while reading this file: ECPay's refund API (`DoAction`) has
  **no test environment** — its documentation says the API cannot be used there
  because no real authorisation can be issued — needs an IP
  allowlist in production, must not be called during its nightly close window,
  and refuses partial refunds on instalments. Its own WooCommerce plugin does not
  implement refunds at all. So the mock is not a convenience here; it is the only
  counterparty this path can ever have before going live, and that is worth
  saying out loud in the file that depends on it.

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
    # A paid order of 10000 to refund, created and paid the way production does
    # it, so the provider really holds a charge for the refund to reverse.
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "key-acme-refundable"
        },
        "body": {
          "merchant_trade_no": "ACME-REFUNDABLE",
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

  Scenario: A full refund closes the payment
    When POST /api/v1/payments/{paymentId}/refunds:
      """json
      {
        "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc", "Idempotency-Key": "refund-full" },
        "body": { "amount": 10000, "reason": "requested_by_customer" }
      }
      """
    Then response status is 201
    And response body contains:
      """json
      {
        "id": "<non-null>",
        "payment_id": "{paymentId}",
        "amount": 10000,
        "currency": "USD",
        "status": "succeeded",
        "reason": "requested_by_customer",
        "created_at": "<non-null>"
      }
      """
    And save response body field "id" as "refundId"
    And payment provider received 1 refund request
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM refunds
       WHERE id = '{refundId}'
         AND payment_id = '{paymentId}'
         AND merchant_id = 1
         AND amount = 10000
         AND status = 'succeeded'
         AND provider_refund_id IS NOT NULL;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments
       WHERE id = '{paymentId}' AND status = 'refunded' AND amount_refunded = 10000;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payment_events
       WHERE payment_id = '{paymentId}'
         AND event_type = 'PaymentRefunded'
         AND payload->>'refund_id' = '{refundId}'
         AND (payload->>'amount')::bigint = 10000
      HAVING count(*) = 1;
      """

  Scenario: Partial refunds add up, and the one that would pass the charge is refused without effect
    When POST /api/v1/payments/{paymentId}/refunds:
      """json
      {
        "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc", "Idempotency-Key": "refund-part-1" },
        "body": { "amount": 3000 }
      }
      """
    Then response status is 201
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments
       WHERE id = '{paymentId}' AND status = 'succeeded' AND amount_refunded = 3000;
      """
    When POST /api/v1/payments/{paymentId}/refunds:
      """json
      {
        "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc", "Idempotency-Key": "refund-part-2" },
        "body": { "amount": 6000 }
      }
      """
    Then response status is 201
    When POST /api/v1/payments/{paymentId}/refunds:
      """json
      {
        "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc", "Idempotency-Key": "refund-part-3" },
        "body": { "amount": 1001 }
      }
      """
    Then response status is 422
    And response body contains:
      """json
      {
        "error": "REFUND_EXCEEDS_REMAINING",
        "remaining": 1000
      }
      """
    And payment provider received 2 refund requests
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM refunds WHERE payment_id = '{paymentId}' HAVING count(*) = 2 AND sum(amount) = 9000;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments WHERE id = '{paymentId}' AND amount_refunded = 9000 AND status = 'succeeded';
      """
    # The last 1000 is still refundable, and refunding it closes the payment.
    When POST /api/v1/payments/{paymentId}/refunds:
      """json
      {
        "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc", "Idempotency-Key": "refund-part-4" },
        "body": { "amount": 1000 }
      }
      """
    Then response status is 201
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments
       WHERE id = '{paymentId}' AND amount_refunded = 10000 AND status = 'refunded';
      """

  Scenario: An order that was never paid cannot be refunded
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "key-acme-unpaid"
        },
        "body": {
          "merchant_trade_no": "ACME-UNPAID",
          "amount": 5000,
          "currency": "USD",
          "item_desc": "Beans",
          "notify_url": "/demo-merchant/api/notify",
          "client_back_url": "/shop/result"
        }
      }
      """
    Then response status is 201
    And save response body field "id" as "pendingId"
    When POST /api/v1/payments/{pendingId}/refunds:
      """json
      {
        "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc", "Idempotency-Key": "refund-unpaid" },
        "body": { "amount": 100 }
      }
      """
    Then response status is 422
    And response body contains:
      """json
      {
        "error": "PAYMENT_NOT_REFUNDABLE"
      }
      """
    And payment provider received 0 refund requests
    And in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM refunds;
      """

  Scenario: A refund amount that breaks a rule is refused before the provider is asked
    When POST /api/v1/payments/{paymentId}/refunds:
      """json
      {
        "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc", "Idempotency-Key": "refund-bad-zero" },
        "body": { "amount": 0 }
      }
      """
    Then response status is 422
    And response body contains:
      """json
      {
        "error": "VALIDATION_FAILED",
        "field": "amount"
      }
      """
    When POST /api/v1/payments/{paymentId}/refunds:
      """json
      {
        "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc", "Idempotency-Key": "refund-bad-negative" },
        "body": { "amount": -500 }
      }
      """
    Then response status is 422
    # Negative zero is still not a positive integer: the same rule that
    # refuses 0 must refuse the sign-carrying spelling of it too.
    When POST /api/v1/payments/{paymentId}/refunds:
      """json
      {
        "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc", "Idempotency-Key": "refund-bad-negative-zero" },
        "body": { "amount": -0 }
      }
      """
    Then response status is 422
    And response body contains:
      """json
      {
        "error": "VALIDATION_FAILED",
        "field": "amount"
      }
      """
    When POST /api/v1/payments/{paymentId}/refunds:
      """json
      {
        "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc", "Idempotency-Key": "refund-bad-fraction" },
        "body": { "amount": 12.5 }
      }
      """
    Then response status is 422
    When POST /api/v1/payments/{paymentId}/refunds:
      """json
      {
        "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc", "Idempotency-Key": "refund-bad-too-much" },
        "body": { "amount": 10001 }
      }
      """
    Then response status is 422
    And response body contains:
      """json
      {
        "error": "REFUND_EXCEEDS_REMAINING",
        "remaining": 10000
      }
      """
    And payment provider received 0 refund requests
    And in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM refunds;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments WHERE id = '{paymentId}' AND amount_refunded = 0;
      """

  Scenario: A malformed refund request is refused before the provider is asked
    # A number typed as a string is not "amount", whatever it says — additionalProperties: false
    # and the field's own type both exist so an integration finds out at the
    # first attempt instead of paygate coercing something that looks close enough.
    When POST /api/v1/payments/{paymentId}/refunds:
      """json
      {
        "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc", "Idempotency-Key": "refund-bad-string-amount" },
        "body": { "amount": "1000" }
      }
      """
    Then response status is 400
    And response body contains:
      """json
      {
        "error": "INVALID_REQUEST"
      }
      """
    # An amount too large to be a minor-unit integer anywhere can't be
    # deserialised into one; it is a shape problem, not a remaining-balance one.
    When POST /api/v1/payments/{paymentId}/refunds:
      """json
      {
        "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc", "Idempotency-Key": "refund-bad-overflow-amount" },
        "body": { "amount": 99999999999999999999999999 }
      }
      """
    Then response status is 400
    And response body contains:
      """json
      {
        "error": "INVALID_REQUEST"
      }
      """
    # additionalProperties: false on CreateRefundRequest: a field this schema
    # never declared is refused rather than silently ignored.
    When POST /api/v1/payments/{paymentId}/refunds:
      """json
      {
        "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc", "Idempotency-Key": "refund-bad-extra-field" },
        "body": { "amount": 1000, "note": "please hurry" }
      }
      """
    Then response status is 400
    And response body contains:
      """json
      {
        "error": "INVALID_REQUEST"
      }
      """
    # `reason` is a closed set (requested_by_customer, duplicate, fraudulent),
    # because it is reported to the merchant verbatim and an open string field
    # would let a caller put anything it likes into that channel.
    When POST /api/v1/payments/{paymentId}/refunds:
      """json
      {
        "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc", "Idempotency-Key": "refund-bad-reason" },
        "body": { "amount": 1000, "reason": "because" }
      }
      """
    Then response status is 422
    And response body contains:
      """json
      {
        "error": "VALIDATION_FAILED",
        "field": "reason"
      }
      """
    And payment provider received 0 refund requests
    And in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM refunds;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments WHERE id = '{paymentId}' AND amount_refunded = 0;
      """

  Scenario: Another merchant cannot refund, or learn anything about, a payment it does not own
    When POST /api/v1/payments/{paymentId}/refunds:
      """json
      {
        "headers": { "Authorization": "Bearer sk_test_globex_9kR2mQ7vX4pL8nT3wZ6yB1cF", "Idempotency-Key": "foreign-refund" },
        "body": { "amount": 100 }
      }
      """
    Then response status is 404
    And response body contains:
      """json
      {
        "error": "PAYMENT_NOT_FOUND"
      }
      """
    And payment provider received 0 refund requests
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments WHERE id = '{paymentId}' AND amount_refunded = 0;
      """

  Scenario: A refund retried with its key is replayed, not repeated
    When POST /api/v1/payments/{paymentId}/refunds:
      """json
      {
        "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc", "Idempotency-Key": "refund-retry" },
        "body": { "amount": 2500 }
      }
      """
    Then response status is 201
    And save response body field "id" as "refundId"
    When POST /api/v1/payments/{paymentId}/refunds:
      """json
      {
        "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc", "Idempotency-Key": "refund-retry" },
        "body": { "amount": 2500 }
      }
      """
    Then response status is 201
    And response header "Idempotent-Replayed" contains "true"
    And response body contains:
      """json
      {
        "id": "{refundId}",
        "amount": 2500
      }
      """
    And payment provider received 1 refund request
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments WHERE id = '{paymentId}' AND amount_refunded = 2500;
      """

  Scenario: A provider that refuses the refund changes nothing here
    When POST /api/v1/payments/{paymentId}/refunds:
      """json
      {
        "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc", "Idempotency-Key": "refund-provider-down" },
        "body": { "amount": 119 }
      }
      """
    Then response status is 502
    And response body contains:
      """json
      {
        "error": "PROVIDER_UNAVAILABLE"
      }
      """
    And payment provider received 1 refund request
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments WHERE id = '{paymentId}' AND amount_refunded = 0;
      """
    And in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM refunds WHERE status = 'succeeded';
      """
    # The key was not spent on an answer nobody got, so the merchant may retry
    # the same request — and this time the provider is asked for 1000, which it
    # will answer.
    When POST /api/v1/payments/{paymentId}/refunds:
      """json
      {
        "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc", "Idempotency-Key": "refund-provider-down" },
        "body": { "amount": 1000 }
      }
      """
    Then response status is 201
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments WHERE id = '{paymentId}' AND amount_refunded = 1000;
      """

  Scenario: A refund whose answer was lost is safe to send again
    # The provider took the request, refunded the money, and never answered.
    # paygate cannot know that, so it must not guess — and it must not double
    # the refund when the merchant retries. What makes the retry safe is that
    # the refund's own id is the number the provider deduplicates on.
    When POST /api/v1/payments/{paymentId}/refunds:
      """json
      {
        "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc", "Idempotency-Key": "refund-lost-answer" },
        "body": { "amount": 408 }
      }
      """
    Then response status is 502
    And response body contains:
      """json
      {
        "error": "PROVIDER_UNAVAILABLE"
      }
      """
    # Nothing is claimed here that is not known: the refund is on the books as
    # pending and the total is untouched, which is the honest state.
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM refunds
       WHERE payment_id = '{paymentId}' AND amount = 408 AND status = 'pending' HAVING count(*) = 1;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments WHERE id = '{paymentId}' AND amount_refunded = 0;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payment_events
       WHERE payment_id = '{paymentId}' AND event_type = 'ProviderCallTimedOut' HAVING count(*) = 1;
      """
    And in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM notifications;
      """
    # The merchant retries the same request. paygate sends the same refund id,
    # the provider recognises it, and the customer is refunded exactly once.
    When POST /api/v1/payments/{paymentId}/refunds:
      """json
      {
        "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc", "Idempotency-Key": "refund-lost-answer" },
        "body": { "amount": 408 }
      }
      """
    Then response status is 201
    And payment provider received 2 refund requests
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM refunds
       WHERE payment_id = '{paymentId}' AND status = 'succeeded' HAVING count(*) = 1;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments WHERE id = '{paymentId}' AND amount_refunded = 408;
      """
    When background work has settled
    Then merchant received 1 notification at "/demo-merchant/api/notify"

  Scenario Outline: Four refunds racing for one payment never refund more than was charged
    # Four DIFFERENT keys, so idempotency has nothing to say here. What stands
    # between 4 × 4000 and a 10000 charge is the row lock: each one reads the
    # total the previous one left behind.
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
        { "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc", "Idempotency-Key": "race-refund-<run>-a" }, "body": { "amount": 4000 } },
        { "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc", "Idempotency-Key": "race-refund-<run>-b" }, "body": { "amount": 4000 } },
        { "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc", "Idempotency-Key": "race-refund-<run>-c" }, "body": { "amount": 4000 } },
        { "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc", "Idempotency-Key": "race-refund-<run>-d" }, "body": { "amount": 4000 } }
      ]
      """
    Then exactly 2 responses are 201
    And exactly 2 responses are 422
    And payment provider received 2 refund requests
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM refunds WHERE payment_id = '{paymentId}' HAVING count(*) = 2 AND sum(amount) = 8000;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments WHERE id = '{paymentId}' AND amount_refunded = 8000;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payment_events
       WHERE payment_id = '{paymentId}' AND event_type = 'PaymentRefunded'
      HAVING count(*) = 2;
      """

    Examples: three runs
      | run |
      | 1   |
      | 2   |
      | 3   |
