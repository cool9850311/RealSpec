@reports
Feature: Reporting, and the projections that feed it

  The report is a projection: the relay publishes `payment_events` to Kafka,
  the ingester consumes it into ClickHouse, and the dashboard reads ClickHouse.
  PostgreSQL is the truth and the projection is a copy that arrives later, so
  this feature asserts four things about the copy.

  That they ARRIVE — across a stopped relay, a stopped ingester and a stopped
  ClickHouse.

  That they arrive ONCE — an event published twice is counted once.

  That it can be THROWN AWAY and rebuilt from `payment_events`, with the same
  numbers coming back. That is what the audit table buys, and it is the reason
  the projection is allowed to be wrong: it is never the record of anything.

  Finally, that they are CUT AT THE MERCHANT'S MIDNIGHT: Acme settles in
  Asia/Taipei (UTC+8) and Globex in UTC, so the seeded events below sit on both
  sides of Taipei's midnight.

  History is seeded as rows of `payment_events`, the same append-only table the
  service writes, because that is the projection's only input and the only past
  it can be rebuilt from. The matching `payments` rows are deliberately left
  out: no scenario here reads them, and seeding them would only restate what
  the audit rows already say. Nothing in this file writes to ClickHouse.

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
    And in PostgreSQL:
      """sql
      INSERT INTO dashboard_users (id, merchant_id, email, password_hash) VALUES
        (1, 1, 'owner@acme.test',   '$2a$10$MwKxT13/lPxFMjvrkm0dL.QJuNll1zljDU.eOoRVvgrWF.kf/hyC2'),
        (2, 2, 'owner@globex.test', '$2a$10$MwKxT13/lPxFMjvrkm0dL.QJuNll1zljDU.eOoRVvgrWF.kf/hyC2');
      """

  Scenario: Live orders, a declined attempt and a refund reach the report, counted once each
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "key-acme-live-1"
        },
        "body": {
          "merchant_trade_no": "ACME-LIVE-1",
          "amount": 1000,
          "currency": "USD",
          "item_desc": "Beans",
          "notify_url": "/demo-merchant/api/notify",
          "client_back_url": "/shop/result"
        }
      }
      """
    Then response status is 201
    And save response body field "id" as "live1Id"
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
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "key-acme-live-2"
        },
        "body": {
          "merchant_trade_no": "ACME-LIVE-2",
          "amount": 2500,
          "currency": "USD",
          "item_desc": "Beans",
          "notify_url": "/demo-merchant/api/notify",
          "client_back_url": "/shop/result"
        }
      }
      """
    Then response status is 201
    And save response body field "id" as "live2Id"
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
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "key-acme-live-3"
        },
        "body": {
          "merchant_trade_no": "ACME-LIVE-3",
          "amount": 700,
          "currency": "USD",
          "item_desc": "Beans",
          "notify_url": "/demo-merchant/api/notify",
          "client_back_url": "/shop/result"
        }
      }
      """
    Then response status is 201
    And save response body field "id" as "live3Id"
    When the payment form is submitted to the payment provider
    Then response status is 200
    And the customer pays at the payment provider:
      """json
      {
        "card_number": "4000000000000002",
        "card_expiry": "12/30",
        "card_cvc": "123"
      }
      """
    Then response status is 303
    # Two attempts are now waiting on the provider: the second order's, which
    # will be approved, and the third's, which will be declined. Neither is
    # decided until the callback arrives.
    When payment provider delivers each pending callback 1 time
    Then exactly 2 responses are 200
    When POST /api/v1/payments/{live1Id}/refunds:
      """json
      {
        "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc", "Idempotency-Key": "live-refund-1" },
        "body": { "amount": 500 }
      }
      """
    Then response status is 201
    When background work has settled
    Then in ClickHouse query returns 1 row:
      """sql
      SELECT count() AS events
        FROM report_events FINAL
       WHERE merchant_id = 1
      HAVING events = 4;
      """
    And in ClickHouse query returns 1 row:
      """sql
      SELECT 1
        FROM report_events FINAL
       WHERE merchant_id = 1
         AND event_type = 'PaymentRefunded'
         AND payment_id = '{live1Id}'
         AND amount = 500;
      """
    When POST /api/v1/dashboard/login:
      """json
      {
        "body": { "email": "owner@acme.test", "password": "secret123" }
      }
      """
    Then response status is 200
    And save response cookie "session" as "ownerSession"
    When GET /api/v1/dashboard/reports/daily:
      """json
      {
        "headers": { "Cookie": "session={ownerSession}" }
      }
      """
    Then response status is 200
    And response body contains:
      """json
      {
        "currency": "USD",
        "timezone": "Asia/Taipei",
        "totals": {
          "succeeded_count": 2,
          "failed_count": 1,
          "refunded_count": 1,
          "gross_amount": 3500,
          "refunded_amount": 500,
          "net_amount": 3000,
          "success_rate_bps": 6666,
          "declines": {
            "card_declined": 1
          }
        }
      }
      """

  Scenario: Days are cut at the merchant's midnight, and each merchant sees only its own events
    Given in PostgreSQL:
      """sql
      INSERT INTO payment_events (event_id, payment_id, seq, merchant_id, event_type, payload, occurred_at) VALUES
        ('0192a000-0000-7000-8000-000000000001', '0192a000-0000-7000-8000-0000000000a1', 1, 1, 'PaymentCreated',
         '{"amount": 1000, "currency": "USD", "card_brand": "visa", "card_last4": "4242", "reference": "a1"}', '2026-09-17T15:59:58Z'),
        ('0192a000-0000-7000-8000-000000000002', '0192a000-0000-7000-8000-0000000000a1', 2, 1, 'PaymentSucceeded',
         '{"amount": 1000, "currency": "USD", "card_brand": "visa", "reference": "a1", "provider_charge_id": "ch_a1", "source": "callback"}', '2026-09-17T15:59:59Z'),
        ('0192a000-0000-7000-8000-000000000003', '0192a000-0000-7000-8000-0000000000a2', 1, 1, 'PaymentCreated',
         '{"amount": 2000, "currency": "USD", "card_brand": "visa", "card_last4": "4242", "reference": "a2"}', '2026-09-17T15:59:59Z'),
        ('0192a000-0000-7000-8000-000000000004', '0192a000-0000-7000-8000-0000000000a2', 2, 1, 'PaymentSucceeded',
         '{"amount": 2000, "currency": "USD", "card_brand": "visa", "reference": "a2", "provider_charge_id": "ch_a2", "source": "callback"}', '2026-09-17T16:00:00Z'),
        ('0192a000-0000-7000-8000-000000000005', '0192a000-0000-7000-8000-0000000000a3', 1, 1, 'PaymentCreated',
         '{"amount": 300, "currency": "USD", "card_brand": "mastercard", "card_last4": "4444", "reference": "a3"}', '2026-09-18T02:59:59Z'),
        ('0192a000-0000-7000-8000-000000000006', '0192a000-0000-7000-8000-0000000000a3', 2, 1, 'PaymentAttemptFailed',
         '{"amount": 300, "currency": "USD", "card_brand": "mastercard", "reference": "a3", "failure_code": "insufficient_funds", "source": "callback"}', '2026-09-18T03:00:00Z'),
        ('0192a000-0000-7000-8000-000000000007', '0192a000-0000-7000-8000-0000000000a2', 3, 1, 'PaymentRefunded',
         '{"amount": 2000, "currency": "USD", "card_brand": "visa", "reference": "a2", "refund_id": "0192a000-0000-7000-8000-0000000000f1", "provider_refund_id": "re_a2"}', '2026-09-18T04:00:00Z'),
        ('0192a000-0000-7000-8000-000000000008', '0192a000-0000-7000-8000-0000000000b1', 1, 2, 'PaymentCreated',
         '{"amount": 1000, "currency": "EUR", "card_brand": "visa", "card_last4": "4242", "reference": "b1"}', '2026-09-17T15:59:58Z'),
        ('0192a000-0000-7000-8000-000000000009', '0192a000-0000-7000-8000-0000000000b1', 2, 2, 'PaymentSucceeded',
         '{"amount": 1000, "currency": "EUR", "card_brand": "visa", "reference": "b1", "provider_charge_id": "ch_b1", "source": "callback"}', '2026-09-17T15:59:59Z'),
        ('0192a000-0000-7000-8000-00000000000a', '0192a000-0000-7000-8000-0000000000b2', 1, 2, 'PaymentCreated',
         '{"amount": 2000, "currency": "EUR", "card_brand": "visa", "card_last4": "4242", "reference": "b2"}', '2026-09-17T15:59:59Z'),
        ('0192a000-0000-7000-8000-00000000000b', '0192a000-0000-7000-8000-0000000000b2', 2, 2, 'PaymentSucceeded',
         '{"amount": 2000, "currency": "EUR", "card_brand": "visa", "reference": "b2", "provider_charge_id": "ch_b2", "source": "callback"}', '2026-09-17T16:00:00Z');
      """
    When background work has settled
    Then in ClickHouse query returns 1 row:
      """sql
      SELECT count() AS events FROM report_events FINAL HAVING events = 6;
      """
    When POST /api/v1/dashboard/login:
      """json
      {
        "body": { "email": "owner@acme.test", "password": "secret123" }
      }
      """
    Then response status is 200
    And save response cookie "session" as "acmeSession"
    When GET /api/v1/dashboard/reports/daily?from=2026-09-17&to=2026-09-18:
      """json
      {
        "headers": { "Cookie": "session={acmeSession}" }
      }
      """
    Then response status is 200
    And response body contains:
      """json
      {
        "currency": "USD",
        "timezone": "Asia/Taipei",
        "from": "2026-09-17",
        "to": "2026-09-18",
        "days": [
          {
            "date": "2026-09-17",
            "succeeded_count": 1,
            "failed_count": 0,
            "refunded_count": 0,
            "gross_amount": 1000,
            "refunded_amount": 0,
            "net_amount": 1000,
            "success_rate_bps": 10000
          },
          {
            "date": "2026-09-18",
            "succeeded_count": 1,
            "failed_count": 1,
            "refunded_count": 1,
            "gross_amount": 2000,
            "refunded_amount": 2000,
            "net_amount": 0,
            "success_rate_bps": 5000
          }
        ],
        "totals": {
          "succeeded_count": 2,
          "failed_count": 1,
          "refunded_count": 1,
          "gross_amount": 3000,
          "refunded_amount": 2000,
          "net_amount": 1000,
          "success_rate_bps": 6666,
          "declines": {
            "insufficient_funds": 1
          }
        }
      }
      """
    When POST /api/v1/dashboard/login:
      """json
      {
        "body": { "email": "owner@globex.test", "password": "secret123" }
      }
      """
    Then response status is 200
    And save response cookie "session" as "globexSession"
    When GET /api/v1/dashboard/reports/daily?from=2026-09-17&to=2026-09-18:
      """json
      {
        "headers": { "Cookie": "session={globexSession}" }
      }
      """
    Then response status is 200
    And response body contains:
      """json
      {
        "currency": "EUR",
        "timezone": "UTC",
        "days": [
          {
            "date": "2026-09-17",
            "succeeded_count": 2,
            "failed_count": 0,
            "gross_amount": 3000,
            "refunded_amount": 0,
            "net_amount": 3000,
            "success_rate_bps": 10000
          },
          {
            "date": "2026-09-18",
            "succeeded_count": 0,
            "failed_count": 0,
            "gross_amount": 0,
            "refunded_amount": 0,
            "net_amount": 0,
            "success_rate_bps": null
          }
        ],
        "totals": {
          "succeeded_count": 2,
          "failed_count": 0,
          "gross_amount": 3000,
          "net_amount": 3000,
          "declines": {}
        }
      }
      """

  Scenario: An event the relay publishes twice is counted once
    # The UPDATE is what a relay that crashed after Kafka acknowledged the
    # message, but before it recorded that, leaves behind: an event that looks
    # unpublished. The relay publishes it again — at-least-once is the contract
    # — and publish_count records that it did, which is the proof that the
    # duplicate really reached Kafka rather than being skipped.
    Given in PostgreSQL:
      """sql
      INSERT INTO payment_events (event_id, payment_id, seq, merchant_id, event_type, payload, occurred_at) VALUES
        ('0192b000-0000-7000-8000-000000000001', '0192b000-0000-7000-8000-0000000000a1', 1, 1, 'PaymentCreated',
         '{"amount": 4200, "currency": "USD", "card_brand": "visa", "card_last4": "4242", "reference": "dup"}', '2026-09-10T01:59:59Z'),
        ('0192b000-0000-7000-8000-000000000002', '0192b000-0000-7000-8000-0000000000a1', 2, 1, 'PaymentSucceeded',
         '{"amount": 4200, "currency": "USD", "card_brand": "visa", "reference": "dup", "provider_charge_id": "ch_dup", "source": "callback"}', '2026-09-10T02:00:00Z');
      """
    When background work has settled
    Then in ClickHouse query returns 1 row:
      """sql
      SELECT 1 FROM report_events FINAL WHERE event_id = '0192b000-0000-7000-8000-000000000002';
      """
    Given in PostgreSQL:
      """sql
      UPDATE payment_events SET published_at = NULL WHERE event_id = '0192b000-0000-7000-8000-000000000002';
      """
    When background work has settled
    Then in ClickHouse query returns 1 row:
      """sql
      SELECT 1 FROM report_events FINAL WHERE event_id = '0192b000-0000-7000-8000-000000000002';
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payment_events
       WHERE event_id = '0192b000-0000-7000-8000-000000000002'
         AND publish_count = 2
         AND published_at IS NOT NULL;
      """
    When POST /api/v1/dashboard/login:
      """json
      {
        "body": { "email": "owner@acme.test", "password": "secret123" }
      }
      """
    Then response status is 200
    And save response cookie "session" as "ownerSession"
    When GET /api/v1/dashboard/reports/daily?from=2026-09-10&to=2026-09-10:
      """json
      {
        "headers": { "Cookie": "session={ownerSession}" }
      }
      """
    Then response status is 200
    And response body contains:
      """json
      {
        "totals": {
          "succeeded_count": 1,
          "gross_amount": 4200
        }
      }
      """

  Scenario: The projection is thrown away and rebuilt from the audit log, and nothing changes
    # A projection is not a record of anything, so destroying it costs nothing
    # but time. The merchant API is untouched by the rebuild — it reads
    # PostgreSQL — which this scenario also shows, by reading an order after the
    # projection has been rebuilt.
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "key-acme-rebuild-1"
        },
        "body": {
          "merchant_trade_no": "ACME-REBUILD-1",
          "amount": 1000,
          "currency": "USD",
          "item_desc": "Beans",
          "notify_url": "/demo-merchant/api/notify",
          "client_back_url": "/shop/result"
        }
      }
      """
    Then response status is 201
    And save response body field "id" as "keptId"
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
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "key-acme-rebuild-2"
        },
        "body": {
          "merchant_trade_no": "ACME-REBUILD-2",
          "amount": 2500,
          "currency": "USD",
          "item_desc": "Beans",
          "notify_url": "/demo-merchant/api/notify",
          "client_back_url": "/shop/result"
        }
      }
      """
    Then response status is 201
    And save response body field "id" as "refundedId"
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
    When POST /api/v1/payments/{refundedId}/refunds:
      """json
      {
        "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc", "Idempotency-Key": "rebuild-refund" },
        "body": { "amount": 2500 }
      }
      """
    Then response status is 201
    When background work has settled
    Then in ClickHouse query returns 1 row:
      """sql
      SELECT count() AS events FROM report_events FINAL WHERE merchant_id = 1 HAVING events = 3;
      """
    When projection "reports" is rebuilt from the event log
    Then in ClickHouse query returns 1 row:
      """sql
      SELECT count() AS events FROM report_events FINAL WHERE merchant_id = 1 HAVING events = 3;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments WHERE id = '{keptId}' AND status = 'succeeded' AND amount = 1000;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments WHERE id = '{refundedId}' AND status = 'refunded';
      """
    When POST /api/v1/dashboard/login:
      """json
      {
        "body": { "email": "owner@acme.test", "password": "secret123" }
      }
      """
    Then response status is 200
    And save response cookie "session" as "ownerSession"
    When GET /api/v1/dashboard/reports/daily:
      """json
      {
        "headers": { "Cookie": "session={ownerSession}" }
      }
      """
    Then response status is 200
    And response body contains:
      """json
      {
        "totals": {
          "succeeded_count": 2,
          "failed_count": 0,
          "refunded_count": 1,
          "gross_amount": 3500,
          "refunded_amount": 2500,
          "net_amount": 1000
        }
      }
      """
    When GET /api/v1/payments/{keptId}:
      """json
      {
        "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc" }
      }
      """
    Then response status is 200
    And response body contains:
      """json
      {
        "id": "{keptId}",
        "status": "succeeded"
      }
      """

  Scenario: Orders keep being paid while the relay is down, and their events arrive once it is back
    Given service "relay" is stopped
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "key-acme-relay-1"
        },
        "body": {
          "merchant_trade_no": "ACME-RELAY-1",
          "amount": 1000,
          "currency": "USD",
          "item_desc": "Beans",
          "notify_url": "/demo-merchant/api/notify",
          "client_back_url": "/shop/result"
        }
      }
      """
    Then response status is 201
    And save response body field "id" as "rd1Id"
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
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "key-acme-relay-2"
        },
        "body": {
          "merchant_trade_no": "ACME-RELAY-2",
          "amount": 2000,
          "currency": "USD",
          "item_desc": "Beans",
          "notify_url": "/demo-merchant/api/notify",
          "client_back_url": "/shop/result"
        }
      }
      """
    Then response status is 201
    And save response body field "id" as "rd2Id"
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
      SELECT 1 FROM payment_events WHERE published_at IS NULL HAVING count(*) = 4;
      """
    When service "relay" is started
    And background work has settled
    Then in ClickHouse query returns 1 row:
      """sql
      SELECT sum(amount) AS gross
        FROM report_events FINAL
       WHERE merchant_id = 1 AND event_type = 'PaymentSucceeded'
      HAVING gross = 3000 AND count() = 2;
      """

  Scenario: Events wait in Kafka while the ingester is down, and arrive once it is back
    Given service "ingester" is stopped
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "key-acme-ing-1"
        },
        "body": {
          "merchant_trade_no": "ACME-ING-1",
          "amount": 1500,
          "currency": "USD",
          "item_desc": "Beans",
          "notify_url": "/demo-merchant/api/notify",
          "client_back_url": "/shop/result"
        }
      }
      """
    Then response status is 201
    And save response body field "id" as "ingId"
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
    When service "ingester" is started
    And background work has settled
    Then in ClickHouse query returns 1 row:
      """sql
      SELECT 1 FROM report_events FINAL
       WHERE payment_id = '{ingId}' AND event_type = 'PaymentSucceeded';
      """

  Scenario: With ClickHouse down, reports are unavailable but payments are not
    When POST /api/v1/dashboard/login:
      """json
      {
        "body": { "email": "owner@acme.test", "password": "secret123" }
      }
      """
    Then response status is 200
    And save response cookie "session" as "ownerSession"
    Given service "clickhouse" is stopped
    When GET /api/v1/dashboard/reports/daily:
      """json
      {
        "headers": { "Cookie": "session={ownerSession}" }
      }
      """
    Then response status is 503
    And response body contains:
      """json
      {
        "error": "REPORTING_UNAVAILABLE"
      }
      """
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "key-acme-ch-1"
        },
        "body": {
          "merchant_trade_no": "ACME-CH-1",
          "amount": 1500,
          "currency": "USD",
          "item_desc": "Beans",
          "notify_url": "/demo-merchant/api/notify",
          "client_back_url": "/shop/result"
        }
      }
      """
    Then response status is 201
    And save response body field "id" as "chId"
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
    When GET /api/v1/payments/{chId}:
      """json
      {
        "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc" }
      }
      """
    Then response status is 200
    Given service "clickhouse" is started
    When background work has settled
    Then in ClickHouse query returns 1 row:
      """sql
      SELECT count() AS events FROM report_events FINAL WHERE merchant_id = 1 HAVING events = 1;
      """
    When GET /api/v1/dashboard/reports/daily:
      """json
      {
        "headers": { "Cookie": "session={ownerSession}" }
      }
      """
    Then response status is 200
    And response body contains:
      """json
      {
        "totals": {
          "succeeded_count": 1,
          "gross_amount": 1500
        }
      }
      """

  Scenario: A report range must be ordered, well formed and at most 92 days
    When POST /api/v1/dashboard/login:
      """json
      {
        "body": { "email": "owner@acme.test", "password": "secret123" }
      }
      """
    Then response status is 200
    And save response cookie "session" as "ownerSession"
    When GET /api/v1/dashboard/reports/daily?from=2026-09-18&to=2026-09-17:
      """json
      {
        "headers": { "Cookie": "session={ownerSession}" }
      }
      """
    Then response status is 422
    And response body contains:
      """json
      {
        "error": "VALIDATION_FAILED",
        "field": "from"
      }
      """
    When GET /api/v1/dashboard/reports/daily?from=2026-06-01&to=2026-09-01:
      """json
      {
        "headers": { "Cookie": "session={ownerSession}" }
      }
      """
    Then response status is 422
    And response body contains:
      """json
      {
        "error": "RANGE_TOO_LARGE"
      }
      """
    When GET /api/v1/dashboard/reports/daily?from=2026-06-02&to=2026-09-01:
      """json
      {
        "headers": { "Cookie": "session={ownerSession}" }
      }
      """
    Then response status is 200
    When GET /api/v1/dashboard/reports/daily?from=2026-09-01&to=09-18-2026:
      """json
      {
        "headers": { "Cookie": "session={ownerSession}" }
      }
      """
    Then response status is 422
    And response body contains:
      """json
      {
        "error": "VALIDATION_FAILED",
        "field": "to"
      }
      """

  Scenario: Reports need a dashboard session; an API key is not one
    When GET /api/v1/dashboard/reports/daily:
      """json
      {
        "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc" }
      }
      """
    Then response status is 401
    And response body contains:
      """json
      {
        "error": "UNAUTHENTICATED"
      }
      """

  Scenario: A 0% day, a 100% day and a day that is entirely a refund, with net_amount going negative
    # net_amount = gross - refunded "may be negative" (spec.md) and nothing
    # above exercised it: every refund so far landed on a day that also had a
    # success. Here the refund is the ONLY thing on its day, dated a full
    # calendar day after the payment it refunds — not merely the other side of
    # one midnight instant — so the day it is cut into has no gross to net
    # against. The day before it has one declined attempt and nothing else
    # (0% of attempts succeeded); the day between has one success and nothing
    # else (100%).
    Given in PostgreSQL:
      """sql
      INSERT INTO payment_events (event_id, payment_id, seq, merchant_id, event_type, payload, occurred_at) VALUES
        ('0192c000-0000-7000-8000-000000000001', '0192c000-0000-7000-8000-0000000000c1', 1, 1, 'PaymentCreated',
         '{"amount": 500, "currency": "USD", "card_brand": "visa", "card_last4": "0002", "reference": "c1"}', '2026-09-20T05:00:00Z'),
        ('0192c000-0000-7000-8000-000000000002', '0192c000-0000-7000-8000-0000000000c1', 2, 1, 'PaymentAttemptFailed',
         '{"amount": 500, "currency": "USD", "card_brand": "visa", "reference": "c1", "failure_code": "card_declined", "source": "callback"}', '2026-09-20T05:00:01Z'),
        ('0192c000-0000-7000-8000-000000000003', '0192c000-0000-7000-8000-0000000000c2', 1, 1, 'PaymentCreated',
         '{"amount": 1000, "currency": "USD", "card_brand": "visa", "card_last4": "4242", "reference": "c2"}', '2026-09-21T05:00:00Z'),
        ('0192c000-0000-7000-8000-000000000004', '0192c000-0000-7000-8000-0000000000c2', 2, 1, 'PaymentSucceeded',
         '{"amount": 1000, "currency": "USD", "card_brand": "visa", "reference": "c2", "provider_charge_id": "ch_c2", "source": "callback"}', '2026-09-21T05:00:01Z'),
        ('0192c000-0000-7000-8000-000000000005', '0192c000-0000-7000-8000-0000000000c2', 3, 1, 'PaymentRefunded',
         '{"amount": 1000, "currency": "USD", "card_brand": "visa", "reference": "c2", "refund_id": "0192c000-0000-7000-8000-0000000000f1", "provider_refund_id": "re_c2"}', '2026-09-22T05:00:00Z');
      """
    When background work has settled
    Then in ClickHouse query returns 1 row:
      """sql
      SELECT count() AS events FROM report_events FINAL WHERE merchant_id = 1 HAVING events = 3;
      """
    When POST /api/v1/dashboard/login:
      """json
      {
        "body": { "email": "owner@acme.test", "password": "secret123" }
      }
      """
    Then response status is 200
    And save response cookie "session" as "ownerSession"
    When GET /api/v1/dashboard/reports/daily?from=2026-09-20&to=2026-09-22:
      """json
      {
        "headers": { "Cookie": "session={ownerSession}" }
      }
      """
    Then response status is 200
    And response body contains:
      """json
      {
        "from": "2026-09-20",
        "to": "2026-09-22",
        "days": [
          {
            "date": "2026-09-20",
            "succeeded_count": 0,
            "failed_count": 1,
            "refunded_count": 0,
            "gross_amount": 0,
            "refunded_amount": 0,
            "net_amount": 0,
            "success_rate_bps": 0
          },
          {
            "date": "2026-09-21",
            "succeeded_count": 1,
            "failed_count": 0,
            "refunded_count": 0,
            "gross_amount": 1000,
            "refunded_amount": 0,
            "net_amount": 1000,
            "success_rate_bps": 10000
          },
          {
            "date": "2026-09-22",
            "succeeded_count": 0,
            "failed_count": 0,
            "refunded_count": 1,
            "gross_amount": 0,
            "refunded_amount": 1000,
            "net_amount": -1000,
            "success_rate_bps": null
          }
        ],
        "totals": {
          "succeeded_count": 1,
          "failed_count": 1,
          "refunded_count": 1,
          "gross_amount": 1000,
          "refunded_amount": 1000,
          "net_amount": 0,
          "success_rate_bps": 5000,
          "declines": {
            "card_declined": 1
          }
        }
      }
      """

  Scenario: A merchant with no activity at all gets a well-formed empty report, not an error
    # succeeded + failed = 0 makes success_rate_bps a division by zero
    # (spec.md: "null when there were neither"), and it is not only a per-day
    # case — a merchant that has done nothing in the whole range must see the
    # same null at the totals level, not a 500 and not an empty body.
    When POST /api/v1/dashboard/login:
      """json
      {
        "body": { "email": "owner@globex.test", "password": "secret123" }
      }
      """
    Then response status is 200
    And save response cookie "session" as "globexSession"
    And background work has settled
    When GET /api/v1/dashboard/reports/daily?from=2026-09-20&to=2026-09-20:
      """json
      {
        "headers": { "Cookie": "session={globexSession}" }
      }
      """
    Then response status is 200
    And response body contains:
      """json
      {
        "currency": "EUR",
        "timezone": "UTC",
        "from": "2026-09-20",
        "to": "2026-09-20",
        "days": [
          {
            "date": "2026-09-20",
            "succeeded_count": 0,
            "failed_count": 0,
            "refunded_count": 0,
            "gross_amount": 0,
            "refunded_amount": 0,
            "net_amount": 0,
            "success_rate_bps": null
          }
        ],
        "totals": {
          "succeeded_count": 0,
          "failed_count": 0,
          "refunded_count": 0,
          "gross_amount": 0,
          "refunded_amount": 0,
          "net_amount": 0,
          "success_rate_bps": null,
          "declines": {}
        }
      }
      """

  Scenario: A rebuild does not disturb events that keep arriving once it is done
    # NFR-PROJ-1 is proved above by throwing the projection away mid-history;
    # this is the other half. The checkpoint a rebuild leaves behind must not
    # stop the ingester from picking up whatever is published next, which is
    # the whole content of "the ingester may keep consuming while it runs"
    # (spec.md, "The report's projection").
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "key-acme-rebuild-live-1"
        },
        "body": {
          "merchant_trade_no": "ACME-REBUILD-LIVE-1",
          "amount": 1000,
          "currency": "USD",
          "item_desc": "Beans",
          "notify_url": "/demo-merchant/api/notify",
          "client_back_url": "/shop/result"
        }
      }
      """
    Then response status is 201
    And save response body field "id" as "beforeId"
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
    Then in ClickHouse query returns 1 row:
      """sql
      SELECT count() AS events FROM report_events FINAL WHERE merchant_id = 1 HAVING events = 1;
      """
    When projection "reports" is rebuilt from the event log
    Then in ClickHouse query returns 1 row:
      """sql
      SELECT count() AS events FROM report_events FINAL WHERE merchant_id = 1 HAVING events = 1;
      """
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "key-acme-rebuild-live-2"
        },
        "body": {
          "merchant_trade_no": "ACME-REBUILD-LIVE-2",
          "amount": 2000,
          "currency": "USD",
          "item_desc": "Beans",
          "notify_url": "/demo-merchant/api/notify",
          "client_back_url": "/shop/result"
        }
      }
      """
    Then response status is 201
    And save response body field "id" as "afterId"
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
    Then in ClickHouse query returns 1 row:
      """sql
      SELECT count() AS events FROM report_events FINAL WHERE merchant_id = 1 HAVING events = 2;
      """
    When POST /api/v1/dashboard/login:
      """json
      {
        "body": { "email": "owner@acme.test", "password": "secret123" }
      }
      """
    Then response status is 200
    And save response cookie "session" as "ownerSession"
    When GET /api/v1/dashboard/reports/daily:
      """json
      {
        "headers": { "Cookie": "session={ownerSession}" }
      }
      """
    Then response status is 200
    And response body contains:
      """json
      {
        "totals": {
          "succeeded_count": 2,
          "gross_amount": 3000
        }
      }
      """
