@e2e @shop
Feature: Buying something, and never once touching paygate

  This is the integration from the customer's side, and what it proves is an
  absence. Three origins appear in the journey:

    /shop, /shop/pay/…, /shop/result   the demo MERCHANT's pages
    /provider/ecpay/…                  the PROVIDER's cashier — where the card goes
    /provider/issuer/3ds/…             the ISSUING BANK's page

  **paygate is not one of them.** The merchant's server calls it once, server to
  server, to create the order and get a signed form; the merchant's own page
  renders that form; the browser submits it to the provider. No page of
  paygate's, no token in a URL, no endpoint a browser calls. That is what ECPay's
  WooCommerce plugin does — its `receipt_page()` renders the form and submits it —
  and it is the point of a platform sitting behind the merchant rather than in
  front of the customer.

  So the card goes to the provider and to nobody else, and the payment is settled
  by the provider's back channel rather than by the browser's return: the shop
  says "awaiting payment" when the customer gets back, and only then does the
  outcome arrive.

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
    And locale is "en_US"

  Scenario: A customer pays at the provider and the shop finds out from the callback
    When visit "/shop"
    And fill "textbox:@shop.amount" with "12.50"
    And click "button:@shop.checkout"
    # The merchant created the order at paygate, server to server, and is now on
    # its OWN payment page with the form paygate gave it.
    Then network request "POST /demo-merchant/api/orders" responded 201
    And "heading:@shop.payTitle" is visible
    And region "testid:shop-pay-summary" contains:
      """json
      {
        "testid:shop-pay-merchant": "Acme Coffee",
        "testid:shop-pay-amount":   "$12.50"
      }
      """
    And save text of "testid:shop-pay-order-no" as "merchantTradeNo"
    # Submitting it leaves the merchant's origin for the provider's. It is the
    # only hop paygate had anything to do with, and it did that by signing a form,
    # not by being called now.
    When click "button:@shop.pay"
    Then network request "POST /provider/ecpay/Cashier/AioCheckOut/V5" responded 200
    And "heading:Payment" is visible
    # The card, at the provider's origin, on the provider's page.
    When fill "textbox:Card number" with "4242424242424242"
    And fill "textbox:Expiry" with "12/30"
    And fill "textbox:CVC" with "123"
    And click "button:Pay"
    Then "heading:3-D Secure" is visible
    And text of "testid:issuer-amount" is "$12.50"
    When click "button:Authenticate"
    # Back at the SHOP, which reads its own record and has not been told anything
    # yet. Guessing here is how a shop ships for free.
    Then page URL is "/shop/result"
    And text of "testid:shop-result-status" is "Awaiting payment"
    # The back channel reaches paygate, paygate tells the merchant, and the page,
    # which polls its own server, catches up.
    And text of "testid:shop-result-status" is "Paid"
    And payment provider received 1 checkout request
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments
       WHERE merchant_trade_no = '{merchantTradeNo}'
         AND merchant_id = 1
         AND amount = 1250
         AND status = 'succeeded'
         AND card_last4 = '4242';
      """
    When background work has settled
    Then merchant received 1 notification at "/demo-merchant/api/notify"
    And console has no errors

  Scenario: The card reaches the provider and nothing else
    When visit "/shop"
    And fill "textbox:@shop.amount" with "20.00"
    And click "button:@shop.checkout"
    And click "button:@shop.pay"
    And fill "textbox:Card number" with "4242424242424242"
    And fill "textbox:Expiry" with "12/30"
    And fill "textbox:CVC" with "123"
    And click "button:Pay"
    And click "button:Authenticate"
    Then text of "testid:shop-result-status" is "Paid"
    # The two servers that must never have seen it, did not.
    And network request "POST /demo-merchant/api/orders" responded 201
    And in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM payments p WHERE p::text LIKE '%424242424242%';
      """
    And in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM payment_attempts a WHERE a::text LIKE '%424242424242%';
      """
    And in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM payment_events e WHERE e::text LIKE '%424242424242%';
      """
    And in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM notifications n WHERE n::text LIKE '%424242424242%';
      """
    # And there is nowhere it could have been put even by accident.
    And in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM information_schema.columns
       WHERE table_schema = 'public'
         AND (column_name LIKE '%card_number%' OR column_name LIKE '%cvc%');
      """
    And console has no errors

  Scenario: A customer who fails authentication can try again, at a new provider number
    When visit "/shop"
    And fill "textbox:@shop.amount" with "7.00"
    And click "button:@shop.checkout"
    And save text of "testid:shop-pay-order-no" as "merchantTradeNo"
    And click "button:@shop.pay"
    And fill "textbox:Card number" with "4242424242424242"
    And fill "textbox:Expiry" with "12/30"
    And fill "textbox:CVC" with "123"
    And click "button:Pay"
    Then "heading:3-D Secure" is visible
    When click "button:Fail authentication"
    Then page URL is "/shop/result"
    And text of "testid:shop-result-status" is "Payment failed"
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments WHERE merchant_trade_no = '{merchantTradeNo}' AND status = 'pending';
      """
    # Nothing final happened, so paygate has told the merchant nothing.
    When background work has settled
    Then merchant received 0 notifications at "/demo-merchant/api/notify"
    # The shop offers the same order again, and paygate signs a new form for it.
    When click "button:@shop.payAgain"
    Then "heading:@shop.payTitle" is visible
    When click "button:@shop.pay"
    And fill "textbox:Card number" with "4242424242424242"
    And fill "textbox:Expiry" with "12/30"
    And fill "textbox:CVC" with "123"
    And click "button:Pay"
    And click "button:Authenticate"
    Then text of "testid:shop-result-status" is "Paid"
    And payment provider received 2 checkout requests
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payment_attempts a
        JOIN payments p ON p.id = a.payment_id
       WHERE p.merchant_trade_no = '{merchantTradeNo}'
      HAVING count(*) = 2 AND count(DISTINCT a.provider_trade_no) = 2;
      """
    When background work has settled
    Then merchant received 1 notification at "/demo-merchant/api/notify"

  Scenario: A declined card is reported by the shop, and the order stays payable
    When visit "/shop"
    And fill "textbox:@shop.amount" with "9.00"
    And click "button:@shop.checkout"
    And click "button:@shop.pay"
    And fill "textbox:Card number" with "4000000000000002"
    And fill "textbox:Expiry" with "12/30"
    And fill "textbox:CVC" with "123"
    And click "button:Pay"
    And click "button:Authenticate"
    Then text of "testid:shop-result-status" is "Payment failed"
    And "button:@shop.payAgain" is visible
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments WHERE status = 'pending' HAVING count(*) = 1;
      """

  Scenario: A customer who abandons the provider is not told they paid
    When visit "/shop"
    And fill "textbox:@shop.amount" with "15.00"
    And click "button:@shop.checkout"
    And save text of "testid:shop-pay-order-no" as "merchantTradeNo"
    And click "button:@shop.pay"
    Then "heading:Payment" is visible
    # They change their mind at the provider and use its own way back.
    When click "button:Cancel"
    Then page URL is "/shop/result"
    And text of "testid:shop-result-status" is "Awaiting payment"
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments WHERE merchant_trade_no = '{merchantTradeNo}' AND status = 'pending';
      """
    And in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM notifications;
      """
    And console has no errors

  Scenario: Pressing Back at the provider is how a customer ends up able to pay twice
    # Not a hypothetical. The customer is at ECPay, changes their mind, and presses
    # the browser's own Back button — which lands on the merchant's payment page,
    # which renders a form on sight, which is a second order at the provider. The
    # rule that forbids reusing an order number makes this unavoidable; what the
    # system owes is that only one of the two can settle the order, and that the
    # other is caught (`duplicate_payment.feature`).
    When visit "/shop"
    And fill "textbox:@shop.amount" with "18.00"
    And click "button:@shop.checkout"
    And save text of "testid:shop-pay-order-no" as "merchantTradeNo"
    And click "button:@shop.pay"
    Then "heading:Payment" is visible
    When go back
    Then "heading:@shop.payTitle" is visible
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payment_attempts a
        JOIN payments p ON p.id = a.payment_id
       WHERE p.merchant_trade_no = '{merchantTradeNo}'
      HAVING count(*) = 2 AND count(DISTINCT a.provider_trade_no) = 2;
      """
    # Paying with the form they came back to settles the order once.
    When click "button:@shop.pay"
    And fill "textbox:Card number" with "4242424242424242"
    And fill "textbox:Expiry" with "12/30"
    And fill "textbox:CVC" with "123"
    And click "button:Pay"
    And click "button:Authenticate"
    Then text of "testid:shop-result-status" is "Paid"
    And payment provider received 3 checkout requests
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments
       WHERE merchant_trade_no = '{merchantTradeNo}' AND status = 'succeeded' AND amount = 1800;
      """
    # Two of the three arrivals were never paid, so there is nothing to give back.
    And in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM refunds;
      """
    And console has no errors

  Scenario: Rendering the payment page twice is two orders at the provider and one paid order
    # ECPay will not take the same order number twice, so the second render gets a
    # new one. The consequence — a customer who could pay twice — is handled
    # afterwards, not by refusing the button.
    When visit "/shop"
    And fill "textbox:@shop.amount" with "30.00"
    And click "button:@shop.checkout"
    And save text of "testid:shop-pay-order-no" as "merchantTradeNo"
    And click "button:@shop.pay"
    Then "heading:Payment" is visible
    When click "button:Cancel"
    And click "button:@shop.payAgain"
    And click "button:@shop.pay"
    Then "heading:Payment" is visible
    And payment provider received 2 checkout requests
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payment_attempts a
        JOIN payments p ON p.id = a.payment_id
       WHERE p.merchant_trade_no = '{merchantTradeNo}'
      HAVING count(*) = 2 AND count(DISTINCT a.provider_trade_no) = 2;
      """
    When fill "textbox:Card number" with "4242424242424242"
    And fill "textbox:Expiry" with "12/30"
    And fill "textbox:CVC" with "123"
    And click "button:Pay"
    And click "button:Authenticate"
    Then text of "testid:shop-result-status" is "Paid"
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM payments WHERE amount = 3000 AND status = 'succeeded' HAVING count(*) = 1;
      """
    # Only one of the two was ever paid, so there is nothing to give back.
    And in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM refunds;
      """
    And console has no errors

  Scenario: A paid order has no payment page any more
    When visit "/shop"
    And fill "textbox:@shop.amount" with "11.00"
    And click "button:@shop.checkout"
    And click "button:@shop.pay"
    And fill "textbox:Card number" with "4242424242424242"
    And fill "textbox:Expiry" with "12/30"
    And fill "textbox:CVC" with "123"
    And click "button:Pay"
    And click "button:Authenticate"
    Then text of "testid:shop-result-status" is "Paid"
    And "button:@shop.payAgain" is hidden
    And payment provider received 1 checkout request

  Scenario: The shop's pages render in Traditional Chinese
    Given locale is "zh_TW"
    When visit "/shop"
    And fill "textbox:@shop.amount" with "12.50"
    And click "button:@shop.checkout"
    Then "heading:@shop.payTitle" is visible
    And region "testid:shop-pay-summary" contains:
      """json
      {
        "testid:shop-pay-amount": "US$12.50"
      }
      """
    When click "button:@shop.pay"
    And fill "textbox:Card number" with "4242424242424242"
    And fill "textbox:Expiry" with "12/30"
    And fill "textbox:CVC" with "123"
    And click "button:Pay"
    And click "button:Authenticate"
    # Asserted through the catalogue rather than as a literal, which is what the
    # `@key` locator form is for: it proves the zh_TW catalogue was the one used
    # without pinning a translation into a test.
    Then "status:@shop.resultPaid" is visible
    And console has no errors
