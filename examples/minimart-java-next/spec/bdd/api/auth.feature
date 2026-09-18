@auth
Feature: Authentication

  A session is a JWT in the HttpOnly `token` cookie. Every request below that
  needs one writes it out — `"Cookie": "token={aliceToken}"` — and the token is
  named by a `save response cookie "token" as "..."` under a real
  `POST /api/v1/auth/login` the scenario writes out itself. There is no cookie
  jar: nothing carries identity from one request to the next except the variable
  the feature names, so who a request is made by is readable at the request.

  The scenarios below the fourth are the other half of authentication: what the
  service does with a credential it never issued. Each carries one of the
  pre-minted credentials of `spec/bdd/format.yml` — `{tokenExpired}`,
  `{tokenWrongKey}`, `{tokenUnknownUser}`, `{tokenClaimsAdmin}` — which are what
  login would have issued for the fixture guest with exactly one field changed.
  They are adversarial on purpose, and the adversary is the suite itself: it
  holds `JWT_SECRET`, because it is the suite that hands that value to the
  container, so it can sign a token the service will accept without the service
  carrying one line of code for the privilege. A backdoor token, a test-only
  endpoint or a debug flag would buy the same scenarios by putting the
  attacker's tools inside the thing being defended.

  What that costs is that the deviation is named here and defined there. The
  request below is the price's other side: a `GET` with a `Cookie` header, the
  same shape as every honest request in this file, which is exactly how an
  attacker's request arrives.

  A credential nobody issued needs no step at all: it is a literal, and the last
  scenario writes it as one beside a real token. The two requests differ in the
  `Cookie` header and in nothing else, which is what makes that scenario an
  assertion rather than a decoration — if the header were being dropped, both
  would answer the same way and the scenario would go red.

  Background:
    Given run migration
    And in PostgreSQL:
      """sql
      INSERT INTO users (id, username, password_hash, role, points) VALUES
        (1, 'alice', '$2a$10$MwKxT13/lPxFMjvrkm0dL.QJuNll1zljDU.eOoRVvgrWF.kf/hyC2', 'guest', 100),
        (2, 'root',  '$2a$10$MwKxT13/lPxFMjvrkm0dL.QJuNll1zljDU.eOoRVvgrWF.kf/hyC2', 'admin', 0);
      """

  Scenario: Correct credentials set the token cookie and leak nothing
    When POST /api/v1/auth/login:
      """json
      {
        "body": {
          "username": "alice",
          "password": "secret123"
        }
      }
      """
    Then response status is 200
    And response body contains:
      """json
      {
        "username": "alice",
        "role": "guest"
      }
      """
    And response header "Set-Cookie" contains "token="
    And response header "Set-Cookie" contains "HttpOnly"
    And response body does not contain "password_hash"
    And response body does not contain "token"

  Scenario Outline: Invalid credentials are rejected without disclosing which half was wrong
    When POST /api/v1/auth/login:
      """json
      {
        "body": {
          "username": "<username>",
          "password": "<password>"
        }
      }
      """
    Then response status is 401
    And response body contains:
      """json
      {
        "error": "INVALID_CREDENTIALS"
      }
      """

    Examples:
      | username   | password  |
      | alice      | wrongpass |
      | nosuchuser | secret123 |

  Scenario: An admin reads their own profile through the session cookie
    When POST /api/v1/auth/login:
      """json
      {
        "body": {
          "username": "root",
          "password": "secret123"
        }
      }
      """
    Then response status is 200
    And save response cookie "token" as "rootToken"
    When GET /api/v1/me:
      """json
      {
        "headers": {
          "Cookie": "token={rootToken}"
        },
        "body": {}
      }
      """
    Then response status is 200
    And response body contains:
      """json
      {
        "username": "root",
        "role": "admin",
        "points": 0
      }
      """

  Scenario: Without the cookie the profile endpoint is 401
    When GET /api/v1/me:
      """json
      {
        "body": {}
      }
      """
    Then response status is 401
    And response body contains:
      """json
      {
        "error": "UNAUTHENTICATED"
      }
      """

  Scenario: An expired token is refused even though it was signed correctly
    When GET /api/v1/me:
      """json
      {
        "headers": {
          "Cookie": "token={tokenExpired}"
        },
        "body": {}
      }
      """
    Then response status is 401
    And response body contains:
      """json
      {
        "error": "UNAUTHENTICATED"
      }
      """

  Scenario: A token signed with another key is refused, whatever it claims
    When GET /api/v1/me:
      """json
      {
        "headers": {
          "Cookie": "token={tokenWrongKey}"
        },
        "body": {}
      }
      """
    Then response status is 401
    And response body contains:
      """json
      {
        "error": "UNAUTHENTICATED"
      }
      """

  Scenario Outline: A token cookie that is not a signed token is refused
    When GET /api/v1/me:
      """json
      {
        "headers": {
          "Cookie": "token=<token>"
        },
        "body": {}
      }
      """
    Then response status is 401
    And response body contains:
      """json
      {
        "error": "UNAUTHENTICATED"
      }
      """

    Examples:
      | case      | token                                                            |
      | empty     |                                                                  |
      | garbage   | not-a-token                                                      |
      | truncated | eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxIiwicm9sZSI6ImFkbWluIn0 |

  Scenario: A token for a user who no longer exists is refused
    Given in PostgreSQL query returns 0 rows:
      """sql
      SELECT 1 FROM users WHERE id = 4242 OR username = 'ghost';
      """
    When GET /api/v1/me:
      """json
      {
        "headers": {
          "Cookie": "token={tokenUnknownUser}"
        },
        "body": {}
      }
      """
    Then response status is 401
    And response body contains:
      """json
      {
        "error": "UNAUTHENTICATED"
      }
      """

  Scenario: A forged admin claim is answered from the users row, not from the token
    Given in PostgreSQL query returns 1 row:
      """sql
      SELECT 1 FROM users WHERE id = 1 AND username = 'alice' AND role = 'guest';
      """
    When GET /api/v1/me:
      """json
      {
        "headers": {
          "Cookie": "token={tokenClaimsAdmin}"
        },
        "body": {}
      }
      """
    Then response status is 200
    And response body contains:
      """json
      {
        "username": "alice",
        "role": "guest"
      }
      """
    When GET /api/v1/admin/orders:
      """json
      {
        "headers": {
          "Cookie": "token={tokenClaimsAdmin}"
        },
        "body": {}
      }
      """
    Then response status is 403
    And response body contains:
      """json
      {
        "error": "FORBIDDEN"
      }
      """

  Scenario: A Cookie header written by hand is the credential the service reads
    When POST /api/v1/auth/login:
      """json
      {
        "body": {
          "username": "alice",
          "password": "secret123"
        }
      }
      """
    Then response status is 200
    And save response cookie "token" as "aliceToken"
    When GET /api/v1/me:
      """json
      {
        "headers": {
          "Cookie": "token=not.a.real.token"
        },
        "body": {}
      }
      """
    Then response status is 401
    And response body contains:
      """json
      {
        "error": "UNAUTHENTICATED"
      }
      """
    When GET /api/v1/me:
      """json
      {
        "headers": {
          "Cookie": "token={aliceToken}"
        },
        "body": {}
      }
      """
    Then response status is 200
    And response body contains:
      """json
      {
        "username": "alice",
        "role": "guest"
      }
      """
