// The assertion primitives the step definitions are built from.
//
// Two rules run through all of it.
//
// WAITING IS A CONDITION, NEVER A DURATION. The registry has no `wait <n>s`
// step and this file has no sleep: every assertion retries its own predicate
// until it holds or the budget runs out. A fixed sleep is the largest single
// source of flake in browser tests and it is also a silent one — it passes on a
// fast machine and fails on a loaded CI runner, which reads as "flaky test"
// rather than "wrong test".
//
// THE FAILURE MESSAGE IS PART OF THE ASSERTION. `untilPasses` re-throws the LAST
// error the predicate produced, unwrapped, rather than a generic "timed out
// waiting for predicate". An assertion whose failure does not say what was
// wrong costs more than the bug it found, so tests/e2e/harness asserts on these
// messages as well as on the failures themselves.

import type { Locator } from '@playwright/test'
import { describeMatches, resolveLocator, type ResolvedLocator } from './locator'
import { normaliseText, type World } from './world'

/**
 * How long an assertion waits for the page to agree with it.
 *
 * Generous enough for a container-backed round trip on a loaded machine,
 * short enough that a genuinely failing assertion reports quickly.
 */
export const ASSERTION_TIMEOUT_MS = 10_000

/** How long an interaction waits for its target to be actionable. */
export const ACTION_TIMEOUT_MS = 10_000

const POLL_INTERVAL_MS = 100

/**
 * How long a page must go without starting a request before it counts as quiet.
 *
 * The same 500 ms Playwright's own `networkidle` uses, and for the same reason:
 * one request's response is routinely what starts the next (login answers, the
 * page then asks `/me`, then routes and asks for the catalogue), so "nothing in
 * flight right now" is not the same question as "nothing left to do". A window
 * wide enough to bridge the gap between two links of that chain answers the
 * second question; the budget below is what bounds it.
 */
const NETWORK_QUIET_MS = 500

/**
 * How long waitForNetworkQuiet waits. Playwright's default navigation
 * timeout, which is what the `waitForLoadState` call this replaces was bounded
 * by — a page that is still fetching is not a failing assertion, it is a slow
 * machine, and the scenario's own 120 s budget is the one that should end it.
 */
const SETTLE_TIMEOUT_MS = 30_000

/** The marker asserting "this element exists here and renders something". */
export const NON_NULL = '<non-null>'

/**
 * Retries `check` until it stops throwing, then returns; on expiry re-throws the
 * last error exactly as the check produced it.
 */
export async function untilPasses(check: () => Promise<void>, timeoutMs = ASSERTION_TIMEOUT_MS): Promise<void> {
  const deadline = Date.now() + timeoutMs
  for (;;) {
    try {
      await check()
      return
    } catch (error) {
      if (Date.now() >= deadline) throw error
      await new Promise((resolve) => setTimeout(resolve, POLL_INTERVAL_MS))
    }
  }
}

/**
 * The text of the one element a locator must match, whitespace-normalised.
 *
 * Strict: zero matches and two matches are both failures, and the second one
 * says which elements it found. This is the function that makes
 * `text of "testid:product-row" is "…"` impossible to satisfy by accident when
 * the page renders three rows.
 */
export async function readSingleText(resolved: ResolvedLocator, step: string): Promise<string> {
  const count = await resolved.locator.count()
  if (count === 0) {
    throw new Error(`${step}: ${resolved.description} matched no element`)
  }
  if (count > 1) {
    throw new Error(
      `${step}: ${resolved.description} matched ${count} elements, and a singular step must not ` +
        `guess which one is meant (Playwright strict mode).\n${await describeMatches(resolved.locator)}`,
    )
  }
  return normaliseText(await resolved.locator.innerText())
}

/** Waits until the one matching element's text equals `expected`. */
export async function assertTextEquals(
  resolved: ResolvedLocator,
  expected: string,
  step: string,
): Promise<void> {
  await untilPasses(async () => {
    const actual = await readSingleText(resolved, step)
    if (actual !== expected) {
      throw new Error(
        `${step}: text of ${resolved.description} is "${actual}", expected "${expected}" ` +
          '(both whitespace-normalised)',
      )
    }
  })
}

/**
 * The partial match behind `region … contains:` and `row N of … contains:`.
 *
 * The browser-side twin of `response body contains:`, with the same semantics:
 * only the keys listed are checked and anything else inside the region is
 * ignored, so adding a column does not break an assertion that never mentioned
 * it. What it will not do is treat a missing key as an empty one — the whole
 * point of the step is that a cell which is not there and a cell which is empty
 * are different failures.
 */
export async function assertRegionContains(
  region: Locator,
  regionDescription: string,
  entries: ReadonlyArray<readonly [string, string]>,
  world: World,
  step: string,
): Promise<void> {
  for (const [key, expected] of entries) {
    const inner = resolveLocator(region, key, world)
    const label = `${step} key ${inner.description} inside ${regionDescription}`
    await untilPasses(async () => {
      const count = await inner.locator.count()
      if (count === 0) {
        throw new Error(`${label} matched no element — the key is missing from the region, not empty`)
      }
      if (count > 1) {
        throw new Error(
          `${label} matched ${count} elements and is therefore ambiguous.\n${await describeMatches(inner.locator)}`,
        )
      }
      const actual = normaliseText(await inner.locator.innerText())
      if (expected === NON_NULL) {
        if (actual === '') {
          throw new Error(`${label} is present but renders no text, and "${NON_NULL}" requires text`)
        }
        return
      }
      if (actual !== expected) {
        throw new Error(`${label} renders "${actual}", expected "${expected}"`)
      }
    })
  }
}

/**
 * Parses a `contains:` docstring into ordered key/value pairs.
 *
 * Flat, and strings only: the keys are locator tokens and the values are the
 * text those elements must render. A nested object would mean the DOM had been
 * re-imagined as a tree of JSON, which is precisely the abstraction this
 * grammar refuses — the page is addressed by locator, once per value.
 */
export function parseFlatObject(docstring: string, world: World, step: string): Array<[string, string]> {
  const resolved = world.resolveVars(docstring)
  let parsed: unknown
  try {
    parsed = JSON.parse(resolved)
  } catch (error) {
    throw new Error(`${step}: docstring is not valid JSON: ${error instanceof Error ? error.message : String(error)}`)
  }
  if (typeof parsed !== 'object' || parsed === null || Array.isArray(parsed)) {
    throw new Error(`${step}: docstring must be a JSON object whose keys are locator tokens`)
  }
  return Object.entries(parsed).map(([key, value]) => {
    if (typeof value !== 'string') {
      throw new Error(
        `${step}: the value of "${key}" must be a string (the text the element renders); got ${JSON.stringify(value)}`,
      )
    }
    return [key, value]
  })
}

/**
 * Waits until the page has stopped making requests.
 *
 * Counted from the call, never read from a cached flag. `World` keeps the set of
 * requests the page has started and not yet settled (see watchInflightRequests
 * for why Playwright's own `networkidle` lifecycle event cannot answer this
 * after a click), and this waits for that set to be empty and to STAY empty for
 * NETWORK_QUIET_MS, so a chain of requests is followed to its end rather than
 * mistaken for finished in the gap between two of its links.
 *
 * It is a condition and not a duration: a page with nothing to do satisfies it
 * in one quiet window, and a page that keeps working is waited on until it
 * stops or the budget ends — at which point the failure says what was still in
 * flight instead of the assertion downstream failing for a reason of its own.
 */
export async function waitForNetworkQuiet(world: World): Promise<void> {
  const deadline = Date.now() + SETTLE_TIMEOUT_MS
  let quietSince: number | null = null
  for (;;) {
    if (world.inflightRequestCount() === 0) {
      quietSince ??= Date.now()
      if (Date.now() - quietSince >= NETWORK_QUIET_MS) return
    } else {
      quietSince = null
    }
    if (Date.now() >= deadline) {
      throw new Error(
        `the page was still making requests after ${SETTLE_TIMEOUT_MS} ms ` +
          `(${world.inflightRequestCount()} in flight); it never went quiet for ${NETWORK_QUIET_MS} ms`,
      )
    }
    await new Promise((resolve) => setTimeout(resolve, POLL_INTERVAL_MS))
  }
}

/**
 * Waits for the SPA to have finished rendering.
 *
 * Two conditions, because neither alone is enough: the network has to go quiet
 * (the pages fetch their rows after mount, so the markup on disk is only the
 * chrome), and the app has to have set its hydration marker
 * (`document.documentElement.dataset.appReady`, set by frontend/plugins/i18n.ts
 * once the locale is settled — a test that raced it would compare the screen
 * against the wrong catalogue).
 */
export async function waitForRender(world: World): Promise<void> {
  await waitForNetworkQuiet(world)
  await world.page.waitForFunction(() => document.documentElement.dataset.appReady === 'true')
}
