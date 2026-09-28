// The browser-side assertions. Mirrors
// examples/minimart-go-nuxt/frontend/tests/e2e/steps/assertion.steps.ts.
//
// The registry's assertions are exact by construction: equality, not
// containment; a count, not a lower bound; a missing key reported as missing,
// not as empty.

import { expect } from '@playwright/test'
import { Then } from '../fixtures'
import {
  ASSERTION_TIMEOUT_MS,
  assertRegionContains,
  assertTextEquals,
  parseFlatObject,
  readSingleText,
  untilPasses,
  waitForNetworkQuiet,
} from '../assertions'
import { LOCATOR_PATTERN, resolveLocator } from '../locator'
import type { NetworkEntry } from '../world'

Then(/^page URL is "(\/[^"]*)"$/, async ({ world }, expected: string) => {
  const want = world.resolveVars(expected)
  await untilPasses(async () => {
    const actual = sitePath(world.page.url())
    if (actual !== want) {
      throw new Error(`page URL is "${actual}", expected "${want}"`)
    }
  })
})

Then(
  new RegExp(`^"${LOCATOR_PATTERN}" is (visible|hidden)$`),
  async ({ world }, token: string, state: string) => {
    const resolved = resolveLocator(world.page, token, world)
    const step = `"${token}" is ${state}`
    // Both "absent from the DOM" and "present but not rendered" count as
    // hidden, on purpose.
    if (state === 'visible') {
      await expect(resolved.locator, `${step}: ${resolved.description}`).toBeVisible({
        timeout: ASSERTION_TIMEOUT_MS,
      })
    } else {
      await expect(resolved.locator, `${step}: ${resolved.description}`).toBeHidden({
        timeout: ASSERTION_TIMEOUT_MS,
      })
    }
  },
)

Then(
  new RegExp(`^text of "${LOCATOR_PATTERN}" is "([^"]*)"$`),
  async ({ world }, token: string, expected: string) => {
    const resolved = resolveLocator(world.page, token, world)
    await assertTextEquals(resolved, world.resolveVars(expected), `text of "${token}" is "${expected}"`)
  },
)

Then(
  new RegExp(`^region "${LOCATOR_PATTERN}" contains:$`),
  async ({ world }, token: string, docstring: string) => {
    const step = `region "${token}" contains:`
    const resolved = resolveLocator(world.page, token, world)
    const entries = parseFlatObject(docstring, world, step)
    await untilPasses(async () => {
      const count = await resolved.locator.count()
      if (count !== 1) {
        throw new Error(`${step}: the region ${resolved.description} matched ${count} elements, expected 1`)
      }
    })
    await assertRegionContains(resolved.locator, resolved.description, entries, world, step)
  },
)

Then(
  new RegExp(`^list "${LOCATOR_PATTERN}" has (\\d+) rows?$`),
  async ({ world }, token: string, expected: string) => {
    const resolved = resolveLocator(world.page, token, world)
    const want = Number(expected)
    // The count is EXACT: 0 is a legitimate expectation and asserts the list
    // is empty (spec/bdd/e2e/reports.feature's "no activity" scenario).
    await untilPasses(async () => {
      const observed = await resolved.locator.count()
      if (observed !== want) {
        throw new Error(`list ${resolved.description} has ${observed} rows, expected exactly ${want}`)
      }
    })
  },
)

Then(
  /^network request "(GET|POST|PUT|PATCH|DELETE) ([^"]+)" responded (\d{3})$/,
  async ({ world }, method: string, path: string, status: string) => {
    const step = `network request "${method} ${path}" responded ${status}`
    const want = Number(status)
    // Matches the MOST RECENT request with that method and path — load-bearing
    // for shop.feature's "pay again" scenarios, which post to the same
    // provider path more than once in one scenario.
    await untilPasses(async () => {
      const log = world.networkLog()
      const matches = log.filter((entry) => entry.method === method && entry.path === path)
      if (matches.length === 0) {
        throw new Error(
          `${step}: no such request was recorded.\nRequests recorded so far:\n${renderLog(log)}`,
        )
      }
      const latest = matches[matches.length - 1]
      if (latest.status !== want) {
        throw new Error(
          `${step}: it responded ${latest.status}.\nEvery ${method} ${path} recorded: ` +
            matches.map((entry) => entry.status).join(', '),
        )
      }
    })
  },
)

/**
 * `console has no errors`. Taken literally, with no allow-list: every
 * `error`-level console message, every uncaught exception and every request
 * that failed at the transport level fails the scenario. See minimart's
 * assertion.steps.ts for why an allow-list is the wrong fix.
 */
Then(/^console has no errors$/, async ({ world }) => {
  await waitForNetworkQuiet(world)
  const errors = world.browserErrors()
  if (errors.length === 0) return
  const detail = errors
    .map((error, index) => `  [${index + 1}] (${error.source}) ${error.text}\n        at ${error.where}`)
    .join('\n')
  throw new Error(
    `console has no errors: the browser reported ${errors.length} error(s) during this scenario:\n${detail}`,
  )
})

Then(
  new RegExp(`^save text of "${LOCATOR_PATTERN}" as "([a-z][a-zA-Z0-9]+)"$`),
  async ({ world }, token: string, varName: string) => {
    const step = `save text of "${token}" as "${varName}"`
    const resolved = resolveLocator(world.page, token, world)
    let text = ''
    await untilPasses(async () => {
      text = await readSingleText(resolved, step)
      if (text === '') throw new Error(`${step}: ${resolved.description} renders no text`)
    })
    world.saveVar(varName, text)
  },
)

/** Path plus query, without the origin — what a feature file writes. */
function sitePath(href: string): string {
  const url = new URL(href)
  return `${url.pathname}${url.search}`
}

function renderLog(log: readonly NetworkEntry[]): string {
  if (log.length === 0) return '  (none)'
  return log.map((entry) => `  ${entry.method} ${entry.path} -> ${entry.status}`).join('\n')
}
