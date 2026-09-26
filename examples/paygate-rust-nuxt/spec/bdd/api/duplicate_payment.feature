@duplicate-payment
Feature: When the customer pays twice

  This file exists because of a rule that cannot be argued with: ECPay refuses an
  order number it has already seen, so every form gets a new one
  (handoff.feature). A customer whose merchant renders the payment page twice
  therefore holds two live ways to pay one order, and sometimes uses both.

  Both callbacks are true. The money moved twice. paygate cannot undo the second
  charge by refusing to believe it, and it must not settle the order twice
  either, so there are exactly three things to do: settle once, record the
  second as a duplicate, and **give the money back**.

  ECPay's own WooCommerce plugin stops one step short of that — it flags the order
  in the admin list with a red warning and a "mark as handled" button, and the shop
  owner refunds by hand. paygate refunds automatically because it already has
  the refund path the merchant API uses (refunds.feature), and because a
  duplicate charge that waits for somebody to notice is a chargeback.

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
    # One order, and a customer who pays for it twice: two forms, two provider
    # order numbers, two cards typed, two outcomes queued.
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "dup-fixture"
        },
        "body": {
          "merchant_trade_no": "ACME-DUP-1",
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
    And the payment form is submitted to the payment provider
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
    # The merchant's page is rendered again, and the customer pays again.
    And the payment form is submitted to the payment provider
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
    And payment provider received 2 checkout requests
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payment_attempts
       WHERE payment_id = '{paymentId}'
      HAVING count(*) = 2 AND count(DISTINCT provider_trade_no) = 2;
      """

  Scenario: The first payment settles the order and the second is recorded as a duplicate
    When payment provider delivers each pending callback 1 time
    Then exactly 2 responses are 200
    # BOTH are acknowledged. Refusing the second would only make the provider
    # send it again, and it is not wrong — it is inconvenient.
    And exactly 2 callbacks were acknowledged
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM provider_events
       WHERE payment_id = '{paymentId}' AND outcome = 'applied' HAVING count(*) = 1;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM provider_events
       WHERE payment_id = '{paymentId}' AND outcome = 'duplicate' HAVING count(*) = 1;
      """
    # The order is paid once, for what it cost.
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments
       WHERE id = '{paymentId}' AND status = 'succeeded' AND amount = 2500;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payment_events
       WHERE payment_id = '{paymentId}' AND event_type = 'PaymentSucceeded' HAVING count(*) = 1;
      """
    # And the second charge is a fact in the audit log, not a footnote in a log
    # file: it names the attempt that took the money.
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payment_events e
       WHERE e.payment_id = '{paymentId}'
         AND e.event_type = 'PaymentDuplicatePaid'
         AND (e.payload->>'amount')::bigint = 2500
         AND EXISTS (
               SELECT 1 FROM payment_attempts a
                WHERE a.id::text = e.payload->>'attempt_id'
                  AND a.payment_id = '{paymentId}'
             )
      HAVING count(*) = 1;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payment_attempts
       WHERE payment_id = '{paymentId}' AND status = 'succeeded' HAVING count(*) = 2;
      """

  Scenario: The duplicate charge is given back, and it is the second one that is refunded
    When payment provider delivers each pending callback 1 time
    Then exactly 2 callbacks were acknowledged
    When background work has settled
    # One refund, at the provider, for the attempt that should not have paid.
    Then payment provider received 1 refund request
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM refunds
       WHERE payment_id = '{paymentId}'
         AND amount = 2500
         AND status = 'succeeded'
         AND reason = 'duplicate'
      HAVING count(*) = 1;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM refunds r
       WHERE r.payment_id = '{paymentId}'
         AND r.attempt_id = (
               SELECT a.id FROM payment_attempts a
                WHERE a.payment_id = '{paymentId}'
                ORDER BY a.started_at DESC
                LIMIT 1
             );
      """
    # The ORDER was never over-refunded, because that money was never the
    # order's. `amount_refunded` is what the merchant may still give back.
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments
       WHERE id = '{paymentId}' AND status = 'succeeded' AND amount_refunded = 0;
      """
    # So the merchant can still refund the payment itself, in full.
    When POST /api/v1/payments/{paymentId}/refunds:
      """json
      {
        "headers": { "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc", "Idempotency-Key": "after-dup" },
        "body": { "amount": 2500 }
      }
      """
    Then response status is 201
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments WHERE id = '{paymentId}' AND amount_refunded = 2500 AND status = 'refunded';
      """

  Scenario: The merchant is told once, about the payment, and not about the duplicate
    When payment provider delivers each pending callback 1 time
    Then exactly 2 callbacks were acknowledged
    When background work has settled
    Then merchant received 1 notification at "/demo-merchant/api/notify"
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM notifications
       WHERE payment_id = '{paymentId}' AND delivered_at IS NOT NULL HAVING count(*) = 1;
      """
    # The merchant sold one thing and is owed one callback. The duplicate is
    # paygate's problem with the provider, and telling the shop about it would
    # only invite it to ship twice or refund twice.
    And in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM notifications WHERE payload->>'RtnMsg' = 'duplicate';
      """

  Scenario: With automatic refunds switched off, the duplicate is still recorded and still visible
    # ECPay's plugin behaviour, kept as a switch so the difference is a
    # configuration rather than a fork: record it, alert, and leave the money
    # where it is for somebody to deal with.
    Given in PostgreSQL:
      """sql
      UPDATE merchants SET duplicate_auto_refund = false WHERE id = 1;
      """
    When payment provider delivers each pending callback 1 time
    Then exactly 2 callbacks were acknowledged
    When background work has settled
    Then payment provider received 0 refund requests
    And in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM refunds;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM provider_events
       WHERE payment_id = '{paymentId}' AND outcome = 'duplicate' HAVING count(*) = 1;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payment_events
       WHERE payment_id = '{paymentId}' AND event_type = 'PaymentDuplicatePaid' HAVING count(*) = 1;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments WHERE id = '{paymentId}' AND status = 'succeeded' AND amount_refunded = 0;
      """

  Scenario: When the automatic refund itself fails at the provider, the duplicate stays recorded
    # ECPay's refund API can refuse service like any other call — the mock's
    # amount 119 always answers 503 (spec.md, "The payment provider mock"), the
    # same amount refunds.feature uses to prove a merchant-initiated refund
    # changes nothing on a provider failure. There is no merchant waiting on
    # THIS refund the way there is for `POST /refunds`, so the failure cannot
    # become the caller's problem the way `502 PROVIDER_UNAVAILABLE` is — but it
    # must not quietly become nobody's problem either. `PaymentDuplicatePaid`
    # already named the money and the attempt that took it, in the audit log,
    # before any refund was attempted, so the duplicate itself is not lost even
    # though the refund of it did not go through.
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "dup-refund-fails"
        },
        "body": {
          "merchant_trade_no": "ACME-DUP-REFUND-FAIL",
          "amount": 119,
          "currency": "USD",
          "item_desc": "Beans",
          "notify_url": "/demo-merchant/api/notify",
          "client_back_url": "/shop/result"
        }
      }
      """
    Then response status is 201
    And save response body field "id" as "failRefundId"
    And the payment form is submitted to the payment provider
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
    And the payment form is submitted to the payment provider
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
    Then exactly 2 callbacks were acknowledged
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payment_events
       WHERE payment_id = '{failRefundId}' AND event_type = 'PaymentDuplicatePaid' HAVING count(*) = 1;
      """
    When background work has settled
    # The provider was asked, and it refused — exactly as it does for a
    # merchant-initiated refund of 119 (refunds.feature).
    Then payment provider received 1 refund request
    And in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM refunds WHERE payment_id = '{failRefundId}' AND status = 'succeeded';
      """
    # The order was never over-refunded by a charge that was never its money,
    # whether or not giving it back succeeded.
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments
       WHERE id = '{failRefundId}' AND status = 'succeeded' AND amount_refunded = 0;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM provider_events
       WHERE payment_id = '{failRefundId}' AND outcome = 'duplicate' HAVING count(*) = 1;
      """
    # The refund is on the books as owed, not as done and not as forgotten.
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM refunds
       WHERE payment_id = '{failRefundId}' AND status = 'pending' AND reason = 'duplicate'
      HAVING count(*) = 1;
      """

  Scenario: A duplicate refund the provider refused is sent again, and lands once
    # The money would otherwise sit at the provider with nothing in this system
    # ever going back for it. The reconciler already exists to chase things nobody
    # answered for, and sending a refund twice is safe for the same reason a
    # merchant's own retry is: the provider deduplicates on the refund id paygate
    # chose (spec.md, "Refunds").
    Given in PostgreSQL:
      """sql
      INSERT INTO refunds (id, payment_id, attempt_id, merchant_id, amount, status, reason, created_at)
      SELECT '01931c4f-0000-7000-8000-0000000000d1', p.id, a.id, p.merchant_id, 2500, 'pending', 'duplicate',
             NOW() - INTERVAL '61 minutes'
        FROM payments p
        JOIN payment_attempts a ON a.payment_id = p.id
       WHERE p.merchant_trade_no = 'ACME-DUP-1'
       ORDER BY a.started_at DESC
       LIMIT 1;
      """
    When the reconciler runs
    Then payment provider received 1 refund request
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM refunds
       WHERE id = '01931c4f-0000-7000-8000-0000000000d1' AND status = 'succeeded';
      """
    # Sending it again did not refund the order twice, and did not touch what the
    # merchant may still give back of its own accord.
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM refunds WHERE payment_id = (
        SELECT id FROM payments WHERE merchant_trade_no = 'ACME-DUP-1'
      ) HAVING count(*) = 1;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments WHERE merchant_trade_no = 'ACME-DUP-1' AND amount_refunded = 0;
      """
