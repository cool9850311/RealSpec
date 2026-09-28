@e2e @auth
Feature: Dashboard sign-in through the browser

  The dashboard is for a merchant's staff: they sign in with email and
  password, and the session is an HttpOnly cookie the page never sees. The four
  lines that open the first scenario — visit, fill, fill, click — are how every
  e2e scenario that needs a session gets one, so the sign-in page is exercised
  by every scenario that depends on it.

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
      INSERT INTO dashboard_users (id, merchant_id, email, password_hash) VALUES
        (1, 1, 'owner@acme.test', '$2a$10$MwKxT13/lPxFMjvrkm0dL.QJuNll1zljDU.eOoRVvgrWF.kf/hyC2');
      """
    And locale is "en_US"

  @stack:auth
  Scenario: The owner signs in and lands on the reports page
    When visit "/login"
    And fill "textbox:@auth.email" with "owner@acme.test"
    And fill "textbox:@auth.password" with "secret123"
    And click "button:@auth.signIn"
    Then network request "POST /api/v1/dashboard/login" responded 200
    And page URL is "/reports"
    And "heading:@reports.title" is visible
    And text of "testid:merchant-name" is "Acme Coffee"
    And "testid:login-error" is hidden
    And console has no errors

  Scenario: A wrong password shows the error and stays on the sign-in page
    When visit "/login"
    And fill "textbox:@auth.email" with "owner@acme.test"
    And fill "textbox:@auth.password" with "wrongpass"
    And press "Enter"
    Then network request "POST /api/v1/dashboard/login" responded 401
    And page URL is "/login"
    And text of "testid:login-error" is "Incorrect email or password."
    And "heading:@reports.title" is hidden

  Scenario: The reports page sends an anonymous visitor to sign in
    When visit "/reports"
    Then page URL is "/login"
    And "textbox:@auth.email" is visible
    And "testid:report-day" is hidden
    And console has no errors

  @stack:auth
  Scenario: Signing out ends the session, and the reports page is closed again
    When visit "/login"
    And fill "textbox:@auth.email" with "owner@acme.test"
    And fill "textbox:@auth.password" with "secret123"
    And click "button:@auth.signIn"
    Then page URL is "/reports"
    When click "button:@nav.signOut"
    Then network request "POST /api/v1/dashboard/logout" responded 204
    And page URL is "/login"
    When visit "/reports"
    Then page URL is "/login"
    And console has no errors

  Scenario: The sign-in page renders in Traditional Chinese
    Given locale is "zh_TW"
    When visit "/login"
    Then "textbox:@auth.email" is visible
    And "button:@auth.signIn" is visible
    And console has no errors
