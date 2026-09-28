@harness @negative
Feature: A row count that does not match the page

  A harness fixture, not part of paygate's specification, and it MUST fail: the
  report's default window is seven days (spec/bdd/e2e/reports.feature), and the
  count here claims two. The count is exact and there is no "at least" form, so
  this MUST fail.

  Background:
    Given run migration
    And in PostgreSQL:
      """sql
      INSERT INTO merchants (id, name, currency, timezone, provider_code, provider_merchant_id, rate_limit_per_minute, hash_key, hash_iv) VALUES
        (1, 'Acme Coffee', 'USD', 'Asia/Taipei', 'ecpay', '2000132', 600, 'acmehashkey0123456789abcdef01234', 'acmehashiv012345');
      """
    And in PostgreSQL:
      """sql
      INSERT INTO providers (code, platform_id, hash_key, hash_iv, cashier_url, query_url, refund_url) VALUES
        ('ecpay', '3002607', 'pwFHCqoQZGmho4w6', 'EkRm7iFT261dpevs', '/provider/ecpay/Cashier/AioCheckOut/V5', '/provider/ecpay/Cashier/QueryTradeInfo/V5', '/provider/ecpay/CreditDetail/DoAction');
      """
    And in PostgreSQL:
      """sql
      INSERT INTO dashboard_users (id, merchant_id, email, password_hash) VALUES
        (1, 1, 'owner@acme.test', '$2a$10$MwKxT13/lPxFMjvrkm0dL.QJuNll1zljDU.eOoRVvgrWF.kf/hyC2');
      """
    And locale is "en_US"

  @stack:auth
  Scenario: Two rows are claimed where the page renders seven
    When visit "/login"
    And fill "textbox:@auth.email" with "owner@acme.test"
    And fill "textbox:@auth.password" with "secret123"
    And click "button:@auth.signIn"
    Then page URL is "/reports"
    And list "testid:report-day" has 2 rows
