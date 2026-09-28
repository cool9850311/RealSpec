@harness
Feature: Two scenarios own the same primary key

  A harness fixture, not part of paygate's specification.

  Both scenarios below seed `merchants.id = 1` with a DIFFERENT name, and each
  asserts that the dashboard shows its own. They can only both pass if each
  scenario has its own PostgreSQL: a shared database would make the second
  INSERT a duplicate-key error, and a shared database truncated between
  scenarios would make the two race and pass or fail by scheduling. Unlike
  examples/minimart-go-nuxt/frontend, there is also no run-wide network here to
  prove separate from — every scenario's stack.ts creates its own network, so
  this pair is what still proves the DATABASES are separate.

  Background:
    Given run migration
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
  Scenario: The first owner of merchant id 1
    Given in PostgreSQL:
      """sql
      INSERT INTO merchants (id, name, currency, timezone, provider_code, provider_merchant_id, rate_limit_per_minute, hash_key, hash_iv) VALUES
        (1, 'Acme Coffee', 'USD', 'Asia/Taipei', 'ecpay', '2000132', 600, 'acmehashkey0123456789abcdef01234', 'acmehashiv012345');
      """
    When visit "/login"
    And fill "textbox:@auth.email" with "owner@acme.test"
    And fill "textbox:@auth.password" with "secret123"
    And click "button:@auth.signIn"
    Then text of "testid:merchant-name" is "Acme Coffee"
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM merchants WHERE id = 1 AND name = 'Acme Coffee';
      """
    And in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM merchants WHERE name = 'Bagel Bros';
      """
    And console has no errors

  @stack:auth
  Scenario: The second owner of merchant id 1
    Given in PostgreSQL:
      """sql
      INSERT INTO merchants (id, name, currency, timezone, provider_code, provider_merchant_id, rate_limit_per_minute, hash_key, hash_iv) VALUES
        (1, 'Bagel Bros', 'USD', 'America/New_York', 'ecpay', '2000199', 600, 'bagelhashkey0123456789abcdef0123', 'bagelhashiv012345');
      """
    When visit "/login"
    And fill "textbox:@auth.email" with "owner@acme.test"
    And fill "textbox:@auth.password" with "secret123"
    And click "button:@auth.signIn"
    Then text of "testid:merchant-name" is "Bagel Bros"
    And in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM merchants WHERE id = 1 AND name = 'Bagel Bros';
      """
    And in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM merchants WHERE name = 'Acme Coffee';
      """
    And console has no errors
