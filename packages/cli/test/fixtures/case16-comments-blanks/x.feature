# a leading comment
Feature: comments and blanks

  # a comment inside the feature

  Scenario: s

    Given run migration

    # Given this comment is not a step
    Then response status is 200

