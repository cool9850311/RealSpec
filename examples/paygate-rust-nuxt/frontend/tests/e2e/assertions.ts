// The assertion primitives the step definitions are built from. Mirrors
// examples/minimart-go-nuxt/frontend/tests/e2e/assertions.ts — format.yml's
// preamble says this half of the registry is minimart's, unchanged in meaning.
//
// Two rules run through all of it.
//
// WAITING IS A CONDITION, NEVER A DURATION. The registry has no `wait <n>s`
// step and this file has no sleep: every assertion retries its own predicate
// until it holds or the budget runs out.
//
// THE FAILURE MESSAGE IS PART OF THE ASSERTION. `untilPasses` re-throws the
// LAST error the predicate produced, unwrapped, rather than a generic
// "timed out waiting for predicate".

import type { Locator } from '@playwright/test'
import { describeMatches, resolveLocator, type ResolvedLocator } from './locator'
import { normaliseText, type World } from './world'

/** How long an assertion waits for the page to agree with it. */
export const ASSERTION_TIMEOUT_MS = 10_000

/** How long an interaction waits for its target to be actionable. */
export const ACTION_TIMEOUT_MS = 10_000

const POLL_INTERVAL_MS = 100

/**
 * How long a page must go without starting a request before it counts as
 * quiet — the same 500 ms Playwright's own `networkidle` uses.
 */
const NETWORK_QUIET_MS = 500

/** How long waitForNetworkQuiet waits before it gives up. */
const SETTLE_TIMEOUT_MS = 30_000

/** The marker asserting "this element exists here and renders something". */
export const NON_NULL = '<non-null>'

/**
 * Retries `check` until it stops throwing, then returns; on expiry re-throws
 * the last error exactly as the check produced it.
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
 * Strict: zero matches and two matches are both failures.
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
 * Only the keys listed are checked; a missing key is reported as missing,
 * never silently treated as empty.
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
 * Parses a `contains:` docstring into ordered key/value pairs. Flat, and
 * strings only: the keys are locator tokens and the values are the text those
 * elements must render.
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
 * Waits until the page has stopped making requests, and has STAYED quiet for
 * NETWORK_QUIET_MS — see minimart's assertions.ts for why Playwright's own
 * `networkidle` cannot answer this after a click.
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
 * Waits for whatever the last navigation or click produced to have finished
 * settling: the network quiet, and — only when the document is paygate's own
 * SPA — the app's hydration marker set
 * (`document.documentElement.dataset.appReady`, set by
 * frontend/plugins/i18n.ts once the locale is settled).
 *
 * `click "button:@shop.pay"` (shop.feature, reports.feature) and the card
 * form's own "Pay"/"Authenticate" buttons leave paygate's SPA entirely for a
 * REAL, server-rendered HTML document — the provider's cashier and the
 * issuing bank's 3-D Secure page (spec.md, "The payment provider mock";
 * backend/crates/provider-mock/src/html.rs's own header: "a real browser
 * walks your cashier and your issuer page, so those two must be real HTML").
 * Neither page ever runs paygate's own JavaScript, so
 * `dataset.appReady` never appears on either one — not because rendering is
 * slow, but because there is no app there to set it, and a `click` that
 * waited for it anyway would wait the full test timeout on every scenario
 * that walks a customer through a real payment.
 *
 * `<div id="__nuxt">` is the root element the paygate build's client-only SPA
 * mounts into on every one of its own pages (nuxt.config.ts: `ssr: false`) and
 * the one thing a provider-mock or demo-merchant page never has any reason to
 * reproduce, so its presence is what tells the two cases apart — a fact about
 * the document actually on screen, not a guess about which path is "still
 * paygate's".
 */
export async function waitForRender(world: World): Promise<void> {
  await waitForNetworkQuiet(world)
  const isPaygateApp = await world.page.evaluate(() => document.getElementById('__nuxt') !== null)
  if (!isPaygateApp) return
  await world.page.waitForFunction(() => document.documentElement.dataset.appReady === 'true')
}
