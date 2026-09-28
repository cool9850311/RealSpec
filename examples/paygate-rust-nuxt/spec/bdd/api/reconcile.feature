@reconcile
Feature: Asking the provider instead of guessing

  A callback can simply not arrive. The customer paid, the provider sent it, and
  something between them and here dropped it — or paygate answered something
  other than `1|OK` five times and the provider gave up. The order is then still
  `pending` with an outstanding attempt and nothing coming, and the temptation is
  to decide locally that it was never paid.

  **That decision is not paygate's to make.** ECPay documents the recovery path
  and its own WooCommerce plugin implements it: wait, then call
  `QueryTradeInfo`, and act only on what comes back — `TradeStatus=1` it was
  paid, `10200095` the consumer never completed it, `0` it is still in progress.
  The plugin cancels an order only on `10200095`, and so does this.

  The reconciler is therefore the second way an order can be settled, and the
  only way one can be given up. Both write the same rows as a callback does, and
  the two cannot both win: whichever commits first leaves the other with nothing
  to do (webhooks.feature, "apply once").

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
    # An order whose customer went to the provider, typed a card, and about whom
    # nothing has come back. The callback is queued at the mock and stays there.
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "reconcile-fixture"
        },
        "body": {
          "merchant_trade_no": "ACME-RECON-1",
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
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments WHERE id = '{paymentId}' AND status = 'pending';
      """

  Scenario: A callback that never arrived is found by asking, and the payment settles late
    # The money moved and paygate did not know. Everything a callback would have
    # written gets written now, including what the merchant is owed.
    Given in PostgreSQL:
      """sql
      UPDATE payment_attempts SET started_at = NOW() - INTERVAL '61 minutes'
       WHERE payment_id = '{paymentId}';
      """
    And payment provider answers the next query with trade status "1"
    When the reconciler runs
    Then in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM provider_queries q
        JOIN payment_attempts a ON a.id = q.attempt_id
       WHERE a.payment_id = '{paymentId}' AND q.trade_status = '1'
      HAVING count(*) = 1;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments WHERE id = '{paymentId}' AND status = 'succeeded';
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payment_attempts
       WHERE payment_id = '{paymentId}' AND status = 'succeeded' AND queried_at IS NOT NULL;
      """
    # The audit row says HOW it was learned, because "the provider told us" and
    # "we went and asked" are different facts about the same payment.
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payment_events
       WHERE payment_id = '{paymentId}'
         AND event_type = 'PaymentReconciled'
         AND payload->>'source' = 'query'
      HAVING count(*) = 1;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM notifications WHERE payment_id = '{paymentId}' HAVING count(*) = 1;
      """
    When background work has settled
    Then merchant received 1 notification at "/demo-merchant/api/notify"

  Scenario: Only the provider may end an attempt
    # `10200095` — the order was never created, the consumer never completed
    # payment. The single answer that permits
    # paygate to give up, and it came from the only party that knows.
    Given in PostgreSQL:
      """sql
      UPDATE payment_attempts SET started_at = NOW() - INTERVAL '61 minutes'
       WHERE payment_id = '{paymentId}';
      """
    And payment provider answers the next query with trade status "10200095"
    When the reconciler runs
    Then in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payment_attempts
       WHERE payment_id = '{paymentId}' AND status = 'abandoned' HAVING count(*) = 1;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments WHERE id = '{paymentId}' AND status = 'pending';
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payment_events
       WHERE payment_id = '{paymentId}' AND event_type = 'PaymentAttemptAbandoned';
      """
    # Nothing happened, so there is nothing to tell the merchant.
    And in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM notifications;
      """
    # And the order is payable again, at a number the provider has not seen —
    # which is only a fact once something asks for one. ECPay would refuse the
    # abandoned number a second time; nothing here has spent it, because
    # nothing here ever paid it.
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "reconcile-retry"
        },
        "body": {
          "merchant_trade_no": "ACME-RECON-1",
          "amount": 2500,
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

  Scenario: A genuine success that outruns the reconciler still settles the order
    # `10200095` is the provider's best answer at the moment it is asked, not a
    # promise that nothing will ever arrive. Abandoning the attempt returns the
    # order to `pending` — the exact state a callback settles from — and the
    # race between a callback and a reconciliation is guarded by
    # `UPDATE … WHERE status = 'pending'` on the PAYMENT (spec.md,
    # "Concurrency"), not by the attempt's own status. So giving up on an
    # attempt does not poison it against a callback that turns out to be real:
    # the card typed in the Background queued its outcome at the provider mock,
    # and that queue is a different control surface from `QueryTradeInfo` —
    # asking the provider one way does not empty what it is separately holding
    # to report the other way.
    Given in PostgreSQL:
      """sql
      UPDATE payment_attempts SET started_at = NOW() - INTERVAL '61 minutes'
       WHERE payment_id = '{paymentId}';
      """
    And payment provider answers the next query with trade status "10200095"
    When the reconciler runs
    Then in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payment_attempts
       WHERE payment_id = '{paymentId}' AND status = 'abandoned' HAVING count(*) = 1;
      """
    When payment provider delivers each pending callback 1 time
    Then exactly 1 response is 200
    And exactly 1 callback was acknowledged
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments WHERE id = '{paymentId}' AND status = 'succeeded';
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payment_attempts
       WHERE payment_id = '{paymentId}' AND status = 'succeeded' HAVING count(*) = 1;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payment_events
       WHERE payment_id = '{paymentId}' AND event_type = 'PaymentSucceeded' HAVING count(*) = 1;
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

  Scenario: An answer of "still in progress" changes nothing and is asked again later
    Given in PostgreSQL:
      """sql
      UPDATE payment_attempts SET started_at = NOW() - INTERVAL '61 minutes'
       WHERE payment_id = '{paymentId}';
      """
    And payment provider answers the next query with trade status "0"
    When the reconciler runs
    Then in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments WHERE id = '{paymentId}' AND status = 'pending';
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payment_attempts
       WHERE payment_id = '{paymentId}' AND status = 'redirected' AND queried_at IS NOT NULL;
      """
    And in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM payment_events WHERE event_type IN ('PaymentReconciled', 'PaymentAttemptAbandoned');
      """
    # Asked once, and it will be asked again — but not immediately, because
    # querying too fast is how ECPay answers 403 and stops answering for half an
    # hour (spec.md, "The payment provider mock").
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM provider_queries HAVING count(*) = 1;
      """
    Given in PostgreSQL:
      """sql
      UPDATE payment_attempts SET queried_at = NOW() - INTERVAL '61 minutes'
       WHERE payment_id = '{paymentId}';
      """
    And payment provider answers the next query with trade status "1"
    When the reconciler runs
    Then in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM provider_queries HAVING count(*) = 2;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments WHERE id = '{paymentId}' AND status = 'succeeded';
      """

  Scenario: An answer nobody could have signed is not an answer
    # The query is the same trust boundary as the callback, pointing the other
    # way. A reconciler that believes whatever comes back has handed anyone who
    # can reach the provider's address a way to settle other people's orders.
    Given in PostgreSQL:
      """sql
      UPDATE payment_attempts SET started_at = NOW() - INTERVAL '61 minutes'
       WHERE payment_id = '{paymentId}';
      """
    And payment provider answers the next query as forged
    When the reconciler runs
    Then in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments WHERE id = '{paymentId}' AND status = 'pending';
      """
    And in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM payment_events
       WHERE event_type IN ('PaymentReconciled', 'PaymentAttemptAbandoned');
      """
    And in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM notifications;
      """
    # An unusable answer is worth exactly as much as no answer: the attempt is
    # still outstanding, and it will be asked about again.
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payment_attempts
       WHERE payment_id = '{paymentId}' AND status = 'redirected';
      """
    Given in PostgreSQL:
      """sql
      UPDATE payment_attempts SET queried_at = NOW() - INTERVAL '61 minutes'
       WHERE payment_id = '{paymentId}';
      """
    And payment provider answers the next query with trade status "1"
    When the reconciler runs
    Then in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments WHERE id = '{paymentId}' AND status = 'succeeded';
      """

  Scenario: A provider that has had enough of being asked is left alone for a while
    # ECPay answers 403 to a caller that queries too often and then stops answering
    # at all for half an hour. A reconciler that treated that as "not paid" would
    # cancel a shop's whole afternoon; one that hammered on would extend its own
    # blackout.
    Given in PostgreSQL:
      """sql
      UPDATE payment_attempts SET started_at = NOW() - INTERVAL '61 minutes'
       WHERE payment_id = '{paymentId}';
      """
    And payment provider answers the next query as throttled
    When the reconciler runs
    Then in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments WHERE id = '{paymentId}' AND status = 'pending';
      """
    And in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM payment_events
       WHERE event_type IN ('PaymentReconciled', 'PaymentAttemptAbandoned');
      """
    # The refusal is recorded as an asking, so the throttle applies to us too and
    # the next pass waits rather than queueing up another 403.
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM provider_queries HAVING count(*) = 1;
      """
    When the reconciler runs
    Then in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM provider_queries HAVING count(*) = 1;
      """

  Scenario: An attempt younger than the window is not asked about at all
    # ECPay's own plugin waits an hour on a credit card before it asks. Asking
    # early would query every payment in flight, and the customer is probably
    # still on the bank's page.
    Given payment provider answers the next query with trade status "1"
    When the reconciler runs
    Then in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM provider_queries;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments WHERE id = '{paymentId}' AND status = 'pending';
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payment_attempts
       WHERE payment_id = '{paymentId}' AND queried_at IS NULL;
      """

  Scenario: A payment that already settled is never asked about
    When payment provider delivers each pending callback 1 time
    Then exactly 1 callback was acknowledged
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments WHERE id = '{paymentId}' AND status = 'succeeded';
      """
    Given in PostgreSQL:
      """sql
      UPDATE payment_attempts SET started_at = NOW() - INTERVAL '61 minutes'
       WHERE payment_id = '{paymentId}';
      """
    When the reconciler runs
    Then in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM provider_queries;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payment_events
       WHERE payment_id = '{paymentId}' AND event_type = 'PaymentSucceeded' HAVING count(*) = 1;
      """

  Scenario: The query and the callback both arrive, and the payment settles once
    # The race the reconciler creates by existing. Both paths write the same
    # rows, so both must go through the same conditional update, and the loser
    # must be able to tell that it lost.
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
    # The callback that was queued all along now turns up.
    When payment provider delivers each pending callback 1 time
    Then exactly 1 callback was acknowledged
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM provider_events
       WHERE payment_id = '{paymentId}' AND outcome = 'no_op' HAVING count(*) = 1;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payment_events
       WHERE payment_id = '{paymentId}'
         AND event_type IN ('PaymentSucceeded', 'PaymentReconciled')
      HAVING count(*) = 1;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM notifications WHERE payment_id = '{paymentId}' HAVING count(*) = 1;
      """
    When background work has settled
    Then merchant received 1 notification at "/demo-merchant/api/notify"

  Scenario: A second charge nobody told paygate about is found by asking, and given back
    # Every duplicate in duplicate_payment.feature announces itself with a second
    # callback. This is the same duplicate with the announcement lost, which is
    # the case that actually needs a reconciler: the only way this charge is ever
    # found is paygate asking about an attempt nobody closed, and being told the
    # money moved. Finding it and then doing nothing would be worse than never
    # asking — the customer is charged twice AND there is a row proving paygate
    # knew.
    # The merchant's page is rendered a second time while the order is still
    # payable, which is the only time it can be (handoff.feature, "A paid order
    # is not handed over again"). That second form is the one the customer uses
    # at the provider and about which NOTHING comes back — no callback, no
    # redirect, nothing but an attempt left open.
    When the payment form is submitted to the payment provider
    Then response status is 200
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payment_attempts
       WHERE payment_id = '{paymentId}' AND status = 'redirected'
      HAVING count(*) = 2 AND count(DISTINCT provider_trade_no) = 2;
      """
    # The FIRST attempt's callback does arrive, and settles the order honestly.
    When payment provider delivers each pending callback 1 time
    Then exactly 1 callback was acknowledged
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments
       WHERE id = '{paymentId}' AND status = 'succeeded' AND amount_refunded = 0;
      """
    Given in PostgreSQL:
      """sql
      UPDATE payment_attempts SET started_at = NOW() - INTERVAL '61 minutes'
       WHERE payment_id = '{paymentId}' AND status = 'redirected';
      """
    And payment provider answers the next query with trade status "1"
    When the reconciler runs
    # Recorded exactly as a callback-announced duplicate is, and against the same
    # attempt: the order is settled once, for what it cost, and the second charge
    # was never this order's money.
    Then in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payment_events
       WHERE payment_id = '{paymentId}' AND event_type = 'PaymentDuplicatePaid'
      HAVING count(*) = 1;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments
       WHERE id = '{paymentId}' AND status = 'succeeded' AND amount_refunded = 0;
      """
    # And given back, against the second attempt's own provider trade number —
    # the reconciler queues the refund `pending` and its own second job sends it,
    # which is the same machinery a duplicate refund that failed goes through
    # (duplicate_payment.feature, "sent again, and lands once").
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM refunds r
        JOIN payment_attempts a ON a.id = r.attempt_id
       WHERE r.payment_id = '{paymentId}'
         AND r.reason = 'duplicate'
         AND r.status = 'succeeded'
         AND r.amount = 2500
         AND a.status = 'succeeded'
      HAVING count(*) = 1;
      """
    And payment provider received 1 refund request
    # Told once, about the payment. A merchant told about the duplicate would
    # ship twice, and the duplicate is paygate's problem with the provider.
    When background work has settled
    Then merchant received 1 notification at "/demo-merchant/api/notify"

  Scenario: With automatic refunds switched off, a duplicate found by asking is still recorded
    # The same preference the callback path reads (duplicate_payment.feature,
    # "With automatic refunds switched off"). Which of the two found the charge
    # cannot be what decides whether the merchant's choice is honoured.
    Given in PostgreSQL:
      """sql
      UPDATE merchants SET duplicate_auto_refund = false WHERE id = 1;
      """
    When the payment form is submitted to the payment provider
    Then response status is 200
    When payment provider delivers each pending callback 1 time
    Then exactly 1 callback was acknowledged
    Given in PostgreSQL:
      """sql
      UPDATE payment_attempts SET started_at = NOW() - INTERVAL '61 minutes'
       WHERE payment_id = '{paymentId}' AND status = 'redirected';
      """
    And payment provider answers the next query with trade status "1"
    When the reconciler runs
    # The fact is written whatever the setting says. What the setting decides is
    # only who gives the money back.
    Then in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payment_events
       WHERE payment_id = '{paymentId}' AND event_type = 'PaymentDuplicatePaid'
      HAVING count(*) = 1;
      """
    And in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM refunds;
      """
    And payment provider received 0 refund requests
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments
       WHERE id = '{paymentId}' AND status = 'succeeded' AND amount_refunded = 0;
      """

  Scenario: The lost callback turns up while the merchant is refunding, and both are honoured
    # Two commands need the same two rows here — the order's and the attempt's —
    # and they reach them from opposite ends: a refund starts from the order, a
    # callback starts from the provider's trade number and has to find the order
    # it belongs to. Taken in two different orders those two lock each other out
    # (PostgreSQL's own `40P01`), and what a merchant would see is a refund that
    # failed for no reason, or a callback the provider goes on resending because
    # nobody ever answered it. Neither is allowed to happen, so this asks for
    # both at one instant.
    # The order settles by ASKING, which is what leaves the callback still
    # queued at the provider: it was never delivered, so it is still owed.
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
    When these things happen at one instant:
      """json
      [
        {
          "method": "POST",
          "path": "/api/v1/payments/{paymentId}/refunds",
          "headers": {
            "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
            "Idempotency-Key": "refund-vs-callback-1"
          },
          "body": { "amount": 1000 }
        },
        {
          "method": "POST",
          "path": "/api/v1/payments/{paymentId}/refunds",
          "headers": {
            "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
            "Idempotency-Key": "refund-vs-callback-2"
          },
          "body": { "amount": 500 }
        },
        {"step": "payment provider delivers each pending callback 2 times"}
      ]
      """
    # Four answers in one set: both refunds, and both copies of the callback.
    Then exactly 2 responses are 201
    And exactly 2 responses are 200
    And exactly 2 callbacks were acknowledged
    # The callback changed nothing — the query had already settled this order,
    # and a second settlement is refused by the same `WHERE status = 'pending'`
    # that refuses one callback racing another (webhooks.feature, "apply once").
    # ONE row for two copies: they carry one event id, and `provider_events`'s
    # own primary key is what makes a re-delivery a no-op that is still
    # answered.
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM provider_events
       WHERE payment_id = '{paymentId}' AND outcome = 'no_op' HAVING count(*) = 1;
      """
    # And both refunds are on the books, once each, for what was asked.
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM refunds
       WHERE payment_id = '{paymentId}' AND status = 'succeeded'
      HAVING count(*) = 2 AND sum(amount) = 1500;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments
       WHERE id = '{paymentId}' AND status = 'succeeded' AND amount_refunded = 1500;
      """
    And payment provider received 2 refund requests

  Scenario: A reconciliation pass and the duplicate's own callback reach the same attempt at once
    # The pass holds its claim for as long as the provider takes to answer — a
    # round trip, not an instant — and only then writes what the answer meant.
    # If that claim were taken on the attempt rather than on the order, the
    # callback for the SAME attempt, which takes the order first, would be
    # holding exactly what the pass is about to want while waiting for exactly
    # what the pass already holds. `888` is the amount whose query answer the
    # provider holds for 800 ms, so this asks for that window on purpose rather
    # than hoping for it.
    # A second order, paid twice: the customer leaves the second payment page
    # open, and uses it after the first payment has already settled the order —
    # which is how a duplicate happens, and why neither the callback nor the
    # pass can be the only one allowed to find it.
    When POST /api/v1/payments:
      """json
      {
        "headers": {
          "Authorization": "Bearer sk_test_acme_4eC39HqLyjWDarjtT1zdp7dc",
          "Idempotency-Key": "recon-race-fixture"
        },
        "body": {
          "merchant_trade_no": "ACME-RECON-RACE",
          "amount": 888,
          "currency": "USD",
          "item_desc": "Beans",
          "notify_url": "/demo-merchant/api/notify",
          "client_back_url": "/shop/result"
        }
      }
      """
    Then response status is 201
    And save response body field "id" as "slowId"
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
    # The second page is rendered while the order is still payable — the only
    # time it can be — and left open.
    And the payment form is submitted to the payment provider
    Then response status is 200
    # Both orders' first callbacks arrive and settle them.
    When payment provider delivers each pending callback 1 time
    Then exactly 2 callbacks were acknowledged
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments WHERE id = '{slowId}' AND status = 'succeeded';
      """
    # Only now does the customer pay on the page they left open. Its callback is
    # queued, and the attempt it belongs to is still outstanding.
    When the customer pays at the payment provider:
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
       WHERE payment_id = '{slowId}' AND status = 'redirected';
      """
    And payment provider answers the next query with trade status "1"
    When these things happen at one instant:
      """json
      [
        {"step": "the reconciler runs"},
        {"step": "payment provider delivers each pending callback 1 time"}
      ]
      """
    # Whichever of the two got there first wrote it; the other found it written
    # and did nothing, the same rule a callback racing a callback lives by.
    Then exactly 1 callback was acknowledged
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payment_events
       WHERE payment_id = '{slowId}' AND event_type = 'PaymentDuplicatePaid'
      HAVING count(*) = 1;
      """
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM refunds
       WHERE payment_id = '{slowId}' AND reason = 'duplicate'
      HAVING count(*) = 1;
      """
    # The order kept its own money: a duplicate's refund is not the order's.
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments
       WHERE id = '{slowId}' AND status = 'succeeded' AND amount_refunded = 0;
      """
