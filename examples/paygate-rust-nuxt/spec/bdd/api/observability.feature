@observability
Feature: Health and request correlation

  An orchestrator needs two answers from each replica — is the process alive,
  and should it receive traffic — and an operator needs one thread to pull on
  when a merchant reports a failed request: the request id, echoed back to the
  caller on every response, success or failure, and written on every log line
  the request produced (spec.md, "Observability").

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

  Scenario: Liveness and readiness answer without credentials
    When GET /api/v1/health/live:
      """json
      {}
      """
    Then response status is 200
    And response body contains:
      """json
      {
        "status": "live"
      }
      """
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
          "redis": "up"
        }
      }
      """

  Scenario: A caller's request id is echoed back, on success and on refusal alike
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "key-tn-trace-1"
        },
        "body": {
          "merchant_trade_no": "TN-TRACE-1",
          "amount": 100,
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
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "X-Request-Id": "merchant-trace-7f3a"
        }
      }
      """
    Then response status is 200
    And response header "X-Request-Id" contains "merchant-trace-7f3a"
    When GET /api/v1/payments/{paymentId}:
      """json
      {
        "headers": {
          "X-Request-Id": "merchant-trace-8b1c"
        }
      }
      """
    Then response status is 401
    And response header "X-Request-Id" contains "merchant-trace-8b1c"

  Scenario: A request without an id is given one
    When GET /api/v1/health/live:
      """json
      {}
      """
    Then response status is 200
    And response header "X-Request-Id" contains "req_"

  Scenario: Every response carries the headers that keep it out of caches and sniffers
    When GET /api/v1/payments/00000000-0000-7000-8000-000000000000:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc"
        }
      }
      """
    Then response status is 404
    And response header "Cache-Control" contains "no-store"
    And response header "X-Content-Type-Options" contains "nosniff"
    And response header "Content-Type" contains "application/json"

  Scenario: Health endpoints tell an anonymous caller up or down, and nothing more
    # /health/live and /health/ready carry no security scheme (spec.md,
    # "Observability") because an orchestrator's probe cannot present one —
    # which means anyone on the network can send the same probe. Their
    # documented shapes are exactly {status} and {status, checks}; a build
    # version, a hostname or a connection string here would hand a scanner,
    # for free, exactly the fingerprint an orchestrator never needed.
    When GET /api/v1/health/live:
      """json
      {}
      """
    Then response status is 200
    And response body does not contain "version"
    And response body does not contain "://"
    When GET /api/v1/health/ready:
      """json
      {}
      """
    Then response status is 200
    And response body does not contain "version"
    And response body does not contain "://"
