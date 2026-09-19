# minimart — System Design (Java + Next.js)

`minimart` is the reference application of the RealSpec standard: the smallest
domain that still exercises every part of it — an HTTP API with authentication,
a read-only list, and one write that must be atomic across three tables, plus a
browser front end described with the same step grammar as the API.

This example has the same domain, API contract, step registry and feature files
as `examples/minimart-go-nuxt`; only the implementation differs — a Java 25 +
Spring Boot 4.1 + Spring Security service, and a Next.js 16 static export. The
spec is a copy of go-nuxt's that differs only in prose naming the
implementation. See *Deliberate differences from minimart-go-nuxt* at the end
of this file.

This file carries **architecture and decisions**. A fact that is executable
lives where it executes and is pointed at from here, never copied: a copy has
two owners and one of them is always about to be wrong. The pointers are

| For | Read |
|-----|------|
| Schema | `backend/src/main/resources/db/migration/V1__init.sql` (Flyway; byte-identical SQL to go-nuxt's `000001_init.up.sql`) |
| API surface — endpoints, shapes, status codes, and why each is what it is | `spec/openapi/openapi.yaml` |
| Behaviour, on both surfaces | `spec/bdd/api/*.feature`, `spec/bdd/e2e/*.feature` |
| The step grammar those features are written in | `spec/bdd/format.yml` |
| Element names and i18n keys the browser features address | the features themselves, and `frontend/` |
| Configuration | `backend/src/main/java/minimart/infrastructure/config/AppConfig.java`, `local/docker-compose.yml` |

---

## System Overview

- Three capabilities, nothing more: **login**, **product list**, **points redemption**.
- A user holds a points balance. Products cost points. Redeeming a product
  deducts points, decrements stock and records an order — all three in one
  transaction, or none of them.
- `stock = -1` means unlimited; any other value is the exact number of units
  left. `-1` is never decremented.
- Authentication is a JWT (HS256, shared secret) in an HttpOnly `token` cookie.
  No refresh token, no CSRF token, no OAuth: authentication exists here to give
  the e2e suite a protected page to test, not to be a study of auth.
- Two roles, `guest` and `admin`. `admin` may list every user's orders
  (`GET /admin/orders`); a `guest` who asks is told `403 FORBIDDEN`. One
  endpoint is enough to make the role load-bearing, and one is the minimum: a
  claim that no endpoint reads is a claim nothing can be wrong about, and a role
  nothing enforces is decoration.
- The role is read from the `users` row of the token's subject on every request
  that needs it — never from the token's own `role` claim. The claim is a
  convenience for the front end; the database is the authority.
- The front end is a Next.js static export (App Router, served from `out/` by
  `serve`) with three pages (`/login`, `/products`, `/orders`) in two locales (`zh_TW`, `en_US`). It talks to the API at the
  relative path `/api`, so the bundle contains no hostname and one prebuilt
  frontend container can be shared by every e2e scenario.

---

## Flow Diagrams

### 1. Login

```mermaid
sequenceDiagram
    participant Browser
    participant API
    participant DB

    Browser->>API: POST /api/v1/auth/login {username, password}
    API->>DB: SELECT id, password_hash, role FROM users WHERE username = ?
    alt no row or bcrypt mismatch
        API-->>Browser: 401 {"error": "INVALID_CREDENTIALS"}
    else match
        API-->>Browser: 200 {username, role}<br/>Set-Cookie: token=<JWT>, HttpOnly, Path=/, SameSite=Lax
    end

    Browser->>API: GET /api/v1/me (cookie)
    API->>DB: SELECT username, role, points FROM users WHERE id = <sub>
    API-->>Browser: 200 {username, role, points}
```

### 2. Browse and redeem

```mermaid
sequenceDiagram
    participant Browser
    participant API
    participant DB

    Browser->>API: GET /api/v1/products      (no auth)
    API->>DB: SELECT id, name, cost_points, stock FROM products WHERE active ORDER BY id
    API-->>Browser: 200 {items, total}

    Browser->>API: POST /api/v1/orders {product_id}   (cookie)
    API->>DB: BEGIN
    API->>DB: SELECT points FROM users WHERE id = ? FOR UPDATE
    API->>DB: SELECT cost_points, stock FROM products WHERE id = ? AND active FOR UPDATE
    alt product missing or inactive
        API->>DB: ROLLBACK
        API-->>Browser: 404 {"error": "PRODUCT_NOT_FOUND"}
    else points < cost_points
        API->>DB: ROLLBACK
        API-->>Browser: 422 {"error": "INSUFFICIENT_POINTS"}
    else stock = 0
        API->>DB: ROLLBACK
        API-->>Browser: 422 {"error": "OUT_OF_STOCK"}
    else
        API->>DB: UPDATE users SET points = points - cost WHERE id = ?
        API->>DB: UPDATE products SET stock = stock - 1 WHERE id = ? AND stock > 0
        API->>DB: INSERT INTO orders (user_id, product_id, cost_points) RETURNING id
        API->>DB: COMMIT
        API-->>Browser: 201 {id, cost_points, balance_after}
    end

    Browser->>API: GET /api/v1/orders (cookie)
    API->>DB: SELECT o.id, p.name, o.cost_points, o.created_at ... WHERE o.user_id = <sub>
    API-->>Browser: 200 {items, total}
```

### 3. Authorisation

```mermaid
sequenceDiagram
    participant Client
    participant API
    participant DB

    Client->>API: GET /api/v1/admin/orders (cookie)
    alt no cookie, bad signature, or expired
        API-->>Client: 401 {"error": "UNAUTHENTICATED"}
    else the token verifies
        API->>DB: SELECT role FROM users WHERE id = <sub>
        alt no row
            API-->>Client: 401 {"error": "UNAUTHENTICATED"}
        else role <> 'admin'
            API-->>Client: 403 {"error": "FORBIDDEN"}
        else role = 'admin'
            API->>DB: SELECT o.id, u.username, p.name, o.cost_points, o.created_at ...
            API-->>Client: 200 {items, total}
        end
    end
```

Authentication first, then authorisation, and the second question is asked of
the database rather than of the token — see *Authentication*.

### 4. Stock semantics

```mermaid
flowchart LR
    A["stock = -1"] -->|redeem| A
    B["stock = n, n > 0"] -->|redeem| C["stock = n - 1"]
    C -->|"n - 1 = 0"| D["stock = 0"]
    D -->|redeem| E["422 OUT_OF_STOCK"]
```

Only `stock > 0` is decremented, which is what makes the `UPDATE` itself the
concurrency guard (see *Idempotency and Atomicity*).

---

## Database Schema

`backend/src/main/resources/db/migration/V1__init.sql` — three tables, two indexes, and the
reasons for what it deliberately does not constrain, written beside the columns.

---

## API Design

`spec/openapi/openapi.yaml` — every endpoint, every request and response shape,
every error code, and the rationale for each choice in its `description`.

---

## Authentication

- JWT, HS256, signed with `JWT_SECRET`; claims `sub`, `username`, `role`, `exp`
  (24h). Delivered as an HttpOnly `token` cookie.
- **The `role` claim is not an authorisation decision.** Every endpoint that
  requires a role re-reads `users.role` for the subject in the security filter
  chain (`RoleAuthorizationManager`), behind
  the signature check and before the handler. A token says who the caller is;
  what they may do is a property of the database at the moment of the request,
  and a JWT is not revocable — a user demoted an hour ago still holds a
  correctly signed token that says `admin`. The round trip is one indexed
  primary-key read on a request that is about to run a bigger query anyway.

  `spec/bdd/api/auth.feature` proves it adversarially rather than by assertion:
  the harness signs its own token — real secret, unexpired, `role: admin` — for
  a user whose row says `guest`, and the service answers `403`. It travels as
  `{tokenClaimsAdmin}`, one of four pre-minted credentials `spec/bdd/format.yml`
  defines, in an ordinary `GET` with an ordinary `Cookie` header. Nothing about
  the shape of the request announces that it is an attack.
- A second cookie, `session_hint=1`, rides along: same `Path`, `SameSite` and
  `Secure`, but **not** `HttpOnly`. The API never reads it.

  The token is HttpOnly, which is correct and which means script cannot see it.
  A single-page front end therefore cannot tell "no session" from "a session I
  have not fetched yet" except by calling `GET /me` and treating the `401` as
  the answer — and that probe is visible: the browser records every failed
  request in the console at error level. It would make a developer's console
  untrustworthy, and it would make `console has no errors` unimplementable
  without excusing a class of message that also carries real failures. So the
  flag is explicit. It carries no claim, grants no access, and a forged one buys
  a `401`. Every `401` expires it (`session_hint=; Max-Age=0`) and the front end
  deletes it too, so a stale hint costs one request, once. A `403` deliberately
  leaves it alone: that session is valid — it is the authority that is missing,
  not the credential — and logging in again was never going to grant it.
- **There is no logout endpoint**, and no logout in the UI. A session ends when
  its token expires or the browser closes; a front end that needed to sign out
  sooner would delete the hint, which is all script can reach, and the server
  keeps no session state to clear.
- How the credential reaches the service is one thing, stated once: the `token`
  cookie on the request. A browser sends it because the browser was given it; a
  Cucumber-JVM scenario writes `"headers": {"Cookie": "token={aliceToken}"}` into the
  request docstring, every request, in full. The API harness keeps no cookie
  jar, so nothing is authenticated ambiently and a request that names no
  credential is testing an anonymous caller on purpose.

---

## Idempotency and Atomicity

minimart has no retries, no webhooks and no workers, so it has exactly one
correctness-critical operation: redemption. The transaction lives in
`OrderRepository.redeem` (`backend/src/main/java/minimart/infrastructure/repository/OrderRepository.java`),
which carries the lock order and the reason for it.

Every protection below is executable. The concurrency ones are two
`Scenario Outline`s in `spec/bdd/api/orders.feature` that race four real callers
and run three times each — one run catches a missing lock about 499 times in
500, three make the gap vanish.

| Protection | Proved by, in `spec/bdd/api/orders.feature` |
|------------|--------------------------------------------|
| All three writes in one transaction; every error path rolls back, leaving points, stock and orders untouched | *Insufficient points is refused and writes nothing*; *A product at zero stock is refused even when the caller can afford it* — both assert `0 rows` in `orders` and the untouched balance in SQL |
| `SELECT … FOR UPDATE` on the product row serialises two claims on the last unit | *Four callers race for the last unit and exactly one wins* — exactly 1 × `201`, 3 × `422`, one order row, `stock = 0` |
| `SELECT … FOR UPDATE` on the wallet serialises two claims on one balance | *Four simultaneous redemptions on one wallet spend it exactly once* — unlimited stock, so the wallet lock is the only thing between one redemption and four |
| `WHERE stock > 0` on the stock `UPDATE` excludes the `-1` sentinel, and the affected-row count is checked against the decision | *Unlimited stock is charged for but never decremented* — the order is written, `stock` is still `-1` |
| Double-submit is **not** protected, deliberately: two clicks are two redemptions, which is correct for a shop | *Two redemptions sent one after the other are two orders, not one* — an exact row count, which is what would catch an accidental collapse into one |

---

## Directory Structure

`spec/` is the specification — this file, `openapi/`, `bdd/`.
`local/` is the compose stack and Caddy config that development and e2e share.

```
backend/                                  Maven project (mvnw), Dockerfile
  src/main/java/minimart/
    domain/                               ErrorCode, DomainException, User, Product, Order, Stock, Redemption
    application/dto/                      records, one per openapi schema
    application/usecase/                  AuthUsecase, ProductUsecase, OrderUsecase
    infrastructure/config/AppConfig.java  the only reader of the environment
    infrastructure/security/*             filter chain, cookie token resolver, JWT, session cookies, role check, CORS
    infrastructure/web/*                  controllers, ApiExceptionHandler, JsonBody
    infrastructure/repository/*           User / Product / Order repositories, SchemaMigrator (Flyway)
  src/main/resources/db/migration/V1__init.sql
  src/test/java/minimart/                 unit tests
  src/test/java/minimart/bdd/             Cucumber-JVM API runner (ScenarioContext, hooks, steps)
frontend/                                 Next.js App Router, static export
  app/                                    routes: /, /login, /products, /orders
  components/, lib/
  i18n/{en_US,zh_TW}.json
  tests/e2e/                              playwright-bdd suite (stack.ts, world.ts, locator.ts, steps/)
  out/                                    build output, served by `serve`
```

---

## Test Plan

### Layers

| Layer | Tool | Lives in | Asserts |
|-------|------|----------|---------|
| Unit | JUnit (`./mvnw test`, Surefire) | `backend/src/test/java/minimart/` | Pure logic: JWT encode/decode, bcrypt verification, stock arithmetic, role enforcement, request parsing, configuration |
| API (BDD) | Cucumber-JVM (`./mvnw verify`, Failsafe) | `spec/bdd/api/*.feature` | HTTP status, response body, and the resulting database state |
| E2E (BDD) | playwright-bdd | `spec/bdd/e2e/*.feature` | What the user sees, and the database rows their clicks produce |
| Grammar | `realspec validate` | CI stage 1 | Every step in every feature exists in `format.yml`, with a legal keyword and the right docstring type |

Two rules govern `format.yml`, and they cut in opposite directions.

**Every entry is used by at least one feature**, and the harness asserts it. A
registered step is an implemented step, so an unused one is an implementation
that has never executed carrying the same badge as the ones that have. This
project was bitten by exactly that shape once: the `headers` key of
`http_request` was implemented, documented, used by no feature, and nobody knew
whether it worked until a scenario finally exercised it. It did — that was luck,
not evidence.

**A step that can be assembled from existing steps is not a step.** An entry is
earned by expressing something the rest of the vocabulary cannot, never by
shortening something it can. There is no step that logs in, on either surface: a
browser session is `visit "/login"`, two `fill`s and a `click` — the real form —
and an API session is the real `POST /auth/login` plus
`save response cookie "token" as "…"`, which is the only part of that exchange
no other step could express.

What a real login cannot express is what the service does with a credential it
never issued. Four context variables are that half — `{tokenExpired}`,
`{tokenWrongKey}`, `{tokenUnknownUser}`, `{tokenClaimsAdmin}` — minted by the
harness at the start of every scenario with the `JWT_SECRET` it set on the
container itself, so the service carries no test-only code for them. A test-only
endpoint or a debug flag would put the attacker's tools inside the artefact
being defended, and the suite would be evidence about a build nobody deploys.
`spec/bdd/format.yml` defines the set, and it is closed. A credential nobody
issued at all needs no machinery: it is a literal,
`"headers": {"Cookie": "token=not-a-token"}`.

Every failure scenario of a *write* asserts the absence of side effects with
`in PostgreSQL query returns 0 rows:` — a `422` that still deducted points would
pass a status-only test. Refusals of *reads* have no side effect to look for, so
they assert instead that nothing leaked out beside the error code
(`response body does not contain "Sticker Pack"`).

Every seeded user uses the same password, `secret123`, whose bcrypt hash is
written out as a literal in the feature files rather than generated: an e2e
scenario types the password one `fill` below the hash its Background seeded, so
neither can be changed without the other.

### Isolation

Per PLAN §4: each scenario gets its own PostgreSQL + backend + Caddy, reached at
`https://127.0.0.1:<its own published port>`, plus its own browser context. The
port is what separates them, so nothing has to be resolved. One prebuilt frontend
container is shared, which is only safe because the bundle is static and
addresses the API relatively.

Scenarios do **not** get a network each. The run creates one Docker network
before the first test and removes it after the last, and every scenario's
containers join it under aliases carrying the run id and the scenario index.
Isolation therefore rests on separate containers rather than separate networks:
own database, own backend, own origin. What is given up is network-level
separation — any scenario's containers can address any other's by alias, and
nothing enforces that they do not.

That is a deliberate trade, and the reason is measured. Creating a bridge gives
it the subnet's IPv4 gateway address and removing it takes the address away;
Chromium, which on a Linux runner shares that network namespace, reads any
address appearing or disappearing as the network having changed and aborts every
HTTP/2 request in flight with `ERR_NETWORK_CHANGED`. A bridge per scenario means
that happening underneath whatever other scenarios are mid-navigation. The
matching hazard from container churn — the IPv6 link-local address a veth gets —
is suppressed instead, by a sysctl the CI workflow sets and `tests/e2e/preflight.ts`
refuses to run without.

Tags select the infrastructure profile:

| Tag | Profile |
|-----|---------|
| *(none)* | postgres + backend + caddy, with `COOKIE_SECURE=false` and a permissive `FRONTEND_ORIGIN` — a scenario that is not about authentication should not go red for a cookie-policy reason |
| `@stack:auth` | the same containers, with the backend configured the way production is: `COOKIE_SECURE=true` and `FRONTEND_ORIGIN` pinned to that scenario's own origin — scheme, address **and** the port Caddy was published on, which is the part that differs per scenario. The cookie then has to survive TLS termination at Caddy, an origin check a wildcard would have waved through, and the browser's own `Secure` / `SameSite` rules |

In the project this standard came from, `@stack:auth` adds a separate auth
service and its database. minimart has no separate auth service, so the tag
selects configuration rather than containers — the mechanism (tag → infrastructure profile) is the part being
proved portable, not the particular containers.

### CI

`.github/workflows/ci.yml` — triggered only by `pull_request`. All three stages
run when this example's own directory changed; a change to shared tooling runs
only the first. Three blocking stages in order: `realspec validate`, then
`./mvnw -B verify` (Spotless, `-Werror`, unit tests, then the Cucumber-JVM API
suite), then `bddgen && playwright test`.
Validation is first because it is the only stage that costs nothing. Neither
runner's concurrency is set there; both halve the machine's CPU count.

---

## Implementation Order

Dependencies run top to bottom; items inside a phase can be built in parallel.

| Phase | Depends on | Delivers |
|-------|-----------|----------|
| 1 — Infrastructure | — | Flyway migration, `AppConfig`, Spring Boot skeleton with `/api/v1`, the Spring Security filter chain and CORS, and the Cucumber-JVM harness (`backend/src/test/java/minimart/bdd/`: `ScenarioContext`, container lifecycle hooks, migration runner, SQL helpers, an HTTP client with no cookie jar) |
| 2 — Auth | 1 | bcrypt + JWT issue (`POST /auth/login`), the cookie bearer-token resolver and JWT decoder, the role authorization manager that re-reads `users.role`, `GET /me`. Unblocks every authenticated scenario, because the features authenticate by actually logging in |
| 3 — Catalogue | 1 (parallel with 2) | `GET /products` with the `active` filter |
| 4 — Redemption | 2 + 3 | The redemption transaction (`POST /orders`), `GET /orders`, `GET /admin/orders`. The only phase with a concurrency requirement: not done until the two-writers-one-unit case is covered |
| 5 — Front end | the *contract* of 2–4, not their code | The Next.js static export: three pages, `zh_TW` + `en_US`, and ARIA correctness — real roles and accessible names, `data-testid` only where no role and name identify the element. That is what makes the locator grammar sufficient |
| 6 — E2E harness + features | 4 + 5 | `stack.ts` (tag → profile, per-scenario host/port), `world.ts`, `locator.ts`, `steps/`, and `spec/bdd/e2e/*.feature` against the same `format.yml` as the API side |

### Dependency graph

```
Phase 1 (Infrastructure)
    ├── Phase 2 (Auth) ───────────┐
    └── Phase 3 (Catalogue) ──────┤
                                  └── Phase 4 (Redemption)
                                          └── Phase 6 (E2E harness + features)
                                                  ↑
Phase 5 (Frontend) ───────────────────────────────┘
```

Phase 5 depends only on the contract in this document and in `openapi.yaml`, so
it can start the moment those are frozen — which is the point of writing them
first.

---

## Environment Variables

`backend/src/main/java/minimart/infrastructure/config/AppConfig.java` is the
whole configuration surface of the service — every name, its default, and why
the migrations directory and the front end's API base are deliberately not among
them. `application.properties` carries no environment placeholders.
`local/docker-compose.yml` is the worked example of setting them.

The names are identical to minimart-go-nuxt:

| Variable | Default | Meaning |
|----------|---------|---------|
| `PORT` | `8080` | HTTP listen port |
| `DB_DSN` | *(required)* | PostgreSQL URL, `postgres://user:pass@host:port/db?sslmode=…` — the same form the Go service takes; `AppConfig` converts it to a JDBC URL, user and password |
| `JWT_SECRET` | *(required)* | HS256 key; at least 32 bytes, validated at boot |
| `SCHEMA_AUTO_MIGRATE` | `false` | Apply the Flyway migrations at startup |
| `FRONTEND_ORIGIN` | *(empty)* | CORS origin; empty sends no CORS headers, `*` mirrors the request origin |
| `COOKIE_SECURE` | `false` | Mark the session cookies `Secure` |

Booleans accept exactly what Go's `strconv.ParseBool` accepts; anything else
fails startup, naming the variable.

`JWT_SECRET` is the one the API suite exploits: each scenario's container is
given a key the harness chose, which is what lets the harness mint the four
pre-minted credentials the service accepts as well-formed. `{tokenWrongKey}` is
signed with a second key the harness keeps and hands to nothing. Neither key is
any deployment's — a scenario that passed only because it shared production's
secret would not be a test.

---

## Deliberate differences from minimart-go-nuxt

Nothing observable through the contract differs; the features are byte-identical
and pass unchanged. What differs is forced by the stack, and each is stated here
so it is not mistaken for drift.

- **`JWT_SECRET` must be at least 32 bytes**, and the service refuses to boot
  with a shorter one. Spring Security signs and verifies through Nimbus, whose
  HS256 implementation rejects keys shorter than the 256-bit hash output. Every
  secret this example sets — compose, the API harness, the e2e harness, and the
  second key behind `{tokenWrongKey}` — is 32 bytes or longer.
- **The API runner is Cucumber-JVM**, not godog: the JUnit Platform engine run
  by Failsafe during `./mvnw verify` (`./mvnw test` runs only unit tests).
  Scenarios run in parallel at `max(1, CPU/2)`, overridable with
  `CUCUMBER_CONCURRENCY`. Step patterns are copied verbatim from `format.yml`,
  and a parity test fails if they drift.
- **JWT clock skew is explicitly 0.** Spring Security's timestamp validator
  defaults to 60 seconds of leeway, which would accept `{tokenExpired}` — a
  token exactly 60 seconds past `exp`. Go's `jwt/v5` has no leeway by default;
  this service sets it to zero to match.
- **The front end is a Next.js static export** (`output: "export"`), not a Nuxt
  SPA. It is prerendered in `en_US`, hydrates, then switches to the locale from
  the `locale` cookie or the browser and marks `data-app-ready`. The bundle, like
  Nuxt's, uses the relative API base `/api` and carries no hostname, so one
  prebuilt frontend container still serves every e2e scenario.
