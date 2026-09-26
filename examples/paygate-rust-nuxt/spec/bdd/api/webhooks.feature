@webhooks
Feature: Callbacks from the payment provider

  This is the endpoint the whole system turns on. Everything before it is an
  intention — an order, a hand-off, a customer who went somewhere — and this is
  the one thing that makes a payment real. It is also the protocol paygate is
  held to from above: ECPay's `ReturnURL` contract, with paygate in the seat its
  own merchants sit in one level down (notify.feature).

  Three rules, and each has a scenario that removes it:

    **Verify first.** A body nobody signed with this provider's key changes
    nothing at all.
    **Acknowledge after committing.** The answer is the body `1|OK`, written
    only once the transaction has committed. Anything else — a refusal, a
    crash, a process that was not there — leaves the provider holding the
    callback, and it comes again. That is the recovery path for every failure
    on this endpoint, including paygate being restarted mid-payment.
    **Apply once.** Delivery is at-least-once, so the same callback arrives
    twice, or four times at one instant across three replicas, and settles one
    payment. `provider_events` has the provider's event id as its primary key.

  There is also a rule that only reading the real thing teaches: **`RtnCode` is
  an open set.** `1` is paid, `10100058` and `10200163` are failures, `10300066` means
  "payment result pending confirmation, do not ship", and ECPay has added more
  over the years. A code this
  gateway does not know is recorded, acknowledged, and acted on in no way — the
  one behaviour that is safe for a code whose meaning arrives after the code
  does. `SimulatePaid=1`, which ECPay's back office sends when somebody presses
  its confirm-test-payment button, is the same kind of trap in the other
  direction: it looks
  exactly like a payment and is not one.

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
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "webhook-fixture"
        },
        "body": {
          "merchant_trade_no": "ACME-WH-1",
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

  Scenario: A callback settles the payment, and is acknowledged only once it has
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
       WHERE payment_id = '{paymentId}' AND status = 'succeeded' AND provider_charge_id IS NOT NULL;
      """
    # The record of having received it, the outcome, the audit row and the
    # merchant's notification are one transaction. `1|OK` came after all of it.
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM provider_events
       WHERE provider_code = 'ecpay' AND payment_id = '{paymentId}' AND outcome = 'applied'
      HAVING count(*) = 1;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payment_events
       WHERE payment_id = '{paymentId}' AND event_type = 'PaymentSucceeded' HAVING count(*) = 1;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM notifications WHERE payment_id = '{paymentId}' HAVING count(*) = 1;
      """

  Scenario: The same callback delivered three times settles one payment and owes one notification
    # At-least-once is not a flaw to be tolerated, it is the contract. A
    # provider that gets no answer sends again, and so does one whose answer was
    # lost on the way back — the second is indistinguishable from the first.
    When payment provider delivers each pending callback 3 times
    Then exactly 3 responses are 200
    And exactly 3 callbacks were acknowledged
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM provider_events
       WHERE payment_id = '{paymentId}' AND outcome = 'applied' HAVING count(*) = 1;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payment_events
       WHERE payment_id = '{paymentId}' AND event_type = 'PaymentSucceeded' HAVING count(*) = 1;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM notifications WHERE payment_id = '{paymentId}' HAVING count(*) = 1;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments WHERE id = '{paymentId}' AND status = 'succeeded' AND amount_refunded = 0;
      """

  Scenario Outline: Four copies of one callback arriving at one instant settle it once
    When payment provider delivers each pending callback 4 times
    Then exactly 4 responses are 200
    And exactly 4 callbacks were acknowledged
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payment_events
       WHERE payment_id = '{paymentId}' AND event_type = 'PaymentSucceeded' HAVING count(*) = 1;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM notifications WHERE payment_id = '{paymentId}' HAVING count(*) = 1;
      """

    # The column is deliberately unused: a race is only worth asserting if the
    # same race is run more than once, and the order this one runs against comes
    # from the Background, which an Examples row cannot reach.
    Examples: the same instant, three times
      | run |
      | 1   |
      | 2   |
      | 3   |

  Scenario: A callback nobody signed with this provider's key changes nothing, and is sent again
    When payment provider delivers each pending callback 1 time, as forged
    Then exactly 1 callback was refused
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments WHERE id = '{paymentId}' AND status = 'pending';
      """
    And in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM provider_events;
      """
    And in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM payment_events WHERE event_type = 'PaymentSucceeded';
      """
    And in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM notifications;
      """
    # Refused means still owed: the provider has it, and the real one arrives.
    When payment provider delivers each pending callback 1 time
    Then exactly 1 callback was acknowledged
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments WHERE id = '{paymentId}' AND status = 'succeeded';
      """

  Scenario: A callback for an order paygate has no record of is refused rather than invented
    Given in PostgreSQL:
      """sql
      DELETE FROM payment_attempts WHERE payment_id = '{paymentId}';
      """
    When payment provider delivers each pending callback 1 time
    Then exactly 1 callback was refused
    And in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM provider_events;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments WHERE id = '{paymentId}' AND status = 'pending';
      """
    And in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM notifications;
      """

  Scenario: A return code this gateway has never seen is recorded and acted on in no way
    # 10300066 — "payment result pending confirmation, do not ship". Not a
    # success, not a failure, and
    # not something to guess about. Acknowledged, because it arrived; ignored,
    # because acting on it would mean inventing its meaning.
    When payment provider delivers each pending callback 1 time, as return code "10300066"
    Then exactly 1 response is 200
    And exactly 1 callback was acknowledged
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM provider_events
       WHERE payment_id = '{paymentId}' AND outcome = 'unknown_code' HAVING count(*) = 1;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payment_attempts
       WHERE payment_id = '{paymentId}' AND status = 'redirected' AND rtn_code = 10300066;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments WHERE id = '{paymentId}' AND status = 'pending';
      """
    And in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM payment_events
       WHERE event_type IN ('PaymentSucceeded', 'PaymentAttemptFailed');
      """
    And in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM notifications;
      """

  Scenario: A simulated payment is not a payment
    # ECPay's back office can send a notification that is identical to a real one
    # except for `SimulatePaid=1`. A gateway that ships on it ships for free.
    When payment provider delivers each pending callback 1 time, as simulated
    Then exactly 1 callback was acknowledged
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payment_attempts
       WHERE payment_id = '{paymentId}' AND simulate_paid = true AND status = 'redirected';
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments WHERE id = '{paymentId}' AND status = 'pending';
      """
    And in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM payment_events WHERE event_type = 'PaymentSucceeded';
      """
    And in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM notifications;
      """

  Scenario: A callback whose amount is not the order's is refused, not reconciled away
    # The one case where being loud is the whole job. Either the order was
    # tampered with on its way to the provider, or two systems disagree about
    # what was bought; settling either way is worse than settling nothing.
    When payment provider delivers each pending callback 1 time, as a wrong amount
    Then exactly 1 callback was refused
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM provider_events
       WHERE payment_id = '{paymentId}' AND outcome = 'amount_mismatch' HAVING count(*) = 1;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments
       WHERE id = '{paymentId}' AND status = 'pending' AND amount = 2500;
      """
    And in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM notifications;
      """
    # Refused means the provider has it still, so the honest one can arrive.
    When payment provider delivers each pending callback 1 time
    Then exactly 1 callback was acknowledged
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments WHERE id = '{paymentId}' AND status = 'succeeded';
      """

  Scenario: A callback that contradicts an already-settled order is a conflict, not a second failure
    # A second attempt of the Background's order, asked for again and
    # declined. Its callback is queued behind the Background's own (still
    # undelivered) success, so delivering the queue settles the order first and
    # only then hands this one a payment that is not pending any more. It
    # cannot become an ordinary failed attempt — that transition belongs to a
    # still-pending order — and it is not a duplicate PAYMENT either, because
    # nothing was charged. spec.md, "Being told by the provider": "A callback
    # that contradicts a settled order changes nothing and is recorded with
    # outcome = 'conflict'; a successful one for a different attempt of a
    # settled order is duplicate, not conflict." This is the other half of
    # that sentence — an unsuccessful one.
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
    When payment provider delivers each pending callback 1 time
    Then exactly 2 responses are 200
    And exactly 2 callbacks were acknowledged
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments WHERE id = '{paymentId}' AND status = 'succeeded';
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM provider_events
       WHERE payment_id = '{paymentId}' AND outcome = 'applied' HAVING count(*) = 1;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM provider_events
       WHERE payment_id = '{paymentId}' AND outcome = 'conflict' HAVING count(*) = 1;
      """
    # Changes nothing: the second attempt is left exactly as the hand-off left
    # it, not moved to 'failed' on a callback that arrived too late to matter.
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payment_attempts
       WHERE payment_id = '{paymentId}' AND status = 'redirected' HAVING count(*) = 1;
      """
    And in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM payment_events WHERE event_type = 'PaymentAttemptFailed';
      """
    # One outcome happened to this order, so the merchant is owed exactly one
    # notification, not two.
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM notifications WHERE payment_id = '{paymentId}' HAVING count(*) = 1;
      """

  Scenario: SimulatePaid overrides even a genuine decline — it settles nothing either way
    # `SimulatePaid=1` is set independently of `RtnCode`: the payment-provider
    # schema's own words are that `simulated` "sets SimulatePaid=1" — nothing
    # about changing the code underneath it, unlike the `return_code` variant,
    # which explicitly sends "the rtn_code below INSTEAD OF what the card
    # implied" (spec/openapi/payment-provider.yaml). So a card that would
    # decline can be delivered "as simulated" exactly as one that would
    # succeed. A guard written as "ignore it when RtnCode looks like success"
    # would let this one through as an ordinary decline instead — moving the
    # attempt to `failed` on a callback nobody's customer produced. The rule is
    # "acted on in no way", whatever the code underneath it says.
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "webhook-sim-decline"
        },
        "body": {
          "merchant_trade_no": "ACME-WH-SIMDECLINE",
          "amount": 2500,
          "currency": "USD",
          "item_desc": "Beans",
          "notify_url": "/demo-merchant/api/notify",
          "client_back_url": "/shop/result"
        }
      }
      """
    Then response status is 201
    And save response body field "id" as "simDeclineId"
    And the payment form is submitted to the payment provider
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
    # Two are pending — this one, and the Background's own still-undelivered
    # success — and "as simulated" applies to whatever it releases, so both
    # arrive stamped SimulatePaid=1. The Background's own is exactly the case
    # "A simulated payment is not a payment" already asserts above, so nothing
    # further is checked about it here; this scenario is only about the
    # declined one.
    When payment provider delivers each pending callback 1 time, as simulated
    Then exactly 2 callbacks were acknowledged
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payment_attempts
       WHERE payment_id = '{simDeclineId}' AND simulate_paid = true AND status = 'redirected';
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments WHERE id = '{simDeclineId}' AND status = 'pending';
      """
    And in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM payment_events
       WHERE payment_id = '{simDeclineId}'
         AND event_type IN ('PaymentSucceeded', 'PaymentAttemptFailed');
      """
    And in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM notifications WHERE payment_id = '{simDeclineId}';
      """

  Scenario: A payment survives paygate being restarted in the middle of it
    # The property that comes free from not being on the payment path. The
    # customer has paid; paygate is gone; the provider has an outcome and
    # nobody to give it to. Nothing may be lost by that.
    Given service "api" is stopped
    When payment provider delivers each pending callback 1 time
    Then exactly 1 callback was refused
    Given service "api" is started
    When payment provider delivers each pending callback 1 time
    Then exactly 1 response is 200
    And exactly 1 callback was acknowledged
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments WHERE id = '{paymentId}' AND status = 'succeeded';
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM provider_events
       WHERE payment_id = '{paymentId}' AND outcome = 'applied' HAVING count(*) = 1;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM notifications WHERE payment_id = '{paymentId}' HAVING count(*) = 1;
      """
    When background work has settled
    Then merchant received 1 notification at "/demo-merchant/api/notify"
