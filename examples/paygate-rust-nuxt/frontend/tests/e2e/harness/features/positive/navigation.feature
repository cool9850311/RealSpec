@harness
Feature: visit arrives at the path it was given, and locale renders before it

  A harness fixture, not part of paygate's specification: the mechanics of
  `visit` and `locale is` in isolation, before either is combined with a real
  sign-in the way spec/bdd/e2e/*.feature always does.

  Background:
    Given run migration

  Scenario: visit arrives at the path it was given, and the page has hydrated
    Given locale is "en_US"
    When visit "/login"
    Then page URL is "/login"
    And "button:@auth.signIn" is visible
    And console has no errors

  Scenario: The sign-in page renders in Traditional Chinese before any navigation happened
    Given locale is "zh_TW"
    When visit "/login"
    Then "button:登入" is visible
    And console has no errors
