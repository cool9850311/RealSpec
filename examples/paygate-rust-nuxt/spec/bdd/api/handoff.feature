@handoff
Feature: The form that sends a customer to a provider

  `POST /payments` answers with the order **and the form**: the provider's URL,
  and every parameter it expects, signed. The merchant's own page renders that
  form and submits it, the customer lands at the provider, and paygate hears
  nothing more until the callback.

  That is one call, and one is the minimum rather than a convenience. ECPay's own
  WooCommerce plugin makes **zero** calls before the customer arrives — its
  `receipt_page()` builds and signs the form locally with the SDK and echoes it —
  because that merchant holds its own `HashKey`. Under a platform the key is
  paygate's, so signing is the one thing a merchant cannot do for itself, and it
  is the only reason this call exists. There is no page of paygate's in the
  browser's path, no checkout token, and no second call: anything more would be
  invented.

  Producing the form opens an **attempt** with a `provider_trade_no` that has
  never been used at that provider, and asking again opens another, because ECPay
  refuses an order number it has seen before whatever became of it. The plugin
  mints a fresh `MerchantTradeNo` every time its payment page renders, and so
  does this. The price is that a customer really can pay twice, and
  `duplicate_payment.feature` is where that is caught and refunded.

  Background:
    Given run migration
    And in PostgreSQL:
      """sql
      INSERT INTO merchants (id, name, currency, timezone, provider_code, provider_merchant_id, rate_limit_per_minute, hash_key, hash_iv) VALUES
        (1, 'Acme Coffee', 'USD', 'Asia/Taipei', 'ecpay',    '2000132',     600, 'acmehashkey0123456789abcdef01234', 'acmehashiv012345'),
        (2, 'Globex',      'EUR', 'UTC',         'newebpay', 'MS150086913', 600, 'globexhashkey0123456789abcdef012', 'globexhashiv0123');
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

  Scenario: The order comes back with a form for the provider, signed by the platform
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "handoff-1"
        },
        "body": {
          "merchant_trade_no": "ACME-HANDOFF-1",
          "amount": 1250,
          "currency": "USD",
          "item_desc": "Single origin beans",
          "notify_url": "/demo-merchant/api/notify",
          "client_back_url": "/shop/result"
        }
      }
      """
    Then response status is 201
    And save response body field "id" as "paymentId"
    And response body contains:
      """json
      {
        "status": "pending",
        "action": "<non-null>",
        "fields": {
          "MerchantID": "2000132",
          "PlatformID": "3002607",
          "MerchantTradeNo": "<non-null>",
          "MerchantTradeDate": "<non-null>",
          "PaymentType": "aio",
          "TotalAmount": "1250",
          "TradeDesc": "<non-null>",
          "ItemName": "Single origin beans",
          "ChoosePayment": "Credit",
          "EncryptType": "1",
          "NeedExtraPaidInfo": "Y",
          "ReturnURL": "<non-null>",
          "ClientBackURL": "/shop/result",
          "CheckMacValue": "<non-null>"
        }
      }
      """
    # `MerchantID` is Acme's own account at ECPay and `PlatformID` is paygate's;
    # the signature is made with the PLATFORM's key pair. That is ECPay's platform
    # rule, and it is why the money is settled into Acme's account and never into
    # paygate's (spec.md, "Platform, not merchant of record").
    #
    # `ClientBackURL` is the merchant's own page, straight from the order: the
    # customer goes back to the shop, not to paygate. `OrderResultURL`, which
    # would POST a result into the browser, is not registered at all.
    And response body does not contain "OrderResultURL"
    # The merchant may hold a signature. It may not hold the key that made one,
    # its own callback signing key, or anybody else's credentials.
    And response body does not contain "pwFHCqoQZGmho4w6"
    And response body does not contain "EkRm7iFT261dpevs"
    And response body does not contain "acmehashkey"
    And response body does not contain "sk_test"
    # One attempt, opened by this call, and nobody has been asked for money yet.
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payment_attempts
       WHERE payment_id = '{paymentId}'
         AND provider_code = 'ecpay'
         AND status = 'redirected'
         AND provider_trade_no ~ '^[A-Za-z0-9]{1,20}$'
         AND provider_trade_no LIKE '%SN%'
      HAVING count(*) = 1;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payment_events
       WHERE payment_id = '{paymentId}'
         AND seq = 2
         AND event_type = 'PaymentAttemptStarted'
         AND payload->>'provider_code' = 'ecpay';
      """
    # `pending` is the honest state: a form exists, and whether anybody used it is
    # not something paygate can know until the provider says so.
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments WHERE id = '{paymentId}' AND status = 'pending';
      """
    And payment provider received 0 checkout requests

  Scenario: Asking again for an unpaid order gives another form, at another number
    # The customer went away, or came back, or the merchant's page was rendered
    # twice. ECPay would refuse the first number a second time, so there is nothing
    # to reuse.
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "again-first"
        },
        "body": {
          "merchant_trade_no": "ACME-AGAIN-1",
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
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "again-second"
        },
        "body": {
          "merchant_trade_no": "ACME-AGAIN-1",
          "amount": 1000,
          "currency": "USD",
          "item_desc": "Beans",
          "notify_url": "/demo-merchant/api/notify",
          "client_back_url": "/shop/result"
        }
      }
      """
    Then response status is 201
    # The same order — not a second one.
    And response body contains:
      """json
      {
        "id": "{paymentId}",
        "merchant_trade_no": "ACME-AGAIN-1",
        "status": "pending"
      }
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments HAVING count(*) = 1;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payment_events WHERE event_type = 'PaymentCreated' HAVING count(*) = 1;
      """
    # Two forms, two numbers, two attempts.
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payment_attempts
       WHERE payment_id = '{paymentId}'
      HAVING count(*) = 2 AND count(DISTINCT provider_trade_no) = 2;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payment_events
       WHERE payment_id = '{paymentId}' AND event_type = 'PaymentAttemptStarted'
      HAVING count(*) = 2;
      """

  Scenario: A retry of the same request replays the stored answer and opens nothing
    # The difference that matters. The same `Idempotency-Key` means "you may not
    # have got my answer", and the answer includes a form that is already in
    # somebody's browser — so it is replayed, not re-minted.
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "replay-handoff"
        },
        "body": {
          "merchant_trade_no": "ACME-REPLAY-1",
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
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "replay-handoff"
        },
        "body": {
          "merchant_trade_no": "ACME-REPLAY-1",
          "amount": 1000,
          "currency": "USD",
          "item_desc": "Beans",
          "notify_url": "/demo-merchant/api/notify",
          "client_back_url": "/shop/result"
        }
      }
      """
    Then response status is 201
    And response header "Idempotent-Replayed" contains "true"
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payment_attempts WHERE payment_id = '{paymentId}' HAVING count(*) = 1;
      """

  Scenario Outline: Six servers asking for one order at one instant get six numbers and one order
    # A merchant's own retry loop, or six workers off one queue message. Every
    # number must be different and none of them may be an error.
    When these things happen at one instant:
      """json
      [
        {"method": "GET", "path": "/api/v1/health/ready"},
        {"method": "GET", "path": "/api/v1/health/ready"},
        {"method": "GET", "path": "/api/v1/health/ready"},
        {"method": "GET", "path": "/api/v1/health/ready"},
        {"method": "GET", "path": "/api/v1/health/ready"},
        {"method": "GET", "path": "/api/v1/health/ready"}
      ]
      """
    Then exactly 6 responses are 200
    When these things happen at one instant:
      """json
      [
        {
          "method": "POST",
          "path": "/api/v1/payments",
          "headers": {
            "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
            "Idempotency-Key": "burst-<run>-1"
          },
          "body": {
            "merchant_trade_no": "ACME-BURST-<run>",
            "amount": 1000,
            "currency": "USD",
            "item_desc": "Beans",
            "notify_url": "/demo-merchant/api/notify",
            "client_back_url": "/shop/result"
          }
        },
        {
          "method": "POST",
          "path": "/api/v1/payments",
          "headers": {
            "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
            "Idempotency-Key": "burst-<run>-2"
          },
          "body": {
            "merchant_trade_no": "ACME-BURST-<run>",
            "amount": 1000,
            "currency": "USD",
            "item_desc": "Beans",
            "notify_url": "/demo-merchant/api/notify",
            "client_back_url": "/shop/result"
          }
        },
        {
          "method": "POST",
          "path": "/api/v1/payments",
          "headers": {
            "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
            "Idempotency-Key": "burst-<run>-3"
          },
          "body": {
            "merchant_trade_no": "ACME-BURST-<run>",
            "amount": 1000,
            "currency": "USD",
            "item_desc": "Beans",
            "notify_url": "/demo-merchant/api/notify",
            "client_back_url": "/shop/result"
          }
        },
        {
          "method": "POST",
          "path": "/api/v1/payments",
          "headers": {
            "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
            "Idempotency-Key": "burst-<run>-4"
          },
          "body": {
            "merchant_trade_no": "ACME-BURST-<run>",
            "amount": 1000,
            "currency": "USD",
            "item_desc": "Beans",
            "notify_url": "/demo-merchant/api/notify",
            "client_back_url": "/shop/result"
          }
        },
        {
          "method": "POST",
          "path": "/api/v1/payments",
          "headers": {
            "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
            "Idempotency-Key": "burst-<run>-5"
          },
          "body": {
            "merchant_trade_no": "ACME-BURST-<run>",
            "amount": 1000,
            "currency": "USD",
            "item_desc": "Beans",
            "notify_url": "/demo-merchant/api/notify",
            "client_back_url": "/shop/result"
          }
        },
        {
          "method": "POST",
          "path": "/api/v1/payments",
          "headers": {
            "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
            "Idempotency-Key": "burst-<run>-6"
          },
          "body": {
            "merchant_trade_no": "ACME-BURST-<run>",
            "amount": 1000,
            "currency": "USD",
            "item_desc": "Beans",
            "notify_url": "/demo-merchant/api/notify",
            "client_back_url": "/shop/result"
          }
        }
      ]
      """
    Then exactly 6 responses are 201
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments HAVING count(*) = 1;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payment_attempts
      HAVING count(*) = 6 AND count(DISTINCT provider_trade_no) = 6;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payment_events WHERE event_type = 'PaymentCreated' HAVING count(*) = 1;
      """

    Examples: three runs
      | run |
      | 1   |
      | 2   |
      | 3   |

  Scenario: The form is submitted, the card is typed at the provider, and the callback settles it
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "pay-1"
        },
        "body": {
          "merchant_trade_no": "ACME-PAY-1",
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
    And payment provider received 1 checkout request
    And the customer pays at the payment provider:
      """json
      {
        "card_number": "4242424242424242",
        "card_expiry": "12/30",
        "card_cvc": "123"
      }
      """
    Then response status is 303
    # The card reached the provider and stopped there. Nothing of it is here, and
    # the order is still not settled: the issuer has not answered yet.
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments WHERE id = '{paymentId}' AND status = 'pending';
      """
    And in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM payment_attempts a WHERE a::text LIKE '%424242424242%';
      """
    And in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM notifications;
      """
    When payment provider delivers each pending callback 1 time
    Then exactly 1 response is 200
    And exactly 1 callback was acknowledged
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments
       WHERE id = '{paymentId}'
         AND status = 'succeeded'
         AND card_brand = 'visa'
         AND card_last4 = '4242';
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payment_attempts
       WHERE payment_id = '{paymentId}' AND status = 'succeeded' AND settled_at IS NOT NULL;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payment_events
       WHERE payment_id = '{paymentId}' AND seq = 3 AND event_type = 'PaymentSucceeded';
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM notifications WHERE payment_id = '{paymentId}' HAVING count(*) = 1;
      """

  Scenario: A form whose amount was changed after paygate signed it buys nothing
    # The form passes through the merchant's page and the customer's browser, so
    # this is the attack the signature exists for — and the PROVIDER is what
    # refuses it, which is the only assertion that proves paygate signed the
    # amount rather than merely signing something.
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "tamper-1"
        },
        "body": {
          "merchant_trade_no": "ACME-TAMPER-1",
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
    When the payment form is submitted to the payment provider, tampered after signing
    Then response status is 400
    And payment provider received 1 checkout request
    # No cashier, so no card, so no callback: the order is exactly as it was.
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payment_attempts
       WHERE payment_id = '{paymentId}' AND status = 'redirected' HAVING count(*) = 1;
      """
    And in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM payment_events WHERE seq > 2;
      """
    And in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM notifications;
      """

  Scenario Outline: A declined attempt leaves the order payable, and the next form gets a new number
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "decline-<failureCode>"
        },
        "body": {
          "merchant_trade_no": "ACME-DECLINE-1",
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
        "card_number": "<cardNumber>",
        "card_expiry": "12/30",
        "card_cvc": "123"
      }
      """
    Then response status is 303
    When payment provider delivers each pending callback 1 time
    Then exactly 1 callback was acknowledged
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments WHERE id = '{paymentId}' AND status = 'pending';
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payment_attempts
       WHERE payment_id = '{paymentId}'
         AND status = 'failed'
         AND failure_code = '<failureCode>'
      HAVING count(*) = 1;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payment_events
       WHERE payment_id = '{paymentId}'
         AND event_type = 'PaymentAttemptFailed'
         AND payload->>'failure_code' = '<failureCode>'
      HAVING count(*) = 1;
      """
    # A failed attempt is not an outcome: the merchant is told nothing.
    And in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM notifications;
      """
    # And the customer may try again, at a number the provider has never seen.
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "decline-<failureCode>-retry"
        },
        "body": {
          "merchant_trade_no": "ACME-DECLINE-1",
          "amount": 1250,
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
      SELECT 1 FROM payment_attempts
       WHERE payment_id = '{paymentId}'
      HAVING count(*) = 2 AND count(DISTINCT provider_trade_no) = 2;
      """
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
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments WHERE id = '{paymentId}' AND status = 'succeeded';
      """
    And payment provider received 2 checkout requests

    Examples: what a provider can refuse
      | cardNumber       | failureCode        |
      | 4000000000000002 | card_declined      |
      | 4000000000009995 | insufficient_funds |
      | 4000000000003063 | three_ds_failed    |

  Scenario: A merchant routed to the other provider gets the other shape
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_globex_9kR2mQ7vX4pL8nT3wZ6yB1cF",
          "Idempotency-Key": "globex-handoff"
        },
        "body": {
          "merchant_trade_no": "GLOBEX-1",
          "amount": 2000,
          "currency": "EUR",
          "item_desc": "Widget",
          "notify_url": "/demo-merchant/api/notify",
          "client_back_url": "/shop/result"
        }
      }
      """
    Then response status is 201
    And save response body field "id" as "globexId"
    And response body contains:
      """json
      {
        "fields": {
          "MerchantID": "MS150086913",
          "TradeInfo": "<non-null>",
          "TradeSha": "<non-null>",
          "Version": "2.0"
        }
      }
      """
    # NewebPay carries the parameters encrypted, so the amount is not in the clear
    # — and the shop's own NewebPay account is still the one that gets paid.
    And response body does not contain "TotalAmount"
    And response body does not contain "Fs5cX1TGqYZ3kLmN8pQrS2tUvW4xY6zA"
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payment_attempts
       WHERE payment_id = '{globexId}' AND provider_code = 'newebpay' AND status = 'redirected';
      """
    # And it is a real form for a real counterparty: the mock verifies TradeSha.
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
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments WHERE id = '{globexId}' AND status = 'succeeded';
      """

  Scenario: A paid order is not handed over again
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "paid-1"
        },
        "body": {
          "merchant_trade_no": "ACME-PAID-1",
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
    Then exactly 1 callback was acknowledged
    # The order number is spent now, at paygate and at ECPay alike.
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "paid-1-again"
        },
        "body": {
          "merchant_trade_no": "ACME-PAID-1",
          "amount": 1250,
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
      SELECT 1 FROM payment_attempts WHERE payment_id = '{paymentId}' HAVING count(*) = 1;
      """
    And payment provider received 1 checkout request

  Scenario: An item description full of characters a signer can trip over still verifies
    # ECPay tells integrators to keep special characters out of TradeDesc, and
    # merchants do it anyway: accents, an emoji, an ampersand that would break a
    # signer that joins parameters with "&", a percent sign that looks like it
    # wants decoding. The CheckMacValue algorithm's URL-encode-then-lowercase
    # step (spec.md, "Authentication") has to survive all of it undisturbed, and
    # the only proof that matters is the one the algorithm itself can't fake:
    # the PROVIDER, recomputing the same digest, accepting the form.
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "handoff-unicode-1"
        },
        "body": {
          "merchant_trade_no": "ACME-UNICODE-1",
          "amount": 1250,
          "currency": "USD",
          "item_desc": "Cafe creme (Ünïcode) & 100% Arabica ☕",
          "notify_url": "/demo-merchant/api/notify",
          "client_back_url": "/shop/result"
        }
      }
      """
    Then response status is 201
    And save response body field "id" as "paymentId"
    And response body contains:
      """json
      {
        "item_desc": "Cafe creme (Ünïcode) & 100% Arabica ☕",
        "fields": {
          "ItemName": "Cafe creme (Ünïcode) & 100% Arabica ☕"
        }
      }
      """
    When the payment form is submitted to the payment provider
    Then response status is 200
    And payment provider received 1 checkout request
