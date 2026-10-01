// The harness's self-test: properties of this harness that cannot be a feature
// file. Mirrors examples/minimart-go-nuxt/frontend/tests/e2e/harness/harness.spec.ts,
// with one deliberate difference throughout, per `spec.md`, "Isolation": Ryuk
// stays enabled here (playwright.config.ts never sets
// TESTCONTAINERS_RYUK_DISABLED), so this suite never sweeps its own
// containers or networks. Every assertion below that minimart's version made
// by calling its own `sweepRunResources` first is made here by READING the
// Docker daemon — polling for a few seconds, because Ryuk's reaping happens
// asynchronously to the child process's exit, not synchronously with it.
//
// The positive cases live as feature files under
// tests/e2e/harness/features/positive/, because the natural way to assert that
// a step works is to use it. The cases below are the ones that cannot be
// written that way:
//
//   the negative fixtures MUST fail, and their failure messages must say why —
//   a feature that fails is a red suite, so those live in features/negative/,
//   excluded from a normal run, and are executed here as a child process whose
//   failures are the assertion;
//
//   a property of a parallel run as a whole (distinct ports and databases, one
//   shared network, Ryuk eventually reaping every scenario's containers);
//
//   registry parity (`spec.md`, "Layers": the "Registry parity" row) — the
//   one test in this file that needs no Docker at all, and must pass without
//   it.

import fs from 'node:fs'
import path from 'node:path'
import { expect, test } from '@playwright/test'
import { parse as parseYaml } from 'yaml'
import { errorText, runChildSuite, testNamed, type ChildRun } from './child-run'
import { findExistingNetworks, findRunContainers } from '../docker'
import { EXAMPLE_DIR, E2E_DIR } from '../env'
import { readScenarioStacks, runNetworkName } from '../stack'
import { LOCATOR_PATTERN } from '../locator'
import { watchBrowserErrors } from '../world'

/** The twenty scenarios of spec/bdd/e2e. */
const SPEC_SCENARIO_COUNT = 20

/**
 * One negative fixture, and what its failure must say. The fragments are the
 * load-bearing part of this file: each is a phrase a reader would need in
 * order to know what to fix.
 */
const NEGATIVE_CASES: Array<{ title: string; mustSay: string[] }> = [
  {
    title: 'A singular assertion is never resolved to the first of several rows',
    mustSay: [
      '"testid:report-day" matched 7 elements',
      'a singular step must not guess which one is meant',
      'Playwright strict mode',
    ],
  },
  {
    title: 'Two rows are claimed where the page renders seven',
    mustSay: ['list "testid:report-day" has 7 rows, expected exactly 2'],
  },
  {
    title: 'A key the region does not contain is reported as missing',
    mustSay: [
      'key "testid:total-discount"',
      'matched no element',
      'the key is missing from the region, not empty',
    ],
  },
]

test.describe.configure({ mode: 'serial' })

test('the negative fixtures fail, and each failure names its cause', async () => {
  const run = await runChildSuite('harness-negative', 'negative')
  expect(
    run.exitCode,
    `the harness-negative project must fail — every scenario in it is written to fail.\n${run.output}`,
  ).not.toBe(0)
  expect(run.tests, `no tests were reported.\n${run.output}`).toHaveLength(NEGATIVE_CASES.length)

  for (const negative of NEGATIVE_CASES) {
    const failed = testNamed(run, negative.title)
    expect(failed.status, `"${negative.title}" did not fail`).toBe('failed')
    const message = errorText(failed)
    for (const fragment of negative.mustSay) {
      expect(
        message,
        `"${negative.title}" failed, but its message does not say "${fragment}", which a reader ` +
          `would need in order to know what to fix.\nMessage was:\n${message}`,
      ).toContain(fragment)
    }
  }

  // Every one of those failures must have left the trace and the container
  // logs — the one artefact Playwright cannot produce by itself, and the one
  // that explains a failure a screenshot cannot.
  for (const negative of NEGATIVE_CASES) {
    const failed = testNamed(run, negative.title)
    const names = failed.attachments.map((attachment) => attachment.name)
    expect(names, `"${negative.title}" left no trace`).toContain('trace')
    for (const container of ['postgres', 'redis', 'kafka', 'clickhouse', 'provider-mock', 'demo-merchant', 'api', 'caddy']) {
      expect(names, `"${negative.title}" left no ${container} log`).toContain(`container-${container}.log`)
    }
    for (const attachment of failed.attachments) {
      if (!attachment.path) continue
      expect(fs.existsSync(attachment.path), `${attachment.name} was reported but is not on disk`).toBe(true)
    }
  }

  // A red scenario returns its containers exactly like a green one: Ryuk is
  // the only thing that reaps here, and it does so once the child process's
  // connection to it closes — which has already happened by the time
  // runChildSuite resolves, but reaping itself is asynchronous, so this polls
  // briefly rather than asserting the instant the process exits.
  await expectContainersEventuallyReaped(run)
})

test('a run gives every scenario its own port, database and network, and Ryuk eventually reaps all of it', async () => {
  const run = await runChildSuite('spec', 'workers', ['--workers=4'])
  expect(run.exitCode, `the spec suite must pass on four workers.\n${run.output}`).toBe(0)
  expect(run.workers, 'the child run did not use four workers').toBe(4)
  expect(run.tests.filter((candidate) => candidate.status === 'passed')).toHaveLength(SPEC_SCENARIO_COUNT)

  const stacks = readScenarioStacks(run.stateDir)
  expect(
    stacks,
    `every scenario records the address it was given; ${run.stateDir} has ${stacks.length} entries`,
  ).toHaveLength(SPEC_SCENARIO_COUNT)

  // The isolation claim. The port is what separates the scenarios at the
  // browser: its own port means its own Caddy, and therefore its own api and
  // its own database — and, because an origin includes the port, its own
  // cookie jar.
  expect(
    stacks.map((stack) => stack.baseURL),
    'a scenario was addressed by something other than loopback and its own port',
  ).toEqual(stacks.map((stack) => `https://127.0.0.1:${stack.port}`))
  expect(unique(stacks.map((stack) => stack.port)), 'two scenarios shared a port').toHaveLength(
    SPEC_SCENARIO_COUNT,
  )
  expect(unique(stacks.map((stack) => stack.database)), 'two scenarios shared a database').toHaveLength(
    SPEC_SCENARIO_COUNT,
  )

  // One Docker network for the whole run (stack.ts's header comment: N
  // scenarios exhausted Docker's address pool when each made its own).
  // Isolation rests on the containers above, not on this — every scenario
  // reports the exact same network.
  const networksUsed = unique(stacks.map((stack) => stack.network))
  expect(networksUsed, 'every scenario of a run should share the one network').toHaveLength(1)

  // This is a NESTED run: it has its own run id (child-run.ts), which picks
  // out its own `runStateDir()` — but the network is looked up by a path
  // outside any single run's state (env.ts, `networkStateFile`), so it must
  // have found and reused the parent's (this very test's) network rather than
  // asking `ensureRunNetwork` to create a second one.
  expect(
    networksUsed[0],
    "the nested run's network is not the parent's — ensureRunNetwork created a second one",
  ).toBe(runNetworkName())
  await expect(findExistingNetworks([networksUsed[0]])).resolves.toEqual([networksUsed[0]])

  // The @stack:auth scenarios really did get the production-shaped profile.
  expect(stacks.filter((stack) => stack.profile === 'auth').length).toBeGreaterThan(0)

  // The network itself is deliberately NOT asserted "eventually reaped" here:
  // it outlives this one nested run on purpose (see
  // `expectContainersEventuallyReaped`'s own comment) — this test's own
  // process is still using it.
  await expectContainersEventuallyReaped(run)
})

test('console has no errors watches all three sources the registry names', async ({ page }) => {
  // The assertion claims "no error-level message and no uncaught exception …
  // and no request failed at the transport level". A collector that quietly
  // watched one of the three would look strict and be blind, so all three are
  // provoked here and all three must be recorded. Needs no Docker.
  const errors = watchBrowserErrors(page)
  await page.setContent('<p>harness</p>')

  await page.evaluate(() => {
    console.error('a deliberate console error')
  })
  await page.evaluate(() => {
    setTimeout(() => {
      throw new Error('a deliberate uncaught exception')
    }, 0)
  })
  await page.evaluate(async () => {
    // Port 9 (discard) with nothing listening: a transport failure, not a status.
    await fetch('http://127.0.0.1:9/never').catch(() => undefined)
  })

  await expect
    .poll(() => unique(errors.map((error) => error.source)).sort(), { timeout: 15_000 })
    .toEqual(['console', 'pageerror', 'requestfailed'])
})

test('registry parity: every e2e/shared step in format.yml is implemented here, and nothing else is', () => {
  // Needs no Docker — `spec.md`, "Layers": the "Registry parity" row requires
  // "every step is implemented there, and nothing else is." `bddgen`'s
  // `missingSteps: 'fail-on-gen'` covers one direction (every step a feature
  // uses is implemented); this test covers the other: nothing extra is
  // implemented. That every registered step is used by some feature is
  // `realspec validate`'s to check, not this harness's.
  const registry = parseYaml(
    fs.readFileSync(path.join(EXAMPLE_DIR, 'spec/bdd/format.yml'), 'utf8'),
  ) as { steps: Array<{ id: string; pattern: string }> }

  const sources = fs
    .readdirSync(path.join(E2E_DIR, 'steps'))
    .filter((name) => name.endsWith('.steps.ts'))
    .map((name) => fs.readFileSync(path.join(E2E_DIR, 'steps', name), 'utf8'))
    .join('\n')

  // The sixteen steps whose `When` is an HTTP call: backend/apitest's own,
  // with no browser meaning (format.yml's own count: "Sixteen more are
  // API-only"). The two concurrency steps are here because a browser drives
  // one page; `payment_form_submitted`/`provider_card_entered` are the API
  // surface playing the customer, which on THIS surface is a real browser at
  // the provider's own pages instead; `provider_delivers_callbacks` is the
  // API surface's way of releasing a queued callback, where the e2e surface
  // has an actual 3-D Secure page to click through instead (spec/bdd/e2e's own
  // header comments say so). `save_response_cookie` has no use here for a
  // stronger reason than "unimplemented": a browser scenario carries a session
  // by signing in through the real form, after which the browser holds an
  // HttpOnly cookie no script could read out in any case.
  const apiOnly = new Set([
    'http_request',
    'requests_concurrent',
    'payment_form_submitted',
    'provider_card_entered',
    'response_status',
    'response_set_status_count',
    'response_body_contains',
    'response_body_does_not_contain',
    'response_header_contains',
    'save_response_body_field',
    'save_response_cookie',
    'provider_delivers_callbacks',
    'callbacks_acknowledged',
    'provider_next_query_answer',
    'reconciler_runs',
    'service_state',
  ])

  const missing: string[] = []
  let implemented = 0
  for (const step of registry.steps) {
    if (apiOnly.has(step.id)) continue
    // A step definition writes its pattern one of exactly two ways: as a regex
    // literal, or — when it takes a locator — as a template literal with the
    // shared fragment interpolated. Both forms are derived from the registry's
    // pattern here, character for character, so this comparison is exact.
    const found = [asRegexLiteral(step.pattern), asTemplateLiteral(step.pattern)].some((form) =>
      sources.includes(form),
    )
    if (found) implemented++
    else missing.push(`${step.id}: ${step.pattern}`)
  }
  expect(missing, 'e2e/shared steps in format.yml with no implementation in tests/e2e/steps').toEqual([])

  // And the other direction: exactly as many registrations as registry entries
  // this surface owns (8 shared + 15 e2e-only = 23).
  const registrations = sources.match(/^(Given|When|Then)\(/gm) ?? []
  expect(
    registrations.length,
    'tests/e2e/steps registers a different number of steps than format.yml declares for this surface',
  ).toBe(implemented)
})

/** The pattern as a JavaScript regex literal: only `/` needs escaping. */
function asRegexLiteral(pattern: string): string {
  return `/${pattern.replace(/\//g, '\\/')}/`
}

/**
 * The pattern as a template literal with the locator fragment interpolated.
 * The fragment is lifted out before the backslashes are doubled, because it
 * contains backslashes of its own and it is written in the source as an
 * interpolation rather than as characters.
 */
function asTemplateLiteral(pattern: string): string {
  const SENTINEL = '\u0000'
  const lifted = pattern.replace(LOCATOR_PATTERN, () => SENTINEL)
  const escaped = lifted.replace(/\\/g, '\\\\').replace(SENTINEL, '${LOCATOR_PATTERN}')
  return `\`${escaped}\``
}

function unique<T>(values: T[]): T[] {
  return [...new Set(values)]
}

/**
 * Polls the Docker daemon for a few seconds, asserting that every container
 * the child run created is gone.
 *
 * Containers only: every scenario stops its own explicitly in a `finally`
 * (fixtures.ts, stack.ts's `teardown`), so this mostly confirms that already
 * happened rather than racing Ryuk for it. The run's one Docker network
 * (stack.ts's header comment) is deliberately NOT checked here — it is not
 * this child run's own to reap. It is the whole session's, shared with
 * whatever nested or parent run is still using it (this very test, for one),
 * and Ryuk's job is to remove it once every last connection to it closes, not
 * once this one child process exits. See the "one shared network" assertion
 * below for what IS checked about it.
 */
async function expectContainersEventuallyReaped(run: ChildRun, timeoutMs = 20_000): Promise<void> {
  const deadline = Date.now() + timeoutMs
  let containers: string[] = []
  for (;;) {
    containers = await findRunContainers(run.runId)
    if (containers.length === 0) return
    if (Date.now() > deadline) {
      throw new Error(
        `Ryuk did not reap the ${run.project} run's containers within ${timeoutMs}ms: ` +
          `${containers.length} still present`,
      )
    }
    await new Promise((resolve) => setTimeout(resolve, 500))
  }
}
