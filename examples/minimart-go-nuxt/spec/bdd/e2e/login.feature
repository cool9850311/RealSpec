@e2e @auth
Feature: Login through the browser

  The same Background as the API features: `Given in PostgreSQL:` is one step,
  shared verbatim by both surfaces, so what a browser scenario seeds is written
  the way an HTTP scenario seeds it.

  The four lines of the first scenario — visit, fill, fill, click — are also how
  every other e2e scenario that needs a session gets one. There is no step that
  signs a browser in, because those four already do it, and doing it this way
  means the login page is exercised by every scenario that depends on it rather
  than only by the one scenario that is about it.

  Background:
    Given run migration
    And in PostgreSQL:
      """sql
      INSERT INTO users (id, username, password_hash, role, points) VALUES
        (1, 'alice', '$2a$10$MwKxT13/lPxFMjvrkm0dL.QJuNll1zljDU.eOoRVvgrWF.kf/hyC2', 'guest', 100);
      """
    And locale is "en_US"

  @stack:auth
  Scenario: Guest logs in through the form and lands on the catalogue
    When visit "/login"
    And fill "textbox:@auth.username" with "alice"
    And fill "textbox:@auth.password" with "secret123"
    And click "button:@auth.signIn"
    Then network request "POST /api/v1/auth/login" responded 200
    And page URL is "/products"
    And text of "testid:points-balance" is "100"
    And "testid:login-error" is hidden
    And console has no errors

  Scenario: A wrong password shows the error and stays on the login page
    When visit "/login"
    And fill "textbox:@auth.username" with "alice"
    And fill "textbox:@auth.password" with "wrongpass"
    And press "Enter"
    Then network request "POST /api/v1/auth/login" responded 401
    And page URL is "/login"
    And text of "testid:login-error" is "Incorrect username or password."
    And "testid:points-balance" is hidden

  Scenario: A protected page sends an unauthenticated visitor to the login page
    When visit "/orders"
    Then page URL is "/login"
    And "textbox:@auth.username" is visible
    And "testid:order-row" is hidden
