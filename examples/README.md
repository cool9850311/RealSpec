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

## Matrix

| Example | Backend | Frontend | Rendering | Infra | Domain |
|---------|---------|----------|-----------|-------|--------|
| [`minimart-go-nuxt`](./minimart-go-nuxt) | Go + Gin | Nuxt (SPA) | static | PostgreSQL | login / product list / points redemption |
| [`minimart-java-next`](./minimart-java-next) | Java 25 + Spring Boot 4.1 + Spring Security | Next.js 16 (App Router) | static export | PostgreSQL | same domain |

`minimart-java-next` breaks the rule below on purpose: it changes two axes at
once — backend language and frontend framework — because its owner asked for
exactly that pair. The cost is diagnosis. When it fails, the failure could be in
either layer, and the example alone cannot say which. Two things narrow it back
down. Its spec is not its own: it is a guarded copy of `minimart-go-nuxt`'s,
held identical by `scripts/check-spec-parity.mjs` — every feature file byte for
byte, the contract and the step registry except for descriptions and comments —
so a failure is never a difference in what is being asserted. And
`minimart-go-nuxt` is unchanged, so running the same scenarios there says
whether the spec or the new implementation is at fault.

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
