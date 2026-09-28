@stack:scaled @serial @scaling
Feature: Horizontal scaling

  Every scenario here runs on the scaled stack: THREE API replicas, TWO relays,
  TWO ingesters, TWO notifiers and a three-partition topic — against one
  PostgreSQL, one Redis, one Kafka and one ClickHouse, which is the shape the
  gateway is deployed in (spec.md, "Scaling"). @serial runs each one with the
  machine to itself: a race between processes is only a race if every process
  gets CPU.

  The harness assigns work to replicas by a fixed rule, never by chance:

    single requests      the k-th request of the scenario → replica ((k-1) mod 3) + 1
    concurrent callers   caller i (0-based)                → replica (i mod 3) + 1
    provider callbacks   delivery i (0-based)              → replica (i mod 3) + 1

  So every claim below is a claim about separate processes that share nothing
  but the data stores, and it is the same claim on every run. The hand-off race
  and the refund race are claims about PostgreSQL specifically: a row lock does
  not care which replica is asking, and neither does a unique index.

  Background:
    Given run migration
    And in PostgreSQL:
      """sql
      INSERT INTO merchants (id, name, currency, timezone, provider_code, provider_merchant_id, rate_limit_per_minute, hash_key, hash_iv) VALUES
        (1, 'Acme Coffee', 'USD', 'Asia/Taipei', 'ecpay',    '2000132',     6000, 'acmehashkey0123456789abcdef01234', 'acmehashiv012345');
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

  Scenario Outline: Six browsers opening one checkout on three replicas get six distinct provider numbers
    # The customer double-clicked, or the page is open in six tabs behind a load
    # balancer. Each one legitimately gets its own order number at ECPay — that is
    # the rule — so what has to hold across processes is that no two of them are
    # the SAME number, and that settling one of them settles the order once.
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "key-acme-scaled-<run>"
        },
        "body": {
          "merchant_trade_no": "ACME-SCALED-<run>",
          "amount": 3000,
          "currency": "USD",
          "item_desc": "Beans",
          "notify_url": "/demo-merchant/api/notify",
          "client_back_url": "/shop/result"
        }
      }
      """
    Then response status is 201
    And save response body field "id" as "paymentId"
    # Warm every replica's pools first, so that the burst below races the three
    # PROCESSES rather than three connection pools being opened for the first
    # time (format.yml, `requests_concurrent`).
    When these things happen at one instant:
      """json
      [
        {"method": "GET", "path": "/api/v1/health/ready", "body": {}},
        {"method": "GET", "path": "/api/v1/health/ready", "body": {}},
        {"method": "GET", "path": "/api/v1/health/ready", "body": {}},
        {"method": "GET", "path": "/api/v1/health/ready", "body": {}},
        {"method": "GET", "path": "/api/v1/health/ready", "body": {}},
        {"method": "GET", "path": "/api/v1/health/ready", "body": {}}
      ]
      """
    Then exactly 6 responses are 200
    # And now the six browsers, at one instant, two per replica. Each carries its
    # own Idempotency-Key, because the same key would be a retry of one request
    # rather than six requests — the other half of "Two kinds of duplicate".
    # Every one of them is answered `201` with the SAME order and a form of its
    # own: that is the rule ECPay forces (spec.md, "One order, three numbers"),
    # and the one whose insert loses the UNIQUE on (merchant_id,
    # merchant_trade_no) must still answer with the order that won rather than
    # with an error.
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
            "merchant_trade_no": "ACME-SCALED-<run>",
            "amount": 3000,
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
            "merchant_trade_no": "ACME-SCALED-<run>",
            "amount": 3000,
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
            "merchant_trade_no": "ACME-SCALED-<run>",
            "amount": 3000,
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
            "merchant_trade_no": "ACME-SCALED-<run>",
            "amount": 3000,
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
            "merchant_trade_no": "ACME-SCALED-<run>",
            "amount": 3000,
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
            "merchant_trade_no": "ACME-SCALED-<run>",
            "amount": 3000,
            "currency": "USD",
            "item_desc": "Beans",
            "notify_url": "/demo-merchant/api/notify",
            "client_back_url": "/shop/result"
          }
        }
      ]
      """
    Then exactly 6 responses are 201
    # Seven forms in total — the merchant's first render, and the six browsers —
    # and no two of them carry the same number. That is the generator's only real
    # test, because it is the only place three processes mint at once.
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payment_attempts
       WHERE payment_id = '{paymentId}'
      HAVING count(*) = 7 AND count(DISTINCT provider_trade_no) = 7;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payment_events
       WHERE payment_id = '{paymentId}' AND event_type = 'PaymentAttemptStarted'
      HAVING count(*) = 7;
      """
    # One order, though: six concurrent callers created no second one.
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments WHERE merchant_trade_no = 'ACME-SCALED-<run>'
      HAVING count(*) = 1;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payment_events
       WHERE payment_id = '{paymentId}' AND event_type = 'PaymentCreated'
      HAVING count(*) = 1;
      """
    # Paying with the first form settles the order once; the six unused numbers
    # are live orders at the provider that nobody paid, and the reconciler ends
    # them later on the provider's word (reconcile.feature).
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
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payment_events
       WHERE payment_id = '{paymentId}' AND event_type = 'PaymentSucceeded'
      HAVING count(*) = 1;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM notifications WHERE payment_id = '{paymentId}' HAVING count(*) = 1;
      """

    Examples: three runs
      | run |
      | 1   |
      | 2   |
      | 3   |

  Scenario Outline: Six refunds racing on three replicas never refund more than was charged
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "key-acme-sref-<run>"
        },
        "body": {
          "merchant_trade_no": "ACME-SREF-<run>",
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
    When these things happen at one instant:
      """json
      [
        {"method": "GET", "path": "/api/v1/health/ready", "body": {}},
        {"method": "GET", "path": "/api/v1/health/ready", "body": {}},
        {"method": "GET", "path": "/api/v1/health/ready", "body": {}},
        {"method": "GET", "path": "/api/v1/health/ready", "body": {}},
        {"method": "GET", "path": "/api/v1/health/ready", "body": {}},
        {"method": "GET", "path": "/api/v1/health/ready", "body": {}}
      ]
      """
    Then exactly 6 responses are 200
    When these things happen at one instant:
      """json
      [
        {
          "method": "POST",
          "path": "/api/v1/payments/{paymentId}/refunds",
          "headers": {
            "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
            "Idempotency-Key": "sref-<run>-a"
          },
          "body": {
            "amount": 3000
          }
        },
        {
          "method": "POST",
          "path": "/api/v1/payments/{paymentId}/refunds",
          "headers": {
            "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
            "Idempotency-Key": "sref-<run>-b"
          },
          "body": {
            "amount": 3000
          }
        },
        {
          "method": "POST",
          "path": "/api/v1/payments/{paymentId}/refunds",
          "headers": {
            "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
            "Idempotency-Key": "sref-<run>-c"
          },
          "body": {
            "amount": 3000
          }
        },
        {
          "method": "POST",
          "path": "/api/v1/payments/{paymentId}/refunds",
          "headers": {
            "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
            "Idempotency-Key": "sref-<run>-d"
          },
          "body": {
            "amount": 3000
          }
        },
        {
          "method": "POST",
          "path": "/api/v1/payments/{paymentId}/refunds",
          "headers": {
            "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
            "Idempotency-Key": "sref-<run>-e"
          },
          "body": {
            "amount": 3000
          }
        },
        {
          "method": "POST",
          "path": "/api/v1/payments/{paymentId}/refunds",
          "headers": {
            "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
            "Idempotency-Key": "sref-<run>-f"
          },
          "body": {
            "amount": 3000
          }
        }
      ]
      """
    Then exactly 3 responses are 201
    And exactly 3 responses are 422
    And payment provider received 3 refund requests
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments WHERE id = '{paymentId}' AND amount_refunded = 9000;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM refunds WHERE payment_id = '{paymentId}' HAVING count(*) = 3 AND sum(amount) = 9000;
      """

    Examples: three runs
      | run |
      | 1   |
      | 2   |
      | 3   |

  Scenario Outline: Six copies of one provider callback delivered to three replicas settle the order once
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "key-acme-swh-<run>"
        },
        "body": {
          "merchant_trade_no": "ACME-SWH-<run>",
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
    When these things happen at one instant:
      """json
      [
        {"method": "GET", "path": "/api/v1/health/ready", "body": {}},
        {"method": "GET", "path": "/api/v1/health/ready", "body": {}},
        {"method": "GET", "path": "/api/v1/health/ready", "body": {}},
        {"method": "GET", "path": "/api/v1/health/ready", "body": {}},
        {"method": "GET", "path": "/api/v1/health/ready", "body": {}},
        {"method": "GET", "path": "/api/v1/health/ready", "body": {}}
      ]
      """
    Then exactly 6 responses are 200
    When payment provider delivers each pending callback 6 times
    Then exactly 6 responses are 200
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payment_events
       WHERE payment_id = '{paymentId}' AND event_type = 'PaymentSucceeded'
      HAVING count(*) = 1;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM provider_events WHERE payment_id = '{paymentId}' HAVING count(*) = 1;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM notifications WHERE payment_id = '{paymentId}' HAVING count(*) = 1;
      """

    Examples: three runs
      | run |
      | 1   |
      | 2   |
      | 3   |

  Scenario: A session opened on one replica is valid on the others, and closed on all of them at once
    # Request 1 → replica 1, request 2 → replica 2, request 3 → replica 3,
    # request 4 → replica 1, request 5 → replica 2.
    When POST /api/v1/dashboard/login:
      """json
      {
        "body": { "email": "owner@acme.test", "password": "secret123" }
      }
      """
    Then response status is 200
    And save response cookie "session" as "ownerSession"
    When GET /api/v1/dashboard/me:
      """json
      {
        "headers": { "Cookie": "session={ownerSession}" }
      }
      """
    Then response status is 200
    When GET /api/v1/dashboard/me:
      """json
      {
        "headers": { "Cookie": "session={ownerSession}" }
      }
      """
    Then response status is 200
    When POST /api/v1/dashboard/logout:
      """json
      {
        "headers": { "Cookie": "session={ownerSession}" }
      }
      """
    Then response status is 204
    When GET /api/v1/dashboard/me:
      """json
      {
        "headers": { "Cookie": "session={ownerSession}" }
      }
      """
    Then response status is 401

  Scenario: An order created on one replica is paid on another, refunded on a third and read on the first
    # Request 1 → replica 1 creates it. The callback that takes the payment is
    # not a single request but a delivery, with its own counter (spec.md,
    # "Scaling"): delivery 0 → replica 1, same replica as the create by
    # coincidence of the formula, not because anything pins it there. Request 2
    # → replica 2 refunds it, request 3 → replica 3 reads it back. Nothing
    # waits: PostgreSQL is the same store for all three.
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "key-acme-across-1"
        },
        "body": {
          "merchant_trade_no": "ACME-ACROSS-1",
          "amount": 5000,
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
        "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc", "Idempotency-Key": "across-refund-1" },
        "body": { "amount": 5000 }
      }
      """
    Then response status is 201
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
        "status": "refunded",
        "amount_refunded": 5000
      }
      """

  Scenario: Two notifiers tell the merchant once, not twice
    # The notification claim is the same SKIP LOCKED claim the relay makes, and
    # it matters more here: a merchant that is told twice may ship twice.
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "key-acme-ns-1"
        },
        "body": {
          "merchant_trade_no": "ACME-NS-1",
          "amount": 1000,
          "currency": "USD",
          "item_desc": "Beans",
          "notify_url": "/demo-merchant/api/notify",
          "client_back_url": "/shop/result"
        }
      }
      """
    Then response status is 201
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
          "Idempotency-Key": "key-acme-ns-2"
        },
        "body": {
          "merchant_trade_no": "ACME-NS-2",
          "amount": 2000,
          "currency": "USD",
          "item_desc": "Beans",
          "notify_url": "/demo-merchant/api/notify",
          "client_back_url": "/shop/result"
        }
      }
      """
    Then response status is 201
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
    Then merchant received 2 notifications at "/demo-merchant/api/notify"
    And in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM notifications WHERE attempts <> 1 OR delivered_at IS NULL;
      """

  Scenario: Two reconcilers racing one outstanding attempt query it once and settle it once
    # reconciler ×2 claims its work the same way relay and notifier do —
    # FOR UPDATE SKIP LOCKED (spec.md, "Scaling"). Both processes see the same
    # outstanding attempt, old enough to ask about, but only one locks the row;
    # the other skips it and finds nothing to do. One query reaches the
    # provider, not two, and the payment settles once.
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "key-acme-srecon-1"
        },
        "body": {
          "merchant_trade_no": "ACME-SRECON-1",
          "amount": 2500,
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
    Given in PostgreSQL:
      """sql
      UPDATE payment_attempts SET started_at = NOW() - INTERVAL '61 minutes'
       WHERE payment_id = '{paymentId}';
      """
    And payment provider answers the next query with trade status "1"
    When the reconciler runs
    Then in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments WHERE id = '{paymentId}' AND status = 'succeeded';
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM provider_queries q
        JOIN payment_attempts a ON a.id = q.attempt_id
       WHERE a.payment_id = '{paymentId}'
      HAVING count(*) = 1;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payment_events
       WHERE payment_id = '{paymentId}' AND event_type = 'PaymentReconciled'
      HAVING count(*) = 1;
      """
    When background work has settled
    Then merchant received 1 notification at "/demo-merchant/api/notify"

  Scenario: Six hundred audit rows through two relays and two ingesters arrive exactly once each
    # publish_count is incremented by the relay each time it publishes a row, so
    # "nothing published twice" is a fact in PostgreSQL rather than an inference
    # from ClickHouse, whose background merges could hide a duplicate. The
    # ingester_id column is written by each ingester from its own instance id;
    # two distinct values prove the consumer group really split the partitions.
    # `gen_random_uuid()` is called in the select list, not through a
    # `LATERAL (SELECT gen_random_uuid())`. A lateral subquery that does not
    # reference the outer relation is flattened by the planner and evaluated
    # ONCE: three hundred rows then share one id and the insert dies on
    # `payment_events_event_id_key`. Verified on PostgreSQL 17 —
    # `count(DISTINCT e.event_id)` over that shape is 1.
    Given in PostgreSQL:
      """sql
      INSERT INTO payment_events (event_id, payment_id, seq, merchant_id, event_type, payload, occurred_at)
      SELECT gen_random_uuid(), gen_random_uuid(), 1, 1, 'PaymentCreated',
             jsonb_build_object('amount', 100, 'currency', 'USD', 'card_brand', 'visa',
                                'reference', 'bulk-' || n),
             TIMESTAMPTZ '2026-09-10T02:00:00Z' + (n * INTERVAL '1 second')
        FROM generate_series(1, 300) AS n;
      """
    # Scoped to the rows above by their reference, so the pair count is exactly
    # six hundred whatever else the scenario's database may hold.
    And in PostgreSQL:
      """sql
      INSERT INTO payment_events (event_id, payment_id, seq, merchant_id, event_type, payload, occurred_at)
      SELECT gen_random_uuid(), i.payment_id, 2, 1, 'PaymentSucceeded',
             jsonb_build_object('amount', 100, 'currency', 'USD', 'card_brand', 'visa',
                                'reference', i.payload->>'reference',
                                'provider_charge_id', 'ch_bulk', 'source', 'callback'),
             i.occurred_at + INTERVAL '1 millisecond'
        FROM payment_events i
       WHERE i.event_type = 'PaymentCreated'
         AND i.payload->>'reference' LIKE 'bulk-%';
      """
    When background work has settled
    Then in ClickHouse query returns 1 row:
      """sql
      SELECT count() AS events
        FROM report_events FINAL
       WHERE merchant_id = 1
      HAVING events = 300 AND uniqExact(event_id) = 300;
      """
    And in ClickHouse query returns 2 rows:
      """sql
      SELECT ingester_id FROM report_events WHERE merchant_id = 1 GROUP BY ingester_id;
      """
    And in ClickHouse query returns 1 row:
      """sql
      SELECT count() AS succeeded
        FROM report_events FINAL
       WHERE merchant_id = 1 AND event_type = 'PaymentSucceeded' AND card_brand = 'visa'
      HAVING succeeded = 300;
      """
    And in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM payment_events WHERE publish_count <> 1 OR published_at IS NULL;
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
          "succeeded_count": 300,
          "failed_count": 0,
          "gross_amount": 30000,
          "net_amount": 30000,
          "success_rate_bps": 10000
        }
      }
      """
