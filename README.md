# RealSpec

A spec-driven development standard — and a far better one than any other SDD
repository you will find.

Better because it is enforced by a machine rather than asked for by a document.
The others hand a model some Markdown and hope, then hand it more next feature —
a spec, a plan and a task list all describing the same system slightly
differently, none of them ever wrong out loud, all of them rotting quietly.

Better to maintain, for the same reason. A stale spec fails the build instead of
aging in a folder, and one fact has exactly one home. Adding behaviour costs a
`.feature` file rather than another pile of prose; changing behaviour means
editing sentences and watching the suite go red until the code agrees. The spec
cannot drift away from the system, because the drift is the failing test.

The unit of truth is a `spec/` directory, the unit of enforcement is
`realspec validate`, and the unit of proof is a runnable example.

```
spec/
├── spec.md                  architecture and the decisions behind it
├── openapi/openapi.yaml     the API contract
└── bdd/
    ├── format.yml           the step registry — the only steps that exist
    ├── api/*.feature        Gherkin, driven programmatically — HTTP, gRPC, a queue
    └── e2e/*.feature        Gherkin, driven as a user — a browser, a CLI, a native app
```

The split is who drives the scenario, not the protocol or the tool. Swapping the
driver changes the bindings and not a single sentence.

## Why Gherkin

Not because a product manager will write the scenarios. They will not.

Because it is a grammar with nowhere to hide: a list of sentences, each with an
optional JSON or SQL block, and nothing else. No expressions, no conditionals,
no escape into code. A format that allows code allows anything — and anything is
what gets written when a suite is under pressure to go green.

Because a step is a line of text, it can be matched against a registry. Prose
cannot be checked; a spec written in a programming language cannot be
constrained, since every expression in it is already legal.

Because what is asserted lives in the feature and how it is performed lives in
shared code, which is what makes "do not edit the test" enforceable rather than
aspirational.

And because every language already has a mature step-binding framework for it.
The sentences are the specification and they travel; bindings are the only part
that was ever language-specific.

Its bad reputation comes from suites that use it as a wordy alias for function
calls — `Given I click the element with id "submit"`. The format does not
prevent that. The registry does.

## Why a registry

Here is how a refused request is usually tested.

```js
const res = await fetch('/api/v1/orders', { method: 'POST', body })
expect(res.status).toBe(422)
```

It passes when the server refuses the order. It also passes when the server
refuses the order *after* deducting the points, or after decrementing the stock,
or after writing a half-finished row. The one thing that matters about a refusal
— that nothing happened — is the one thing it never looks at.

Here is the same refusal as a scenario.

```gherkin
Scenario: Insufficient points is refused and writes nothing
  When POST /api/v1/orders:
    """json
    { "headers": { "Cookie": "token={bobToken}" }, "body": { "product_id": 1 } }
    """
  Then response status is 422
  And response body contains:
    """json
    { "error": "INSUFFICIENT_POINTS" }
    """
  And in PostgreSQL query returns 0 rows:
    """sql
    SELECT 1 FROM orders;
    """
  And in PostgreSQL query returns 1 row:
    """sql
    SELECT 1 FROM users WHERE username = 'bob' AND points = 10;
    """
```

`format.yml` registers every step that may appear, as a fully anchored regular
expression. A step that is not in it is a validation error, not a new step.

**Why the spec is precise.** A step is not test code written for this scenario.
It is a fixed operation, implemented once and reused everywhere — 27 steps carry
319 uses across these features, so `response status is` and
`in PostgreSQL query returns 0 rows:` already mean something settled long before
this feature was written. A scenario is assembled from words whose behaviour is
not up for negotiation, which is why it says exactly what it says.

**Why it does not overfit.** Overfitting happens when the test is edited to fit
the code — an assertion loosened, a selector widened, a case quietly dropped.
That is also how a model makes a red suite go green. Here there is nothing in a
scenario to edit: it is sentences from a closed vocabulary. To soften one
assertion you would have to change a step implementation that 30 other scenarios
share, which breaks them and shows up immediately in review.

So the expected shape of adding a feature is a new `.feature` file and **no
change to step code at all**. When a diff touches a step implementation, that is
the signal: either the vocabulary genuinely lacked a word, or something is being
made to pass.

## When to add a step

Almost never. The first question is never "what step do I need"; it is "what can
I already say".

**If existing steps compose into the same meaning, compose them.** A step that
logs a user in was deleted because `visit`, `fill` and `click` already say it,
and a step that filled a whole form was deleted because it was one `fill` per
field. Neither was a capability. Both were an abbreviation, and an abbreviation
in a shared vocabulary costs more than the lines it saves.

**Prefer generalising an existing step to adding a specific one.** A step that
logged in and named the token did two things: it made a request, which
`http_request` already does, and it read a cookie out of the response, which
nothing could do. Only the second half deserved a word. It became
`save response cookie "<name>" as "<var>"` — more general than what it replaced,
and the login went back to being an ordinary request.

**A new step must be a capability, not a convenience.** Concurrency earned two:
firing four requests at one instant is not four requests in a row, and no
sequence of existing steps expresses it. That is the bar. "It would be shorter"
is not.

**Every registered step must be used by a scenario.** A registered step is an
implemented step, so an unused one is code that has never run wearing the badge
of code that has. Three of those were deleted; a test now fails the build if
another appears.

## The CLI, and CI

```bash
npx realspec validate spec/bdd/**/*.feature
```

`format.yml` is found by walking up from each feature file, so that command
needs no configuration. Exit code is `0` when every file passes and `1` when
anything fails. The report is one block per file:

```
────────────────────────────────────────────────────────────────────
  FAIL  spec/bdd/e2e/redeem.feature  (1 violation(s))
────────────────────────────────────────────────────────────────────
  line   12  no matching step definition found in format.yml
           → click "tbody tr"
```

CI is that command and then the two suites, in this order, each stage gating the
next:

```
1 · validate      the specification is well-formed
2 · API scenarios godog (go-nuxt) · Cucumber-JVM via ./mvnw verify (java-next)
3 · e2e scenarios playwright-bdd
```

Each example has its own three stages. The pipeline runs on pull requests only,
and only for the examples whose files the pull request changed — or for all of
them, when it changes something they share: the validator, the root workspace,
the scripts, the pipeline itself. An untouched example is not rebuilt to prove
it still works.

Validation goes first because it is the only stage that costs nothing. A feature
naming a step that does not exist should never reach a container. Stages 2 and 3
are sequential rather than parallel: both start containers per scenario, and
competing for the runner turns a timing-sensitive suite into a flaky one.

Every command in CI is the command that runs locally. There is no CI-only
variant, no relaxed timeout, no retry. A suite that needs different settings to
pass on a runner is not passing. The runner is provisioned first — one line, for
a property of the machine rather than a setting of the suite — and the suite
refuses to start on a machine that lacks it, wherever it runs.

The validator is the only packaged artefact here, because it is the only part
with no coupling to a project: it reads `format.yml` and `.feature` files and
nothing else. It is also a behaviour-compatible port of the Python validator this
standard grew out of — byte-identical output over that project's whole corpus,
held there by a test that runs wherever the corpus is pointed at
(`REALSPEC_PARITY_CORPUS`). Step implementations know
which containers to start and which images to build; they are not a library, they
are an example you copy.

## Getting started

Copy a spec directory and edit it:

```bash
cp -r examples/minimart-go-nuxt/spec your-project/
```

Then read that example. It is a complete, working application — Go + Gin +
PostgreSQL behind a Nuxt static SPA — and its CI is three stages: validate, API
scenarios, e2e scenarios. [`minimart-java-next`](./examples/minimart-java-next)
is the same application again — the same spec, copied and held identical by a
check — built with Java + Spring Boot behind a Next.js static export, which is
what it looks like when only the implementation changes. See
[`examples/`](./examples) for the matrix of examples and which axis each one
varies.
