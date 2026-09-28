@orders
Feature: Creating an order

  The merchant's server creates the order, and that same response already
  carries the provider's form: an amount, the merchant's own order number, the
  two URLs paygate will need later, and somewhere to send the customer. No
  card data, and no provider called — producing the form is signing, not a
  request. What that form looks like, how it is signed, and what asking again
  for it does is handoff.feature's question; this file asserts the order.

  It cannot carry card data, and the last two scenarios say so twice over: a
  body containing a card field is refused rather than ignored, and no column in
  this schema could hold one anyway. That is the property the whole redirect
  exists for (spec.md, "paygate is not on the payment path").

  `merchant_trade_no` is the merchant's own order number and is unique per
  merchant, exactly as ECPay's `MerchantTradeNo` is — one level down, paygate
  plays the part ECPay plays for it. It is the business-level guard against
  creating one order twice; `Idempotency-Key` is the transport one, and the two
  answer different questions (spec.md, "Two kinds of duplicate").

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

  Scenario: An order is created as pending, with somewhere to send the customer
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "create-1001"
        },
        "body": {
          "merchant_trade_no": "ACME20260921001",
          "amount": 1250,
          "currency": "USD",
          "item_desc": "Single origin beans",
          "notify_url": "/demo-merchant/api/notify",
          "client_back_url": "/shop/result"
        }
      }
      """
    Then response status is 201
    And response body contains:
      """json
      {
        "id": "<non-null>",
        "merchant_trade_no": "ACME20260921001",
        "status": "pending",
        "amount": 1250,
        "currency": "USD",
        "item_desc": "Single origin beans",
        "amount_refunded": 0
      }
      """
    And save response body field "id" as "paymentId"
    # Nobody has been asked for money yet: the form exists, and it is the
    # merchant's page that will submit it (handoff.feature).
    And payment provider received 0 checkout requests
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments
       WHERE id = '{paymentId}'
         AND merchant_id = 1
         AND merchant_trade_no = 'ACME20260921001'
         AND status = 'pending'
         AND amount = 1250
         AND amount_refunded = 0
         AND notify_url = '/demo-merchant/api/notify'
         AND client_back_url = '/shop/result'
         AND card_brand IS NULL
         AND card_last4 IS NULL;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payment_events
       WHERE payment_id = '{paymentId}'
         AND seq = 1
         AND merchant_id = 1
         AND event_type = 'PaymentCreated'
         AND (payload->>'amount')::bigint = 1250;
      """
    # And nothing was handed to anybody: no token, no URL, no form. The order
    # exists, and the next thing that happens is the merchant asking for a form
    # when it renders its own payment page (spec.md, "The hand-off").
    And in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM information_schema.columns
       WHERE table_schema = 'public' AND column_name LIKE 'checkout%';
      """

  Scenario: The merchant's own order number cannot be used twice by that merchant, and never blocks another
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "dup-a"
        },
        "body": {
          "merchant_trade_no": "SHARED-0001",
          "amount": 1000,
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
          "Idempotency-Key": "dup-b"
        },
        "body": {
          "merchant_trade_no": "SHARED-0001",
          "amount": 9999,
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
    And in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM payments WHERE amount = 9999;
      """
    # The same number under another merchant is another order entirely.
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_globex_9kR2mQ7vX4pL8nT3wZ6yB1cF",
          "Idempotency-Key": "dup-c"
        },
        "body": {
          "merchant_trade_no": "SHARED-0001",
          "amount": 1000,
          "currency": "EUR",
          "item_desc": "Widget",
          "notify_url": "/demo-merchant/api/notify",
          "client_back_url": "/shop/result"
        }
      }
      """
    Then response status is 201
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments WHERE merchant_trade_no = 'SHARED-0001' HAVING count(*) = 2;
      """

  Scenario: Retrying the same creation with its key replays the first order rather than refusing it
    # The distinction that matters: a RETRY carries the same Idempotency-Key and
    # is replayed; a second order that happens to reuse the trade number is
    # refused. One is a lost response, the other is a mistake.
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "retry-create-1"
        },
        "body": {
          "merchant_trade_no": "ACME-RETRY-1",
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
          "Idempotency-Key": "retry-create-1"
        },
        "body": {
          "merchant_trade_no": "ACME-RETRY-1",
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

  Scenario Outline: A field that breaks a rule is refused, and no order exists afterwards
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "bad-<field>-<tradeNo>"
        },
        "body": {
          "merchant_trade_no": "<tradeNo>",
          "amount": 1000,
          "currency": "<currency>",
          "item_desc": "<itemDesc>",
          "notify_url": "<notifyUrl>",
          "client_back_url": "/shop/result"
        }
      }
      """
    Then response status is 422
    And response body contains:
      """json
      {
        "error": "VALIDATION_FAILED",
        "field": "<field>"
      }
      """
    And in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM payments;
      """
    And in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM payment_attempts;
      """
    And in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM idempotency_keys;
      """

    Examples: every field has a rule
      | field             | tradeNo                          | currency | itemDesc | notifyUrl                 |
      | merchant_trade_no |                                  | USD      | Beans    | /demo-merchant/api/notify |
      | merchant_trade_no | ACME-0001-TOO-LONG-FOR-THE-LIMIT | USD      | Beans    | /demo-merchant/api/notify |
      | merchant_trade_no | ACME/0001                        | USD      | Beans    | /demo-merchant/api/notify |
      | currency          | ACME-0004                        | usd      | Beans    | /demo-merchant/api/notify |
      | item_desc         | ACME-0005                        | USD      |          | /demo-merchant/api/notify |
      | notify_url        | ACME-0006                        | USD      | Beans    | not-a-url                 |

  Scenario: An amount outside 1 to 99,999,999 minor units is refused
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "amount-zero"
        },
        "body": {
          "merchant_trade_no": "ACME-A1",
          "amount": 0,
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
        "error": "VALIDATION_FAILED",
        "field": "amount"
      }
      """
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "amount-fraction"
        },
        "body": {
          "merchant_trade_no": "ACME-A2",
          "amount": 12.5,
          "currency": "USD",
          "item_desc": "Beans",
          "notify_url": "/demo-merchant/api/notify",
          "client_back_url": "/shop/result"
        }
      }
      """
    # A fraction is not an integer out of range — it is not the type `amount`
    # is at all, so it is caught by body shape, before field rules ever run
    # (openapi.yaml, createOrder's order of checks).
    Then response status is 400
    And response body contains:
      """json
      {
        "error": "INVALID_REQUEST"
      }
      """
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "amount-too-large"
        },
        "body": {
          "merchant_trade_no": "ACME-A3",
          "amount": 100000000,
          "currency": "USD",
          "item_desc": "Beans",
          "notify_url": "/demo-merchant/api/notify",
          "client_back_url": "/shop/result"
        }
      }
      """
    Then response status is 422
    And in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM payments;
      """

  Scenario: A currency the merchant does not settle in is refused
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "wrong-currency"
        },
        "body": {
          "merchant_trade_no": "ACME-C1",
          "amount": 1000,
          "currency": "EUR",
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
        "error": "CURRENCY_NOT_SUPPORTED"
      }
      """
    And in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM payments;
      """

  Scenario: Card data sent to paygate is refused rather than accepted quietly
    # Neither the merchant nor paygate may hold a card number. An integration
    # that tries anyway is told at the first attempt, instead of having its
    # mistake silently dropped while it believes the opposite.
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "card-at-create"
        },
        "body": {
          "merchant_trade_no": "ACME-D1",
          "amount": 1000,
          "currency": "USD",
          "item_desc": "Beans",
          "notify_url": "/demo-merchant/api/notify",
          "client_back_url": "/shop/result",
          "card_number": "4242424242424242"
        }
      }
      """
    Then response status is 400
    And response body contains:
      """json
      {
        "error": "INVALID_REQUEST"
      }
      """
    And in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM payments;
      """

  Scenario: There is no column in this schema that could hold a card
    # The strongest form of the claim, and the cheapest to check: not "we do not
    # write one" but "there is nowhere to write one". A migration that adds a
    # place for a PAN fails here, before anyone has to catch it in review.
    Then in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM information_schema.columns
       WHERE table_schema = 'public'
         AND (column_name LIKE '%card_number%'
              OR column_name LIKE '%pan%'
              OR column_name LIKE '%cvc%'
              OR column_name LIKE '%cvv%');
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM information_schema.columns
       WHERE table_schema = 'public'
         AND table_name = 'payment_attempts'
         AND column_name = 'card_last4';
      """

  Scenario: An order is readable by its merchant, by its own id or the merchant's number, and by nobody else
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "read-1"
        },
        "body": {
          "merchant_trade_no": "ACME-READ-1",
          "amount": 1500,
          "currency": "USD",
          "item_desc": "Secret blend",
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
        "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc" }
      }
      """
    Then response status is 200
    And response body contains:
      """json
      {
        "id": "{paymentId}",
        "merchant_trade_no": "ACME-READ-1",
        "status": "pending",
        "amount": 1500
      }
      """
    # Reading an order does not hand anything over. The form is minted by the
    # call that creates or re-creates the order, and by nothing else.
    And response body does not contain "CheckMacValue"
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payment_attempts WHERE payment_id = '{paymentId}' HAVING count(*) = 1;
      """
    When GET /api/v1/payments?merchant_trade_no=ACME-READ-1:
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
        "merchant_trade_no": "ACME-READ-1"
      }
      """
    When GET /api/v1/payments/{paymentId}:
      """json
      {
        "headers": { "Authorization": "Bearer sk_test_globex_9kR2mQ7vX4pL8nT3wZ6yB1cF" }
      }
      """
    Then response status is 404
    And response body contains:
      """json
      {
        "error": "PAYMENT_NOT_FOUND"
      }
      """
    And response body does not contain "Secret blend"

  Scenario: An amount at either edge of 1 to 99,999,999 is accepted, not just refused outside it
    # The scenario above proves the fence keeps the wrong values out; this one
    # proves it does not also keep the right ones out. A boundary check that
    # rejects 0 and 100,000,000 but was written with `>` instead of `>=` would
    # still pass every scenario above and fail exactly these two amounts.
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "amount-min"
        },
        "body": {
          "merchant_trade_no": "ACME-A4",
          "amount": 1,
          "currency": "USD",
          "item_desc": "Beans",
          "notify_url": "/demo-merchant/api/notify",
          "client_back_url": "/shop/result"
        }
      }
      """
    Then response status is 201
    And response body contains:
      """json
      {
        "amount": 1
      }
      """
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "amount-max"
        },
        "body": {
          "merchant_trade_no": "ACME-A5",
          "amount": 99999999,
          "currency": "USD",
          "item_desc": "Beans",
          "notify_url": "/demo-merchant/api/notify",
          "client_back_url": "/shop/result"
        }
      }
      """
    Then response status is 201
    And response body contains:
      """json
      {
        "amount": 99999999
      }
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments WHERE amount IN (1, 99999999) HAVING count(*) = 2;
      """

  Scenario: A body whose amount is not even the right JSON type is malformed, not merely out of range
    # `amount` is an integer field. A string in its place is not a value that
    # breaks a rule — it is a request that does not have the shape this endpoint
    # accepts at all, and the API's own order of checks puts a wrong type before
    # field rules run (openapi.yaml, createOrder: "body shape (400
    # INVALID_REQUEST — ... a wrong type ...), field rules (422
    # VALIDATION_FAILED)"). Answering 422 here would mean the amount was parsed
    # as a number before anybody decided it was one.
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "amount-wrong-type"
        },
        "body": {
          "merchant_trade_no": "ACME-A6",
          "amount": "1000",
          "currency": "USD",
          "item_desc": "Beans",
          "notify_url": "/demo-merchant/api/notify",
          "client_back_url": "/shop/result"
        }
      }
      """
    Then response status is 400
    And response body contains:
      """json
      {
        "error": "INVALID_REQUEST"
      }
      """
    And in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM payments;
      """

  Scenario: A merchant trade number exactly at the twenty-character limit is accepted
    # The Outline above proves twenty-one characters and a slash are refused.
    # Twenty plain alphanumerics is the one length nobody wrote a test for, and
    # it is exactly the value an off-by-one in `{1,20}` would get wrong.
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "tradeno-max-length"
        },
        "body": {
          "merchant_trade_no": "ACME0001234567890123",
          "amount": 1000,
          "currency": "USD",
          "item_desc": "Beans",
          "notify_url": "/demo-merchant/api/notify",
          "client_back_url": "/shop/result"
        }
      }
      """
    Then response status is 201
    And response body contains:
      """json
      {
        "merchant_trade_no": "ACME0001234567890123"
      }
      """

  Scenario: An item description holding a quote and a statement is stored and returned exactly, not built into a query
    # `item_desc` is free text a merchant controls, and free text a merchant
    # controls is exactly what gets typed into a query string by an integration
    # that concatenates instead of parameterising. Storing this value and
    # reading it back byte for byte is the cheapest check that never happened.
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "item-desc-quote"
        },
        "body": {
          "merchant_trade_no": "ACME-INJECT-1",
          "amount": 1000,
          "currency": "USD",
          "item_desc": "Dave's Beans'; DROP TABLE payments; --",
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
        "item_desc": "Dave's Beans'; DROP TABLE payments; --"
      }
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments
       WHERE id = '{paymentId}'
         AND item_desc = 'Dave''s Beans''; DROP TABLE payments; --';
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments HAVING count(*) = 1;
      """

  Scenario: An Idempotency-Key past its 255-character limit is refused, not truncated
    # openapi.yaml bounds Idempotency-Key to 255 characters and documents
    # `IDEMPOTENCY_KEY_INVALID` for exactly this. Nothing else in this file, or
    # in idempotency.feature, sends a key past that limit — so nothing proves
    # today that an oversized key is refused rather than silently truncated to
    # something a later, correctly-sized retry would collide with.
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "0123456789012345678901234567890123456789012345678901234567890123456789012345678901234567890123456789012345678901234567890123456789012345678901234567890123456789012345678901234567890123456789012345678901234567890123456789012345678901234567890123456789012345"
        },
        "body": {
          "merchant_trade_no": "ACME-A7",
          "amount": 1000,
          "currency": "USD",
          "item_desc": "Beans",
          "notify_url": "/demo-merchant/api/notify",
          "client_back_url": "/shop/result"
        }
      }
      """
    Then response status is 400
    And response body contains:
      """json
      {
        "error": "IDEMPOTENCY_KEY_INVALID"
      }
      """
    And in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM payments;
      """
    And in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM idempotency_keys;
      """
