@harness @negative
Feature: A region assertion that names a key the region does not contain

  A harness fixture, not part of paygate's specification, and it MUST fail. Its
  message must say which key was missing and that it was missing rather than
  empty — a testid this report does not render, ever.

  The region is the report's totals row, and the docstring names one key that IS
  there beside one that never is. That pairing is the point: a step that failed on
  every key would satisfy a fixture naming only the absent one.

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
  Scenario: A key the region does not contain is reported as missing
    When visit "/login"
    And fill "textbox:@auth.email" with "owner@acme.test"
    And fill "textbox:@auth.password" with "secret123"
    And click "button:@auth.signIn"
    Then page URL is "/reports"
    And region "testid:report-totals" contains:
      """json
      {
        "testid:total-succeeded": "0",
        "testid:total-discount":  "0"
      }
      """
