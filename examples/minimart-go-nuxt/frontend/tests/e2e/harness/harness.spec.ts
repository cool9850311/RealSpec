// The harness's self-test: PLAN.md §6.B's cases that cannot be a feature file.
//
// The positive cases ARE feature files — features/positive/, run as the
// `harness` project, because the natural way to assert that a step works is to
// use it. The cases below are the ones that cannot be written that way:
//
//   6, 7, 8, 9, 12, 14  a step that MUST FAIL. A feature that fails is a red
//                       suite, so these live in features/negative/, are excluded
//                       from a normal run, and are executed here as a child
//                       process whose failures are the assertion. Proving the
//                       failure is only half of it: an assertion whose message
//                       does not say what was wrong costs more than the bug it
//                       found, so each one's message is asserted too.
//   17                  a property of a parallel run as a whole.
//   18                  what a failed scenario leaves behind, and what it does
//                       not leave behind.
//
// These fixtures live under tests/e2e/harness and NOT under spec/bdd: they test
// this harness. Nothing about a deliberately broken assertion belongs in
// minimart's specification.

import fs from 'node:fs'
import path from 'node:path'
import { expect, test } from '@playwright/test'
import { parse as parseYaml } from 'yaml'
import { errorText, runChildSuite, testNamed, type ChildRun } from './child-run'
import { findRunResources, sweepRunResources } from '../docker'
import { ENV, EXAMPLE_DIR, E2E_DIR } from '../env'
import { readScenarioStacks } from '../stack'
import { LOCATOR_PATTERN } from '../locator'
import { watchBrowserErrors } from '../world'

/** The eleven scenarios of spec/bdd/e2e — minimart's own e2e suite. */
const SPEC_SCENARIO_COUNT = 11

/**
 * One negative fixture, and what its failure must say.
 *
 * The fragments are the load-bearing part of this file. Each one is a phrase a
 * reader would need in order to know what to fix: which key was missing, how
 * many rows there actually were, which elements an ambiguous locator matched.
 */
const NEGATIVE_CASES: Array<{
  planCase: number
  title: string
  mustSay: string[]
}> = [
  {
    planCase: 6,
    title: 'A key the region does not contain is reported as missing',
    mustSay: [
      'region "testid:product-row-1" contains:',
      'key "testid:product-discount"',
      'matched no element',
      'the key is missing from the region, not empty',
    ],
  },
  {
    planCase: 7,
    title: 'Two rows are claimed where the page renders three',
    mustSay: ['list "testid:product-row" has 3 rows, expected exactly 2'],
  },
  {
    planCase: 8,
    title: 'An element that renders the empty string is not non-null',
    mustSay: [
      'key "testid:product-name"',
      'is present but renders no text',
      '"<non-null>" requires text',
    ],
  },
  {
    planCase: 9,
    title: 'An ambiguous locator is never resolved to its first match',
    mustSay: [
      '"testid:product-row" matched 2 elements',
      'a singular step must not guess which one is meant',
      'Playwright strict mode',
      'data-testid="product-row-1"',
      'data-testid="product-row-2"',
    ],
  },
  {
    planCase: 12,
    title: 'A refused redemption leaves no order row',
    mustSay: ['expected exactly 1 row, got 0', 'SELECT 1 FROM orders WHERE product_id = 1;'],
  },
  {
    planCase: 14,
    title: 'A failed request is an error even when the page handled it',
    mustSay: [
      'console has no errors: the browser reported',
      'error(s) during this scenario',
      '/api/v1/me',
      '401',
    ],
  },
]

test.describe.configure({ mode: 'serial' })

test('§6.B cases 6-9, 12, 14: the negative fixtures fail, and each failure names its cause', async () => {
  const run = await runChildSuite('harness-negative', 'negative')
  try {
    expect(
      run.exitCode,
      `the harness-negative project must fail — every scenario in it is written to fail.\n${run.output}`,
    ).not.toBe(0)
    expect(run.tests, `no tests were reported.\n${run.output}`).toHaveLength(NEGATIVE_CASES.length)

    for (const negative of NEGATIVE_CASES) {
      const failed = testNamed(run, negative.title)
      expect(failed.status, `PLAN §6.B case ${negative.planCase} did not fail: ${negative.title}`).toBe(
        'failed',
      )
      const message = errorText(failed)
      for (const fragment of negative.mustSay) {
        expect(
          message,
          `PLAN §6.B case ${negative.planCase} failed, but its message does not say ` +
            `"${fragment}", which a reader would need in order to know what to fix.\n` +
            `Message was:\n${message}`,
        ).toContain(fragment)
      }
    }

    // ── §6.B case 18 ────────────────────────────────────────────────────────
    // Every one of those failures must have left the three artefacts behind,
    // and container logs are the one Playwright cannot produce by itself.
    for (const negative of NEGATIVE_CASES) {
      const failed = testNamed(run, negative.title)
      const names = failed.attachments.map((attachment) => attachment.name)
      expect(names, `case ${negative.planCase} left no trace`).toContain('trace')
      expect(names, `case ${negative.planCase} left no screenshot`).toContain('screenshot')
      for (const container of ['postgres', 'backend', 'caddy']) {
        expect(names, `case ${negative.planCase} left no ${container} log`).toContain(
          `container-${container}.log`,
        )
      }
      for (const attachment of failed.attachments) {
        if (!attachment.path) continue
        expect(fs.existsSync(attachment.path), `${attachment.name} was reported but is not on disk`).toBe(
          true,
        )
      }
    }

    // ── §6.B case 18, second half ───────────────────────────────────────────
    // A red scenario returns its containers exactly like a green one. The child
    // run's global teardown sweeps nothing (it adopted what it did not create),
    // so anything found here was left by a scenario, which is the point.
    await expectNothingLeftBehind(run)
  } finally {
    // Tidy up even when the assertions above failed, so one bad run does not
    // leave the machine dirty for the next one.
    await sweepRunResources(run.runId)
  }
})

test('§6.B case 17: four workers, a distinct port and database each, and nothing left behind', async () => {
  const run = await runChildSuite('spec', 'workers', ['--workers=4'])
  try {
    expect(run.exitCode, `the spec suite must pass on four workers.\n${run.output}`).toBe(0)
    expect(run.workers, 'the child run did not use four workers').toBe(4)
    expect(run.tests.filter((candidate) => candidate.status === 'passed')).toHaveLength(
      SPEC_SCENARIO_COUNT,
    )

    const stacks = readScenarioStacks(run.stateDir)
    expect(
      stacks,
      `every scenario records the address it was given; ${run.stateDir} has ${stacks.length} entries`,
    ).toHaveLength(SPEC_SCENARIO_COUNT)

    // The isolation claim. The port is what separates the scenarios: its own
    // port means its own proxy, and therefore its own backend and its own
    // database — and, because an origin includes the port, its own cookie jar.
    // The address is a literal, so no scenario depends on a name resolving.
    expect(
      stacks.map((stack) => stack.baseURL),
      'a scenario was addressed by something other than loopback and its own port',
    ).toEqual(stacks.map((stack) => `https://127.0.0.1:${stack.port}`))
    expect(unique(stacks.map((stack) => stack.port)), 'two scenarios shared a port').toHaveLength(
      SPEC_SCENARIO_COUNT,
    )

    // Scenarios do not get a network each: they share the run's one network,
    // created before the first test and removed after the last, so that no
    // bridge appears or disappears while a browser is running. Both halves are
    // pinned — the network is the shared one, the databases are one per
    // scenario, identified by container id because every scenario's PostgreSQL
    // answers to an alias this harness chose. Each of these eleven seeds a row
    // at primary key 1 and all eleven pass on four workers, which one database
    // between them could not do;
    // features/positive/database-isolation.feature says it as a scenario pair.
    expect(
      unique(stacks.map((stack) => stack.network)),
      'a scenario created a network of its own instead of joining the shared one',
    ).toEqual([process.env[ENV.sharedNetwork]])
    expect(unique(stacks.map((stack) => stack.database)), 'two scenarios shared a database').toHaveLength(
      SPEC_SCENARIO_COUNT,
    )

    // The @stack:auth scenarios really did get the production-shaped profile.
    expect(stacks.filter((stack) => stack.profile === 'auth').length).toBeGreaterThan(0)

    await expectNothingLeftBehind(run)
  } finally {
    await sweepRunResources(run.runId)
  }
})

test('console has no errors watches all three sources the registry names', async ({ page }) => {
  // The assertion claims "no error-level message and no uncaught exception …
  // and no request failed at the transport level". A collector that quietly
  // watched one of the three would look strict and be blind, so all three are
  // provoked here and all three must be recorded.
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
    // Port 9 (discard) with nothing listening: the request never connects, which
    // is a transport failure rather than an HTTP status.
    await fetch('http://127.0.0.1:9/never').catch(() => undefined)
  })

  await expect
    .poll(() => unique(errors.map((error) => error.source)).sort(), { timeout: 15_000 })
    .toEqual(['console', 'pageerror', 'requestfailed'])
})

test('§6.D: every e2e step in format.yml is implemented, and nothing else is', () => {
  // The registry is the grammar and the runner is its only implementation. A
  // pattern in one and not the other is a step that validates and does not run,
  // or runs and cannot be written — both of which are invisible until somebody
  // needs that step.
  const registry = parseYaml(
    fs.readFileSync(path.join(EXAMPLE_DIR, 'spec/bdd/format.yml'), 'utf8'),
  ) as { steps: Array<{ id: string; pattern: string }> }

  const sources = fs
    .readdirSync(path.join(E2E_DIR, 'steps'))
    .filter((name) => name.endsWith('.steps.ts'))
    .map((name) => fs.readFileSync(path.join(E2E_DIR, 'steps', name), 'utf8'))
    .join('\n')

  // API-only step ids. The three infrastructure steps are shared by both
  // surfaces and are implemented on both; the nine below are the HTTP client's
  // own and have no browser meaning.
  //
  // The two concurrency steps are in that list because a browser drives one
  // page. Four callers racing for the last unit is not something the e2e
  // surface can state, and a simulation of it would prove less than nothing.
  //
  // `save_response_cookie` is in that list for a stronger reason than
  // "unimplemented here". It is what an API scenario uses to carry a session,
  // and a browser scenario has no use for it at all: it signs in through the
  // form — `visit "/login"`, two `fill` steps and a `click`, all of them steps
  // this list does not exclude — after which the browser holds the cookie
  // itself, and script could not read the token out of it in any case because
  // it is HttpOnly. The asymmetry is the surfaces', not the registry's.
  const apiOnly = new Set([
    'http_request',
    'http_request_concurrent',
    'response_status',
    'response_set_status_count',
    'response_body_contains',
    'response_body_does_not_contain',
    'response_header_contains',
    'save_response_body_field',
    'save_response_cookie',
  ])

  const missing: string[] = []
  let implemented = 0
  for (const step of registry.steps) {
    if (apiOnly.has(step.id)) continue
    // A step definition writes its pattern one of exactly two ways: as a regex
    // literal, or — when it takes a locator — as a template literal with the
    // shared fragment interpolated. Both forms are derived from the registry's
    // pattern here, character for character, so this comparison is exact rather
    // than approximate: a pattern that drifted by one character is a miss.
    const found = [asRegexLiteral(step.pattern), asTemplateLiteral(step.pattern)].some((form) =>
      sources.includes(form),
    )
    if (found) implemented++
    else missing.push(`${step.id}: ${step.pattern}`)
  }
  expect(missing, 'e2e steps in format.yml with no implementation in tests/e2e/steps').toEqual([])

  // And the other direction: exactly as many registrations as registry entries.
  const registrations = sources.match(/^(Given|When|Then)\(/gm) ?? []
  expect(
    registrations.length,
    'tests/e2e/steps registers a different number of steps than format.yml declares for the e2e surface',
  ).toBe(implemented)
})

test('§6.D: every step in format.yml is used by at least one feature', () => {
  // The sibling test above proves the registry and the runner agree. This one
  // proves the registry and the FEATURES agree, which is the direction that
  // catches dead vocabulary.
  //
  // A registered step is an implemented step. An implementation no scenario ever
  // runs is unverified code wearing the badge of verified code, and it is
  // indistinguishable from the working kind until somebody needs it. This
  // project has already been bitten by exactly that: the `headers` key of
  // `http_request` was implemented, documented, and used by no feature; when a
  // scenario finally exercised it, it worked — but nobody had known that.
  //
  // Keeping an unused step "because the registry is what a new project copies"
  // does not pay for itself either: a copying project must write the
  // implementation for its own application regardless, so the entry saves only
  // the wording and costs the claim that everything here is proven.
  const registry = parseYaml(
    fs.readFileSync(path.join(EXAMPLE_DIR, 'spec/bdd/format.yml'), 'utf8'),
  ) as { steps: Array<{ id: string; pattern: string }> }

  const featureFiles: string[] = []
  const walk = (dir: string): void => {
    for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
      const full = path.join(dir, entry.name)
      if (entry.isDirectory()) walk(full)
      else if (entry.name.endsWith('.feature')) featureFiles.push(full)
    }
  }
  walk(path.join(EXAMPLE_DIR, 'spec/bdd'))
  walk(path.join(E2E_DIR, 'harness/features'))

  // The same tokenising rules realspec validate applies: keyword stripped,
  // Examples tables and comments skipped, Scenario Outline parameters
  // normalised before matching.
  const stepLine = /^(Given|When|Then|And|But)\s+(.+)$/
  const section = /^(Feature:|Background:|Scenario Outline:|Scenario:|Examples:)/
  const outlineParam = /<[a-zA-Z][a-zA-Z0-9]*>/g

  const compiled = registry.steps.map((step) => {
    let pattern = step.pattern.split(/\s+/).join(' ')
    if (!pattern.startsWith('^')) pattern = `^${pattern}`
    if (!pattern.endsWith('$')) pattern = `${pattern}$`
    return { id: step.id, re: new RegExp(pattern) }
  })

  const uses = new Map(compiled.map((step) => [step.id, 0]))
  for (const file of featureFiles) {
    let inExamples = false
    for (const raw of fs.readFileSync(file, 'utf8').split('\n')) {
      const line = raw.trim()
      if (line === '' || line.startsWith('#')) continue
      if (section.test(line)) {
        inExamples = line.startsWith('Examples:')
        continue
      }
      if (inExamples && line.startsWith('|')) continue
      const match = stepLine.exec(line)
      if (!match) continue
      const text = match[2]!.replace(outlineParam, 'OUTLINE_PARAM')
      const hit = compiled.find((step) => step.re.test(text))
      if (hit) uses.set(hit.id, uses.get(hit.id)! + 1)
    }
  }

  const unused = [...uses].filter(([, count]) => count === 0).map(([id]) => id)
  expect(
    unused,
    `format.yml registers these steps, and no feature in ${featureFiles.length} files uses them. ` +
      'Either write a scenario that exercises the step, or delete it from the registry and both harnesses.',
  ).toEqual([])
})

/** The pattern as a JavaScript regex literal: only `/` needs escaping. */
function asRegexLiteral(pattern: string): string {
  return `/${pattern.replace(/\//g, '\\/')}/`
}

/**
 * The pattern as a template literal with the locator fragment interpolated.
 *
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

/** Asserts the child run returned every container and every network it created. */
async function expectNothingLeftBehind(run: ChildRun): Promise<void> {
  const leftovers = await findRunResources(run.runId)
  expect(
    leftovers,
    `the ${run.project} run left Docker resources behind; every scenario must stop its stack in a finally`,
  ).toEqual({ containerIds: [], networkNames: [] })
}
