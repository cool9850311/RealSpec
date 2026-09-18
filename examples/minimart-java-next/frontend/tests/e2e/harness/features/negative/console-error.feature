@harness @negative @stack:auth
Feature: console has no errors against a page that really did fail

  PLAN.md §6.B case 14.

  The session is genuine: alice logs in and the browser holds both cookies. The
  row behind it is then deleted, so the session hint still says "ask" while the
  token's subject no longer exists. The catalogue therefore calls GET /api/v1/me
  and is refused with 401 — a real failure, not a probe: the page had every
  reason to believe it had a session.

  The browser records that refused request in its console at error level, and
  "console has no errors" is taken literally, so this MUST fail. No allow-list,
  no text matching: the step's answer to an expected error is that the page must
  stop producing it, which is why the hint cookie exists and why the 401 that
  exposes a stale one also expires it.

  Background:
    Given run migration
    And in PostgreSQL:
      """sql
      INSERT INTO users (id, username, password_hash, role, points) VALUES
        (1, 'alice', '$2a$10$MwKxT13/lPxFMjvrkm0dL.QJuNll1zljDU.eOoRVvgrWF.kf/hyC2', 'guest', 100);
      INSERT INTO products (id, name, cost_points, stock, active) VALUES
        (1, 'Sticker Pack', 50, 3, TRUE);
      """
    And locale is "en_US"

  Scenario: A failed request is an error even when the page handled it
    When visit "/login"
    And fill "textbox:@auth.username" with "alice"
    And fill "textbox:@auth.password" with "secret123"
    And click "button:@auth.signIn"
    And in PostgreSQL:
      """sql
      DELETE FROM users WHERE id = 1;
      """
    When visit "/products"
    Then network request "GET /api/v1/me" responded 401
    And list "testid:product-row" has 1 row
    And console has no errors
