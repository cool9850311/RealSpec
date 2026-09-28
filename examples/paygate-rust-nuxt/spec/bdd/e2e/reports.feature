@e2e @reports
Feature: The daily report in the browser

  The report page reads the ClickHouse projection through the gateway, so what
  it shows arrives after the payments that caused it. Everything else on the
  dashboard reads PostgreSQL and is immediate. A scenario therefore never looks
  at the page before `background work has settled` — the same condition step the
  API features use — and the page is then asserted as a settled fact rather
  than polled for.

  History is seeded as rows of `payment_events`, the same append-only audit
  table the service writes, and travels relay → Kafka → ingester like
  everything else.

  Row testids: each day is a row `report-day-<yyyy-mm-dd>` (the family
  `report-day` counts them), each cell inside it is `day-<column>`, and the
  totals are `total-<column>`. Amounts are formatted in the merchant's currency
  for the active locale.

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
    And in PostgreSQL:
      """sql
      INSERT INTO dashboard_users (id, merchant_id, email, password_hash) VALUES
        (1, 1, 'owner@acme.test', '$2a$10$MwKxT13/lPxFMjvrkm0dL.QJuNll1zljDU.eOoRVvgrWF.kf/hyC2');
      """
    And locale is "en_US"

  Scenario: Seeded history is reported per merchant day, cut at Taipei's midnight
    Given in PostgreSQL:
      """sql
      INSERT INTO payment_events (event_id, payment_id, seq, merchant_id, event_type, payload, occurred_at) VALUES
        ('0192c000-0000-7000-8000-000000000001', '0192c000-0000-7000-8000-0000000000a1', 1, 1, 'PaymentCreated',
         '{"amount": 1000, "currency": "USD", "card_brand": "visa", "card_last4": "4242", "reference": "c1"}', '2026-09-17T15:59:58Z'),
        ('0192c000-0000-7000-8000-000000000002', '0192c000-0000-7000-8000-0000000000a1', 2, 1, 'PaymentSucceeded',
         '{"amount": 1000, "currency": "USD", "card_brand": "visa", "reference": "c1", "provider_charge_id": "ch_c1", "source": "callback"}', '2026-09-17T15:59:59Z'),
        ('0192c000-0000-7000-8000-000000000003', '0192c000-0000-7000-8000-0000000000a2', 1, 1, 'PaymentCreated',
         '{"amount": 2000, "currency": "USD", "card_brand": "visa", "card_last4": "4242", "reference": "c2"}', '2026-09-17T15:59:59Z'),
        ('0192c000-0000-7000-8000-000000000004', '0192c000-0000-7000-8000-0000000000a2', 2, 1, 'PaymentSucceeded',
         '{"amount": 2000, "currency": "USD", "card_brand": "visa", "reference": "c2", "provider_charge_id": "ch_c2", "source": "callback"}', '2026-09-17T16:00:00Z'),
        ('0192c000-0000-7000-8000-000000000005', '0192c000-0000-7000-8000-0000000000a3', 1, 1, 'PaymentCreated',
         '{"amount": 300, "currency": "USD", "card_brand": "visa", "card_last4": "0002", "reference": "c3"}', '2026-09-18T02:59:59Z'),
        ('0192c000-0000-7000-8000-000000000006', '0192c000-0000-7000-8000-0000000000a3', 2, 1, 'PaymentAttemptFailed',
         '{"amount": 300, "currency": "USD", "card_brand": "visa", "reference": "c3", "failure_code": "card_declined", "source": "callback"}', '2026-09-18T03:00:00Z'),
        ('0192c000-0000-7000-8000-000000000007', '0192c000-0000-7000-8000-0000000000a2', 3, 1, 'PaymentRefunded',
         '{"amount": 2000, "currency": "USD", "card_brand": "visa", "reference": "c2", "refund_id": "0192c000-0000-7000-8000-0000000000f1", "provider_refund_id": "re_c2"}', '2026-09-18T04:00:00Z');
      """
    And background work has settled
    And in ClickHouse query returns 1 row:
      """sql
      SELECT count() AS events FROM report_events FINAL WHERE merchant_id = 1 HAVING events = 4;
      """
    When visit "/login"
    And fill "textbox:@auth.email" with "owner@acme.test"
    And fill "textbox:@auth.password" with "secret123"
    And click "button:@auth.signIn"
    Then page URL is "/reports"
    When fill "textbox:@reports.from" with "2026-09-17"
    And fill "textbox:@reports.to" with "2026-09-18"
    And click "button:@reports.apply"
    Then network request "GET /api/v1/dashboard/reports/daily" responded 200
    And list "testid:report-day" has 2 rows
    And region "testid:report-day-2026-09-17" contains:
      """json
      {
        "testid:day-date":         "2026-09-17",
        "testid:day-succeeded":    "1",
        "testid:day-failed":       "0",
        "testid:day-gross":        "$10.00",
        "testid:day-refunded":     "$0.00",
        "testid:day-net":          "$10.00",
        "testid:day-success-rate": "100.00%"
      }
      """
    And region "testid:report-day-2026-09-18" contains:
      """json
      {
        "testid:day-succeeded":    "1",
        "testid:day-failed":       "1",
        "testid:day-gross":        "$20.00",
        "testid:day-refunded":     "$20.00",
        "testid:day-net":          "$0.00",
        "testid:day-success-rate": "50.00%"
      }
      """
    And text of "testid:total-net" is "$10.00"
    And text of "testid:total-success-rate" is "66.66%"
    And console has no errors

  Scenario: A payment taken at the cashier appears on today's report
    When visit "/shop"
    And fill "textbox:@shop.amount" with "12.50"
    And click "button:@shop.checkout"
    And click "button:@shop.pay"
    And fill "textbox:Card number" with "4242424242424242"
    And fill "textbox:Expiry" with "12/30"
    And fill "textbox:CVC" with "123"
    And click "button:Pay"
    And click "button:Authenticate"
    Then text of "testid:shop-result-status" is "Paid"
    When background work has settled
    Then in ClickHouse query returns 1 row:
      """sql
      SELECT 1 FROM report_events FINAL
       WHERE merchant_id = 1 AND event_type = 'PaymentSucceeded' AND amount = 1250;
      """
    When visit "/login"
    And fill "textbox:@auth.email" with "owner@acme.test"
    And fill "textbox:@auth.password" with "secret123"
    And click "button:@auth.signIn"
    Then page URL is "/reports"
    And list "testid:report-day" has 7 rows
    And text of "testid:total-succeeded" is "1"
    And text of "testid:total-gross" is "$12.50"
    And text of "testid:total-success-rate" is "100.00%"
    And console has no errors

  Scenario: A merchant with no activity sees zeros, not an error
    When visit "/login"
    And fill "textbox:@auth.email" with "owner@acme.test"
    And fill "textbox:@auth.password" with "secret123"
    And click "button:@auth.signIn"
    Then page URL is "/reports"
    And list "testid:report-day" has 7 rows
    And text of "testid:total-succeeded" is "0"
    And text of "testid:total-gross" is "$0.00"
    And text of "testid:total-success-rate" is "—"
    And "testid:report-unavailable" is hidden
    And console has no errors

  Scenario: A day with no rows inside an otherwise busy range renders as zero, not a gap
    # Different from the merchant-with-no-activity case above: here the RANGE
    # has real rows either side of the empty day, so a query that skipped
    # missing dates instead of filling them in would silently shrink the table
    # by one row rather than show it empty.
    Given in PostgreSQL:
      """sql
      INSERT INTO payment_events (event_id, payment_id, seq, merchant_id, event_type, payload, occurred_at) VALUES
        ('0192f000-0000-7000-8000-000000000001', '0192f000-0000-7000-8000-0000000000a1', 1, 1, 'PaymentCreated',
         '{"amount": 1000, "currency": "USD", "card_brand": "visa", "card_last4": "4242", "reference": "f1"}', '2026-09-19T02:00:00Z'),
        ('0192f000-0000-7000-8000-000000000002', '0192f000-0000-7000-8000-0000000000a1', 2, 1, 'PaymentSucceeded',
         '{"amount": 1000, "currency": "USD", "card_brand": "visa", "reference": "f1", "provider_charge_id": "ch_f1", "source": "callback"}', '2026-09-19T02:00:01Z');
      """
    And background work has settled
    When visit "/login"
    And fill "textbox:@auth.email" with "owner@acme.test"
    And fill "textbox:@auth.password" with "secret123"
    And click "button:@auth.signIn"
    Then page URL is "/reports"
    When fill "textbox:@reports.from" with "2026-09-19"
    And fill "textbox:@reports.to" with "2026-09-20"
    And click "button:@reports.apply"
    Then network request "GET /api/v1/dashboard/reports/daily" responded 200
    And list "testid:report-day" has 2 rows
    And region "testid:report-day-2026-09-19" contains:
      """json
      {
        "testid:day-date":      "2026-09-19",
        "testid:day-succeeded": "1",
        "testid:day-gross":     "$10.00"
      }
      """
    And region "testid:report-day-2026-09-20" contains:
      """json
      {
        "testid:day-succeeded":    "0",
        "testid:day-failed":       "0",
        "testid:day-gross":        "$0.00",
        "testid:day-refunded":     "$0.00",
        "testid:day-net":          "$0.00",
        "testid:day-success-rate": "—"
      }
      """
    And console has no errors

  Scenario: A rebuilt projection shows the browser the same report it showed before
    # The operator's command, run against a live stack: throw the report's
    # projection away and build it again from the audit table. The page is the
    # same page afterwards, which is what makes a projection safe to change.
    Given in PostgreSQL:
      """sql
      INSERT INTO payment_events (event_id, payment_id, seq, merchant_id, event_type, payload, occurred_at) VALUES
        ('0192e000-0000-7000-8000-000000000001', '0192e000-0000-7000-8000-0000000000a1', 1, 1, 'PaymentCreated',
         '{"amount": 1000, "currency": "USD", "card_brand": "visa", "card_last4": "4242", "reference": "e1"}', '2026-09-17T02:00:00Z'),
        ('0192e000-0000-7000-8000-000000000002', '0192e000-0000-7000-8000-0000000000a1', 2, 1, 'PaymentSucceeded',
         '{"amount": 1000, "currency": "USD", "card_brand": "visa", "reference": "e1", "provider_charge_id": "ch_e1", "source": "callback"}', '2026-09-17T02:00:01Z');
      """
    And background work has settled
    When visit "/login"
    And fill "textbox:@auth.email" with "owner@acme.test"
    And fill "textbox:@auth.password" with "secret123"
    And click "button:@auth.signIn"
    And fill "textbox:@reports.from" with "2026-09-17"
    And fill "textbox:@reports.to" with "2026-09-17"
    And click "button:@reports.apply"
    Then text of "testid:total-gross" is "$10.00"
    When projection "reports" is rebuilt from the event log
    And click "button:@reports.apply"
    Then network request "GET /api/v1/dashboard/reports/daily" responded 200
    And list "testid:report-day" has 1 row
    And text of "testid:total-gross" is "$10.00"
    And text of "testid:total-succeeded" is "1"
    And console has no errors

  Scenario: The report renders in Traditional Chinese with the merchant's currency
    Given locale is "zh_TW"
    And in PostgreSQL:
      """sql
      INSERT INTO payment_events (event_id, payment_id, seq, merchant_id, event_type, payload, occurred_at) VALUES
        ('0192d000-0000-7000-8000-000000000001', '0192d000-0000-7000-8000-0000000000a1', 1, 1, 'PaymentCreated',
         '{"amount": 1000, "currency": "USD", "card_brand": "visa", "card_last4": "4242", "reference": "d1"}', '2026-09-17T02:00:00Z'),
        ('0192d000-0000-7000-8000-000000000002', '0192d000-0000-7000-8000-0000000000a1', 2, 1, 'PaymentSucceeded',
         '{"amount": 1000, "currency": "USD", "card_brand": "visa", "reference": "d1", "provider_charge_id": "ch_d1", "source": "callback"}', '2026-09-17T02:00:01Z');
      """
    And background work has settled
    And in ClickHouse query returns 1 row:
      """sql
      SELECT 1 FROM report_events FINAL WHERE merchant_id = 1;
      """
    When visit "/login"
    And fill "textbox:@auth.email" with "owner@acme.test"
    And fill "textbox:@auth.password" with "secret123"
    And click "button:@auth.signIn"
    And fill "textbox:@reports.from" with "2026-09-17"
    And fill "textbox:@reports.to" with "2026-09-17"
    And click "button:@reports.apply"
    Then "heading:@reports.title" is visible
    And list "testid:report-day" has 1 row
    And text of "testid:total-gross" is "US$10.00"
    And console has no errors
