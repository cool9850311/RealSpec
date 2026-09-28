@notify
Feature: Telling the merchant

  The merchant's server is not in the path the card travels, so the only way it
  learns that an order was paid is this callback — modelled on ECPay's ReturnURL:
  a signed form POST to the `notify_url` the order named, which the merchant
  must answer with the exact body `1|OK`. Anything else, including a `200` with
  another body, is a failed delivery and is retried with backoff up to
  `NOTIFY_MAX_ATTEMPTS`.

  It is the same contract paygate is held to from above, in the other
  direction: a provider tells paygate out of band and demands `1|OK`
  (webhooks.feature), and paygate tells its merchants out of band and demands
  `1|OK` here. One implementation of the rule, written twice from opposite
  seats, which is the clearest way to see that it is a rule and not a quirk.

  A notification row is written in the SAME transaction as the outcome it
  reports, so a paid order can never end up with nobody owing the merchant a
  callback. The notifier is a background worker; every assertion about delivery
  therefore comes after `background work has settled`.

  The merchant's behaviour is stated the way the provider's is — by the URL the
  order was created with:

    /demo-merchant/api/notify          verifies nothing, answers `1|OK`
    /demo-merchant/api/notify-strict   verifies the signature, then `1|OK`
    /demo-merchant/api/notify-flaky    fails twice, then answers `1|OK`
    /demo-merchant/api/notify-reject   always answers `500`
    /demo-merchant/api/notify-mumble   answers `200` with the body `0|ERROR`
    /demo-merchant/api/notify-loose    answers `200` with `1|OK` and a newline
    /demo-merchant/api/notify-slow     answers `1|OK`, too late to count

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

  Scenario: A paid order is reported once, and the merchant's 1|OK ends it
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "key-acme-n-1"
        },
        "body": {
          "merchant_trade_no": "ACME-N-1",
          "amount": 1250,
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
    When background work has settled
    Then merchant received 1 notification at "/demo-merchant/api/notify"
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM notifications
       WHERE payment_id = '{paymentId}'
         AND attempts = 1
         AND last_status = 200
         AND delivered_at IS NOT NULL;
      """
    # What the merchant is sent: ECPay's shape, and a signature over it.
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM notifications
       WHERE payment_id = '{paymentId}'
         AND payload->>'MerchantTradeNo' = 'ACME-N-1'
         AND payload->>'TradeNo' = '{paymentId}'
         AND (payload->>'RtnCode')::int = 1
         AND (payload->>'TradeAmt')::bigint = 1250
         AND payload->>'PaymentDate' IS NOT NULL
         AND payload->>'CheckMacValue' IS NOT NULL;
      """

  Scenario: The signature is one the merchant can verify
    # The strict endpoint recomputes CheckMacValue from the merchant's hash key
    # and refuses anything that does not match, so a delivery it accepted is
    # proof that the signature was right.
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "key-acme-n-strict"
        },
        "body": {
          "merchant_trade_no": "ACME-N-STRICT",
          "amount": 800,
          "currency": "USD",
          "item_desc": "Beans",
          "notify_url": "/demo-merchant/api/notify-strict",
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
    When background work has settled
    Then merchant received 1 notification at "/demo-merchant/api/notify-strict"
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM notifications WHERE payment_id = '{paymentId}' AND delivered_at IS NOT NULL;
      """

  Scenario: A merchant that fails twice is retried, and the third delivery ends it
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "key-acme-n-flaky"
        },
        "body": {
          "merchant_trade_no": "ACME-N-FLAKY",
          "amount": 900,
          "currency": "USD",
          "item_desc": "Beans",
          "notify_url": "/demo-merchant/api/notify-flaky",
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
    When background work has settled
    Then merchant received 3 notifications at "/demo-merchant/api/notify-flaky"
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM notifications
       WHERE payment_id = '{paymentId}'
         AND attempts = 3
         AND last_status = 200
         AND delivered_at IS NOT NULL;
      """

  Scenario: A merchant that never answers is retried to the limit and then left alone
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "key-acme-n-reject"
        },
        "body": {
          "merchant_trade_no": "ACME-N-REJECT",
          "amount": 700,
          "currency": "USD",
          "item_desc": "Beans",
          "notify_url": "/demo-merchant/api/notify-reject",
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
    When background work has settled
    Then merchant received 5 notifications at "/demo-merchant/api/notify-reject"
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM notifications
       WHERE payment_id = '{paymentId}'
         AND attempts = 5
         AND last_status = 500
         AND delivered_at IS NULL
         AND exhausted_at IS NOT NULL;
      """
    # The money is not in doubt — only the merchant's knowledge of it is.
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments WHERE id = '{paymentId}' AND status = 'succeeded';
      """

  Scenario: A 200 that does not say 1|OK is not a delivery
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "key-acme-n-mumble"
        },
        "body": {
          "merchant_trade_no": "ACME-N-MUMBLE",
          "amount": 600,
          "currency": "USD",
          "item_desc": "Beans",
          "notify_url": "/demo-merchant/api/notify-mumble",
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
    When background work has settled
    Then merchant received 5 notifications at "/demo-merchant/api/notify-mumble"
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM notifications
       WHERE payment_id = '{paymentId}'
         AND attempts = 5
         AND last_status = 200
         AND delivered_at IS NULL;
      """

  Scenario: `1|OK` with a newline after it is not `1|OK`
    # The trap that makes the exact comparison worth stating. ECPay specifies the
    # body, and its own plugin writes it with `echo '1|OK'; exit;` and compares
    # what it receives exactly. A gateway that trims, lowercases or prefix-matches
    # accepts merchants who never acknowledged anything, and finds out in a
    # reconciliation months later.
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "key-acme-n-loose"
        },
        "body": {
          "merchant_trade_no": "ACME-N-LOOSE",
          "amount": 650,
          "currency": "USD",
          "item_desc": "Beans",
          "notify_url": "/demo-merchant/api/notify-loose",
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
    Then exactly 1 callback was acknowledged
    When background work has settled
    Then merchant received 5 notifications at "/demo-merchant/api/notify-loose"
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM notifications
       WHERE payment_id = '{paymentId}'
         AND attempts = 5
         AND last_status = 200
         AND delivered_at IS NULL
         AND exhausted_at IS NOT NULL;
      """
    # The payment is untouched by any of it: only the merchant's knowledge is.
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments WHERE id = '{paymentId}' AND status = 'succeeded';
      """

  Scenario: An answer that arrives after the notifier gave up waiting is not an answer
    # `/notify-slow` eventually says exactly the right thing. A notifier that
    # waited for it would be held hostage by one slow merchant and would stop
    # delivering to everybody else; one that counted a timeout as delivered would
    # lose the order quietly. So it is a failed delivery and it is retried.
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "key-acme-n-slow"
        },
        "body": {
          "merchant_trade_no": "ACME-N-SLOW",
          "amount": 700,
          "currency": "USD",
          "item_desc": "Beans",
          "notify_url": "/demo-merchant/api/notify-slow",
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
    Then exactly 1 callback was acknowledged
    When background work has settled
    Then merchant received 5 notifications at "/demo-merchant/api/notify-slow"
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM notifications
       WHERE payment_id = '{paymentId}'
         AND attempts = 5
         AND delivered_at IS NULL
         AND exhausted_at IS NOT NULL;
      """
    # Giving up on one merchant does not slow down the next: the queue moved on.
    And in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM notifications WHERE delivered_at IS NULL AND exhausted_at IS NULL;
      """

  Scenario: An order settled by the provider's webhook is reported when it settles, not before
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "key-acme-n-async"
        },
        "body": {
          "merchant_trade_no": "ACME-N-ASYNC",
          "amount": 4000,
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
    When background work has settled
    Then merchant received 0 notifications at "/demo-merchant/api/notify"
    And in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM notifications;
      """
    When payment provider delivers each pending callback 1 time
    Then exactly 1 response is 200
    When background work has settled
    Then merchant received 1 notification at "/demo-merchant/api/notify"
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM notifications WHERE payment_id = '{paymentId}' AND delivered_at IS NOT NULL;
      """

  Scenario: Payments keep working while the notifier is down, and the merchant hears about them once it is back
    Given service "notifier" is stopped
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "key-acme-n-down"
        },
        "body": {
          "merchant_trade_no": "ACME-N-DOWN",
          "amount": 500,
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
      SELECT 1 FROM notifications
       WHERE payment_id = '{paymentId}' AND attempts = 0 AND delivered_at IS NULL;
      """
    And merchant received 0 notifications at "/demo-merchant/api/notify"
    When service "notifier" is started
    And background work has settled
    Then merchant received 1 notification at "/demo-merchant/api/notify"
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM notifications WHERE payment_id = '{paymentId}' AND delivered_at IS NOT NULL;
      """

  Scenario: A refund is reported to the merchant too
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "key-acme-n-refund"
        },
        "body": {
          "merchant_trade_no": "ACME-N-REFUND",
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
    When POST /api/v1/payments/{paymentId}/refunds:
      """json
      {
        "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc", "Idempotency-Key": "n-refund-1" },
        "body": { "amount": 4000 }
      }
      """
    Then response status is 201
    When background work has settled
    Then merchant received 2 notifications at "/demo-merchant/api/notify"
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM notifications
       WHERE payment_id = '{paymentId}'
         AND payload->>'RtnMsg' = 'refunded'
         AND (payload->>'TradeAmt')::bigint = 4000
         AND delivered_at IS NOT NULL;
      """

  Scenario: A payment refunded before its own success notification is delivered still owes both, in order
    # The notifier claims the oldest due row first; it does not know or care why
    # a row became due. Stopping it here lets a refund become due while the
    # success notification is still sitting unclaimed — exactly what happens
    # when a merchant is refunded moments after paying — and proves the two
    # still leave in the order they were written, not the order they raced in.
    Given service "notifier" is stopped
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "key-acme-n-race"
        },
        "body": {
          "merchant_trade_no": "ACME-N-RACE",
          "amount": 800,
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
      SELECT 1 FROM notifications
       WHERE payment_id = '{paymentId}' AND attempts = 0 AND delivered_at IS NULL;
      """
    When POST /api/v1/payments/{paymentId}/refunds:
      """json
      {
        "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc", "Idempotency-Key": "n-race-refund" },
        "body": { "amount": 800 }
      }
      """
    Then response status is 201
    And merchant received 0 notifications at "/demo-merchant/api/notify"
    When service "notifier" is started
    And background work has settled
    Then merchant received 2 notifications at "/demo-merchant/api/notify"
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1
        FROM notifications a JOIN notifications b ON a.payment_id = b.payment_id
       WHERE a.payment_id = '{paymentId}'
         AND a.payload->>'RtnMsg' = 'paid'
         AND b.payload->>'RtnMsg' = 'refunded'
         AND a.delivered_at IS NOT NULL
         AND b.delivered_at IS NOT NULL
         AND a.delivered_at <= b.delivered_at;
      """

  Scenario: Two notifications owed to one payment exhaust independently
    # PaymentSucceeded and PaymentRefunded each write their own notification
    # row (spec.md, "Telling the merchant"), so a merchant that never answers
    # burns through NOTIFY_MAX_ATTEMPTS twice — once per outcome it is owed —
    # not once for the payment as a whole.
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "key-acme-n-reject-both"
        },
        "body": {
          "merchant_trade_no": "ACME-N-REJECT-BOTH",
          "amount": 1200,
          "currency": "USD",
          "item_desc": "Beans",
          "notify_url": "/demo-merchant/api/notify-reject",
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
    When POST /api/v1/payments/{paymentId}/refunds:
      """json
      {
        "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc", "Idempotency-Key": "n-reject-both-refund" },
        "body": { "amount": 1200 }
      }
      """
    Then response status is 201
    When background work has settled
    Then merchant received 10 notifications at "/demo-merchant/api/notify-reject"
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM notifications
       WHERE payment_id = '{paymentId}' AND attempts = 5 AND exhausted_at IS NOT NULL AND delivered_at IS NULL
      HAVING count(*) = 2;
      """
    # Exhaustion is not a failure of the payment: the refund moved the money
    # whether or not the merchant ever heard about either outcome.
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments WHERE id = '{paymentId}' AND status = 'refunded';
      """

  Scenario: An exhausted notification is not retried again; the query endpoint is how the merchant recovers
    # There is no reconciler for exhausted notifications (spec.md, Non-goals) —
    # the merchant's own GET is the recovery path. Running background work has
    # settled a second time, with nothing new due, is what makes "no automatic
    # resend" a checked fact instead of an absence of evidence.
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "key-acme-n-stop"
        },
        "body": {
          "merchant_trade_no": "ACME-N-STOP",
          "amount": 450,
          "currency": "USD",
          "item_desc": "Beans",
          "notify_url": "/demo-merchant/api/notify-reject",
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
    When background work has settled
    Then merchant received 5 notifications at "/demo-merchant/api/notify-reject"
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM notifications
       WHERE payment_id = '{paymentId}'
         AND attempts = 5
         AND exhausted_at IS NOT NULL
         AND delivered_at IS NULL;
      """
    When background work has settled
    Then merchant received 5 notifications at "/demo-merchant/api/notify-reject"
    When GET /api/v1/payments/{paymentId}:
      """json
      {
        "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc" }
      }
      """
    Then response status is 200
    And response body contains:
      """json
      {
        "id": "{paymentId}",
        "status": "succeeded"
      }
      """
