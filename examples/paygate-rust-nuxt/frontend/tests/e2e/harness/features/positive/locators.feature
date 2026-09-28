@harness
Feature: The literal-name locator form

  A harness fixture, not part of paygate's specification: spec/bdd/e2e/*.feature
  always addresses the sign-in form through the "<role>:@<i18n.key>" form
  (`textbox:@auth.email`), so the plain "<role>:<literal name>" form format.yml
  also admits is otherwise never exercised. This fixture is that exercise: the
  accessible name of each field is its own `<label>` text, read literally
  instead of through the catalogue.

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
  Scenario: A field is found by its own label text, and a button by its own text
    When visit "/login"
    And fill "textbox:Email" with "owner@acme.test"
    And fill "textbox:Password" with "secret123"
    And click "button:Sign in"
    Then page URL is "/reports"
    And console has no errors
