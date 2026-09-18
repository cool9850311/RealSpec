@harness
Feature: Navigation and the protected-page gate

  A harness fixture, not part of minimart's specification: PLAN.md §6.B cases
  1 and 13. It asserts that `visit` goes where it was told, that a signed-in
  guest reaches a protected page, and that an anonymous visitor does not.

  Background:
    Given run migration
    And in PostgreSQL:
      """sql
      INSERT INTO users (id, username, password_hash, role, points) VALUES
        (1, 'alice', '$2a$10$MwKxT13/lPxFMjvrkm0dL.QJuNll1zljDU.eOoRVvgrWF.kf/hyC2', 'guest', 100);
      """
    And locale is "en_US"

  Scenario: visit arrives at the path it was given
    When visit "/products"
    Then page URL is "/products"
    And "heading:@shop.title" is visible
    And console has no errors

  @stack:auth
  Scenario: A signed-in guest reaches the protected page
    When visit "/login"
    And fill "textbox:@auth.username" with "alice"
    And fill "textbox:@auth.password" with "secret123"
    And click "button:@auth.signIn"
    And visit "/orders"
    Then page URL is "/orders"
    And "heading:@orders.title" is visible
    And "testid:orders-empty" is visible
    And text of "testid:points-balance" is "100"
    And console has no errors

  Scenario: An anonymous visitor is sent to the login page
    When visit "/orders"
    Then page URL is "/login"
    And "textbox:@auth.username" is visible
    And "testid:points-balance" is hidden
    And console has no errors
