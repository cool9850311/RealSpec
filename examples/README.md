# Examples

Each example is a complete, runnable application that follows the RealSpec
standard end to end. They exist to prove the standard is real: if a rule cannot
be followed by a working application, it is not a rule, it is a wish.

Every example, without exception, has:

- `spec/spec.md` — architecture and the decisions behind it
- `spec/openapi/openapi.yaml` — the API contract
- `spec/bdd/format.yml` — the step registry (verbs), covering both surfaces
- `spec/bdd/api/*.feature` — scenarios whose `When` is an HTTP call
- `spec/bdd/e2e/*.feature` — scenarios whose `When` is a real browser action
- per-scenario infrastructure isolation
- a CI pipeline of exactly three stages — validate → API tests → e2e tests —
  in `.github/workflows/`, where a runner actually reads it

What differs between examples is the implementation. The point of having more
than one is to keep proving that the standard is independent of the stack.

## Validating

Stage 1 of every example's pipeline is the validator, the `@realspec/cli`
package of this repository. Install and build it once, then point it at an
example's features; `format.yml` is found by walking up from each file. These
are the commands `.github/workflows/ci.yml` runs:

```bash
npm ci
npm run build -w @realspec/cli
node packages/cli/dist/cli.js validate \
  examples/minimart-go-nuxt/spec/bdd/api/*.feature \
  examples/minimart-go-nuxt/spec/bdd/e2e/*.feature
```

`minimart-java-next` takes the same two globs under its own directory.
`paygate-rust-nuxt` also validates its `spec.md` and OpenAPI documents, and names
the registry explicitly:

```bash
node packages/cli/dist/cli.js validate \
  --format examples/paygate-rust-nuxt/spec/bdd/format.yml \
  examples/paygate-rust-nuxt/spec/spec.md \
  examples/paygate-rust-nuxt/spec/openapi/*.yaml \
  examples/paygate-rust-nuxt/spec/bdd/api/*.feature \
  examples/paygate-rust-nuxt/spec/bdd/e2e/*.feature
```

The harness fixtures live outside `spec/`, so they name the registry explicitly
(the same for each example, with its own directory):

```bash
node packages/cli/dist/cli.js validate \
  --format examples/minimart-go-nuxt/spec/bdd/format.yml \
  examples/minimart-go-nuxt/frontend/tests/e2e/harness/features/*/*.feature
```

All three registries bind their HTTP steps to `spec/openapi`, so a feature that
names a method and path the contract does not define fails validation.

## Matrix

| Example | Backend | Frontend | Rendering | Infra | Domain |
|---------|---------|----------|-----------|-------|--------|
| [`minimart-go-nuxt`](./minimart-go-nuxt) | Go + Gin | Nuxt (SPA) | static | PostgreSQL | login / product list / points redemption |
| [`minimart-java-next`](./minimart-java-next) | Java 25 + Spring Boot 4.1 + Spring Security | Next.js 16 (App Router) | static export | PostgreSQL | same domain |
| [`paygate-rust-nuxt`](./paygate-rust-nuxt) | Rust + axum | Nuxt (SPA) | static | PostgreSQL · Redis · Kafka · ClickHouse · an external payment provider | card payments through ECPay/NewebPay, as a platform merchant |

`minimart-java-next` breaks the rule below on purpose: it changes two axes at
once — backend language and frontend framework — because its owner asked for
exactly that pair. The cost is diagnosis. When it fails, the failure could be in
either layer, and the example alone cannot say which. Two things narrow it back
down. Its spec is not its own: it is a copy of `minimart-go-nuxt`'s — every
feature file byte for byte, the contract and the step registry except for
descriptions and comments — so a failure is not a difference in what is being
asserted. And `minimart-go-nuxt` is unchanged, so running the same scenarios
there says whether the spec or the new implementation is at fault.

`paygate-rust-nuxt` is the example that is not CRUD, and it changes three axes
rather than one: backend language, infrastructure and domain. It holds the
fourth still on purpose — the front end is Nuxt, with minimart's locator grammar
and minimart's e2e harness, unchanged in meaning — so when it breaks, the
browser layer is the one place the failure cannot have come from.

The three it does change are the three the standard had never been asked about
together. A payment service is a protocol implementation rather than a set of
tables: the same ECPay-shaped contract appears twice and paygate is on the
opposite side of it each time, signing what it hands a provider and verifying
what comes back, answering `1|OK` to its providers and demanding `1|OK` from its
merchants. Its infrastructure adds a cache whose loss must be survivable, a
queue, an OLAP store that is deliberately only eventually consistent, and a real
external counterparty — a provider mock that verifies paygate's signatures and
refuses what does not check out, which is how a scenario asserts "signed
correctly" without a digest ever appearing in a spec file. And its scenarios are
races: six servers asking for one order at one instant, four copies of one
callback, a customer who pays twice because the browser's Back button is not a
convenience in payments, refunds that must never exceed the charge.

What that stresses in the standard is whether a step vocabulary written for a
shop can say those things. Two steps and one variant were added to the registry
to find out, and the reasoning for each is in its own entry in
[`format.yml`](./paygate-rust-nuxt/spec/bdd/format.yml).

## Naming

`<domain>-<backend>-<frontend>`.

## Adding an example

An example earns its place by changing exactly one axis, so that when it breaks
you know which axis broke it:

| Axis | Candidates | What it stresses in the standard |
|------|-----------|----------------------------------|
| Backend language | TypeScript, Python | The shape of the API step runner; whether `format.yml` is genuinely language-neutral |
| Frontend framework | React SPA, SvelteKit | Portability of the locator grammar and of `tests/e2e/` |
| **Rendering mode** | **SSR (non-static)** | **Breaks the shared-frontend-container premise.** A server-rendered app has a runtime server, so either each scenario needs its own frontend container or the server must forward the incoming Host. This is the one assumption in the isolation design that is tied to the frontend being a static bundle. |
| Infrastructure | MySQL, no cache, no object storage | Whether `in PostgreSQL:` and friends are named and factored so they can be swapped |
| Domain | Scheduling, review workflows | Whether `region … contains:` and `list … has N rows` are expressive enough |
