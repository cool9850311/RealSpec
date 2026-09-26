@rate-limits
Feature: Per-merchant rate limits

  Each merchant has a token bucket of `rate_limit_per_minute` tokens that
  refills continuously, held in Redis and spent atomically by one script per
  request — so the limit holds for simultaneous requests and across replicas,
  not only for requests that arrive one after another. A refused request is
  answered 429 with Retry-After, and it is refused BEFORE its idempotency key is
  recorded, so it costs nothing and blocks nothing.

  The limit is on the MERCHANT API — creating orders, querying them, refunding
  them — because that is the surface a merchant's own loop can hammer. Two
  surfaces are deliberately outside it:

    the checkout   a busy shop's customers are not each other's noisy
                   neighbours. A customer who reloads does open another order at
                   the provider, and the answer to that is to refund the second
                   payment, not to refuse the reload (duplicate_payment.feature)
    the callbacks  refusing a provider's callback only makes it arrive again,
                   later, with the payment still unsettled. A rate limit there
                   would be a way to lose money slowly.

  The limit is seeded per scenario as data, before the first request, because
  the merchant row is cached with the API key it belongs to.

  If Redis is unavailable the limiter fails OPEN — see resilience.feature. That
  is a decision: a merchant's customers should not be refused payment because
  an abuse guard is down (spec.md, "Failure model").

  Background:
    Given run migration
    And in PostgreSQL:
      """sql
      INSERT INTO merchants (id, name, currency, timezone, provider_code, provider_merchant_id, rate_limit_per_minute, hash_key, hash_iv) VALUES
        (1, 'Acme Coffee', 'USD', 'Asia/Taipei', 'ecpay',    '2000132',     3,   'acmehashkey0123456789abcdef01234', 'acmehashiv012345'),
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

  Scenario: The request past the limit is refused with Retry-After, and costs nothing
    When POST /api/v1/payments:
      """json
      {
        "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc", "Idempotency-Key": "rl-1" },
        "body": { "merchant_trade_no": "TN-RL-1", "amount": 100, "currency": "USD", "item_desc": "Beans", "notify_url": "/demo-merchant/api/notify", "client_back_url": "/shop/result" }
      }
      """
    Then response status is 201
    When POST /api/v1/payments:
      """json
      {
        "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc", "Idempotency-Key": "rl-2" },
        "body": { "merchant_trade_no": "TN-RL-2", "amount": 100, "currency": "USD", "item_desc": "Beans", "notify_url": "/demo-merchant/api/notify", "client_back_url": "/shop/result" }
      }
      """
    Then response status is 201
    When POST /api/v1/payments:
      """json
      {
        "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc", "Idempotency-Key": "rl-3" },
        "body": { "merchant_trade_no": "TN-RL-3", "amount": 100, "currency": "USD", "item_desc": "Beans", "notify_url": "/demo-merchant/api/notify", "client_back_url": "/shop/result" }
      }
      """
    Then response status is 201
    When POST /api/v1/payments:
      """json
      {
        "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc", "Idempotency-Key": "rl-4" },
        "body": { "merchant_trade_no": "TN-RL-4", "amount": 100, "currency": "USD", "item_desc": "Beans", "notify_url": "/demo-merchant/api/notify", "client_back_url": "/shop/result" }
      }
      """
    Then response status is 429
    And response body contains:
      """json
      {
        "error": "RATE_LIMITED"
      }
      """
    And response header "Retry-After" contains "20"
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments HAVING count(*) = 3;
      """
    And in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM idempotency_keys WHERE idempotency_key = 'rl-4';
      """

  Scenario: A customer at the checkout is not spending the merchant's bucket
    # The merchant has one token left. Its customer opens the checkout, is
    # handed off, comes back and reloads — none of which may cost the merchant
    # anything, or a busy shop would lock its own API out.
    When POST /api/v1/payments:
      """json
      {
        "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc", "Idempotency-Key": "rl-co-1" },
        "body": { "merchant_trade_no": "TN-CO-1", "amount": 100, "currency": "USD", "item_desc": "Beans", "notify_url": "/demo-merchant/api/notify", "client_back_url": "/shop/result" }
      }
      """
    Then response status is 201
    # Two tokens were left, and two are left.
    When POST /api/v1/payments:
      """json
      {
        "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc", "Idempotency-Key": "rl-co-2" },
        "body": { "merchant_trade_no": "TN-CO-2", "amount": 100, "currency": "USD", "item_desc": "Beans", "notify_url": "/demo-merchant/api/notify", "client_back_url": "/shop/result" }
      }
      """
    Then response status is 201
    When POST /api/v1/payments:
      """json
      {
        "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc", "Idempotency-Key": "rl-co-3" },
        "body": { "merchant_trade_no": "TN-CO-3", "amount": 100, "currency": "USD", "item_desc": "Beans", "notify_url": "/demo-merchant/api/notify", "client_back_url": "/shop/result" }
      }
      """
    Then response status is 201

  Scenario: Another merchant's bucket is its own
    When POST /api/v1/payments:
      """json
      {
        "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc", "Idempotency-Key": "own-1" },
        "body": { "merchant_trade_no": "TN-OWN-1", "amount": 100, "currency": "USD", "item_desc": "Beans", "notify_url": "/demo-merchant/api/notify", "client_back_url": "/shop/result" }
      }
      """
    Then response status is 201
    When POST /api/v1/payments:
      """json
      {
        "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc", "Idempotency-Key": "own-2" },
        "body": { "merchant_trade_no": "TN-OWN-2", "amount": 100, "currency": "USD", "item_desc": "Beans", "notify_url": "/demo-merchant/api/notify", "client_back_url": "/shop/result" }
      }
      """
    Then response status is 201
    When POST /api/v1/payments:
      """json
      {
        "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc", "Idempotency-Key": "own-3" },
        "body": { "merchant_trade_no": "TN-OWN-3", "amount": 100, "currency": "USD", "item_desc": "Beans", "notify_url": "/demo-merchant/api/notify", "client_back_url": "/shop/result" }
      }
      """
    Then response status is 201
    # Acme's bucket is empty. Globex's is not, and the same order number under
    # another merchant is another order.
    When POST /api/v1/payments:
      """json
      {
        "headers": { "Authorization": "Bearer sk_test_globex_9kR2mQ7vX4pL8nT3wZ6yB1cF", "Idempotency-Key": "own-1" },
        "body": { "merchant_trade_no": "TN-OWN-1", "amount": 100, "currency": "EUR", "item_desc": "Beans", "notify_url": "/demo-merchant/api/notify", "client_back_url": "/shop/result" }
      }
      """
    Then response status is 201
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments HAVING count(*) = 4;
      """

  Scenario Outline: Six simultaneous requests against a bucket of three let exactly three through
    When GET /api/v1/health/ready is called concurrently:
      """json
      [
        { "body": {} },
        { "body": {} },
        { "body": {} },
        { "body": {} },
        { "body": {} },
        { "body": {} }
      ]
      """
    Then exactly 6 responses are 200
    When POST /api/v1/payments is called concurrently:
      """json
      [
        { "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc", "Idempotency-Key": "burst-<run>-1" },
          "body": { "merchant_trade_no": "TN-BURST-<run>-1", "amount": 100, "currency": "USD", "item_desc": "Beans", "notify_url": "/demo-merchant/api/notify", "client_back_url": "/shop/result" } },
        { "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc", "Idempotency-Key": "burst-<run>-2" },
          "body": { "merchant_trade_no": "TN-BURST-<run>-2", "amount": 100, "currency": "USD", "item_desc": "Beans", "notify_url": "/demo-merchant/api/notify", "client_back_url": "/shop/result" } },
        { "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc", "Idempotency-Key": "burst-<run>-3" },
          "body": { "merchant_trade_no": "TN-BURST-<run>-3", "amount": 100, "currency": "USD", "item_desc": "Beans", "notify_url": "/demo-merchant/api/notify", "client_back_url": "/shop/result" } },
        { "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc", "Idempotency-Key": "burst-<run>-4" },
          "body": { "merchant_trade_no": "TN-BURST-<run>-4", "amount": 100, "currency": "USD", "item_desc": "Beans", "notify_url": "/demo-merchant/api/notify", "client_back_url": "/shop/result" } },
        { "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc", "Idempotency-Key": "burst-<run>-5" },
          "body": { "merchant_trade_no": "TN-BURST-<run>-5", "amount": 100, "currency": "USD", "item_desc": "Beans", "notify_url": "/demo-merchant/api/notify", "client_back_url": "/shop/result" } },
        { "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc", "Idempotency-Key": "burst-<run>-6" },
          "body": { "merchant_trade_no": "TN-BURST-<run>-6", "amount": 100, "currency": "USD", "item_desc": "Beans", "notify_url": "/demo-merchant/api/notify", "client_back_url": "/shop/result" } }
      ]
      """
    Then exactly 3 responses are 201
    And exactly 3 responses are 429
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments HAVING count(*) = 3;
      """

    Examples: three runs
      | run |
      | 1   |
      | 2   |
      | 3   |

  Scenario: Being refused does not cost a token, so the next caller sees the same wait
    # If a 429 itself spent a token, a client that keeps politely retrying
    # while empty would push its own bucket further into debt and be told to
    # wait longer each time it asked — punishing the caller for obeying
    # Retry-After. A token bucket only ever spends on a request it admits.
    When POST /api/v1/payments:
      """json
      { "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc", "Idempotency-Key": "rl-cost-1" },
        "body": { "merchant_trade_no": "TN-RL-COST-1", "amount": 100, "currency": "USD", "item_desc": "Beans", "notify_url": "/demo-merchant/api/notify", "client_back_url": "/shop/result" } }
      """
    Then response status is 201
    When POST /api/v1/payments:
      """json
      { "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc", "Idempotency-Key": "rl-cost-2" },
        "body": { "merchant_trade_no": "TN-RL-COST-2", "amount": 100, "currency": "USD", "item_desc": "Beans", "notify_url": "/demo-merchant/api/notify", "client_back_url": "/shop/result" } }
      """
    Then response status is 201
    When POST /api/v1/payments:
      """json
      { "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc", "Idempotency-Key": "rl-cost-3" },
        "body": { "merchant_trade_no": "TN-RL-COST-3", "amount": 100, "currency": "USD", "item_desc": "Beans", "notify_url": "/demo-merchant/api/notify", "client_back_url": "/shop/result" } }
      """
    Then response status is 201
    When POST /api/v1/payments:
      """json
      { "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc", "Idempotency-Key": "rl-cost-4" },
        "body": { "merchant_trade_no": "TN-RL-COST-4", "amount": 100, "currency": "USD", "item_desc": "Beans", "notify_url": "/demo-merchant/api/notify", "client_back_url": "/shop/result" } }
      """
    Then response status is 429
    And response header "Retry-After" contains "20"
    When POST /api/v1/payments:
      """json
      { "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc", "Idempotency-Key": "rl-cost-5" },
        "body": { "merchant_trade_no": "TN-RL-COST-5", "amount": 100, "currency": "USD", "item_desc": "Beans", "notify_url": "/demo-merchant/api/notify", "client_back_url": "/shop/result" } }
      """
    Then response status is 429
    And response header "Retry-After" contains "20"
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments HAVING count(*) = 3;
      """

  Scenario: A bucket of zero refuses every request, from the first
    # rate_limit_per_minute is data, not a constant, and nothing stops it
    # being set to zero — a merchant suspended for abuse, or a plan with no
    # API access at all. Continuous refill of zero tokens per minute is still
    # zero, so the very first request must be refused exactly like the
    # steady-state case above, not treated as a free initial allowance.
    Given in PostgreSQL:
      """sql
      UPDATE merchants SET rate_limit_per_minute = 0 WHERE id = 1;
      """
    When POST /api/v1/payments:
      """json
      { "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc", "Idempotency-Key": "rl-zero-1" },
        "body": { "merchant_trade_no": "TN-RL-ZERO-1", "amount": 100, "currency": "USD", "item_desc": "Beans", "notify_url": "/demo-merchant/api/notify", "client_back_url": "/shop/result" } }
      """
    Then response status is 429
    And response body contains:
      """json
      {
        "error": "RATE_LIMITED"
      }
      """
    And in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM payments;
      """
    And in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM idempotency_keys WHERE idempotency_key = 'rl-zero-1';
      """
