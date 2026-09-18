// The browser-side assertions.
//
// The registry's assertions are exact by construction: equality, not
// containment; a count, not a lower bound; a missing key reported as missing,
// not as empty. Each of those choices is a defence against one specific way a
// browser test passes without testing anything, and the negative fixtures in
// tests/e2e/harness exist to prove each defence still holds.

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
  // Polling rather than sampling once is what makes this usable directly after
  // a click that triggers a client-side route change.
  await untilPasses(async () => {
    const actual = sitePath(world.page.url())
    if (actual !== want) {
      // Exact, not prefix: "/products" does not match "/products?page=2".
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
    // hidden, on purpose: a feature should not have to know which way the app
    // chose to remove an element.
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
    // Equality, not containment. Containment is how a test ends up passing on a
    // table header.
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
    let observed = -1
    // The count is EXACT. There is no "at least" form: ">= 1" is the assertion
    // that let an empty table pass as a populated one, and it is not
    // expressible here. 0 is legitimate and asserts the list is empty.
    await untilPasses(async () => {
      observed = await resolved.locator.count()
      if (observed !== want) {
        throw new Error(
          `list ${resolved.description} has ${observed} rows, expected exactly ${want}`,
        )
      }
    })
  },
)

Then(
  new RegExp(`^row (\\d+) of "${LOCATOR_PATTERN}" contains:$`),
  async ({ world }, index: string, token: string, docstring: string) => {
    const step = `row ${index} of "${token}" contains:`
    const resolved = resolveLocator(world.page, token, world)
    const entries = parseFlatObject(docstring, world, step)
    const wanted = Number(index)
    // An off-by-one is reported as "there are only N rows", never as a missing
    // key inside a row that does not exist.
    await untilPasses(async () => {
      const rows = await resolved.locator.count()
      if (rows < wanted) {
        throw new Error(`${step}: ${resolved.description} matched ${rows} rows, so there is no row ${wanted}`)
      }
    })
    const row = resolved.locator.nth(wanted - 1)
    await assertRegionContains(row, `row ${wanted} of ${resolved.description}`, entries, world, step)
  },
)

Then(
  /^network request "(GET|POST|PUT|PATCH|DELETE) ([^"]+)" responded (\d{3})$/,
  async ({ world }, method: string, path: string, status: string) => {
    const step = `network request "${method} ${path}" responded ${status}`
    const want = Number(status)
    // The log runs from the first navigation onward, so a request made before
    // this step is found without waiting; one that has not happened yet is
    // waited for.
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
 * `console has no errors`.
 *
 * Taken literally, with no allow-list: every `error`-level console message,
 * every uncaught exception and every request that failed at the transport level
 * fails the scenario.
 *
 * Excusing a class of message by matching its text — "Failed to load resource",
 * say — is the obvious shortcut and it is the wrong one twice over: the wording
 * belongs to the browser and changes with it, and the same wording carries real
 * failures. The right fix for a page that provokes an expected error is for the
 * page to stop provoking it, which is why the API sets a readable session hint
 * beside the HttpOnly token (spec.md, "Authentication"): with it the SPA knows
 * whether asking `/me` can succeed, and an anonymous page load makes no failing
 * request at all.
 *
 * The wait for the network to go quiet is not a sleep: it is what guarantees
 * that a request which is going to fail has already failed, so that the
 * assertion describes the whole scenario rather than whatever happened to have
 * finished. It counts from here rather than asking Playwright's `networkidle`
 * lifecycle event, which on an already-loaded document answers yes while a
 * request is still in flight — see watchInflightRequests in world.ts.
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
      // Saving "" would make every later interpolation quietly wrong.
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
