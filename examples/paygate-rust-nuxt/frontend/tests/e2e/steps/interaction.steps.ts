// Navigation and interaction: everything a scenario's `When` can do to the
// page. Mirrors examples/minimart-go-nuxt/frontend/tests/e2e/steps/interaction.steps.ts.
//
// Every one of these is strict. Playwright's own actions refuse to act when a
// locator matches more than one element, and the failures below only add what
// Playwright cannot know: which token the feature wrote, and — for an `@key`
// token — which string that key resolved to in the active locale.

import { When } from '../fixtures'
import { ACTION_TIMEOUT_MS, waitForNetworkQuiet, waitForRender } from '../assertions'
import { LOCATOR_PATTERN, resolveLocator } from '../locator'

When(/^visit "(\/[^"]*)"$/, async ({ world }, target: string) => {
  const response = await world.page.goto(target)
  if (!response) {
    throw new Error(`visit "${target}": the browser performed no navigation`)
  }
  if (!response.ok()) {
    throw new Error(`visit "${target}": the document responded ${response.status()}`)
  }
  world.markNavigated()
  // Deliberately does NOT assert the resulting URL: a page that sends an
  // unauthenticated visitor to /login is a legitimate outcome, and the
  // feature states it with `page URL is`.
  await waitForRender(world)
})

When(/^go back$/, async ({ world }) => {
  // The page being left is allowed to finish first: leaving while a prefetch
  // is in flight cancels it, and a cancelled request is one more thing the
  // next assertion would have to reason about. It also matters more here than
  // it did for minimart: `shop.feature`'s "pressing Back at the provider" is
  // this suite's whole reason to have this step.
  await waitForNetworkQuiet(world)
  const entries = await world.page.evaluate(() => window.history.length)
  if (entries <= 1) {
    throw new Error('go back: there is no previous entry in this browser context history')
  }
  await world.page.goBack()
  await waitForRender(world)
})

/**
 * `click "<locator>"` — press the element, then let the page catch up on the
 * same two conditions `visit` does: network quiet and the hydration marker.
 * That trailing wait is what makes a click composable — see minimart's
 * interaction.steps.ts for the measured failure mode it replaces.
 */
When(new RegExp(`^click "${LOCATOR_PATTERN}"$`), async ({ world }, token: string) => {
  const resolved = resolveLocator(world.page, token, world)
  await act(`click ${resolved.description}`, () =>
    resolved.locator.click({ timeout: ACTION_TIMEOUT_MS }),
  )
  await waitForRender(world)
})

When(
  new RegExp(`^fill "${LOCATOR_PATTERN}" with "([^"]*)"$`),
  async ({ world }, token: string, value: string) => {
    const resolved = resolveLocator(world.page, token, world)
    const text = world.resolveVars(value)
    await act(`fill ${resolved.description} with "${text}"`, () =>
      resolved.locator.fill(text, { timeout: ACTION_TIMEOUT_MS }),
    )
  },
)

/**
 * `press "<key>"` — sent to whatever has focus, which is the accessible path a
 * click-only test never covers (login.feature's "press Enter" submit).
 */
When(/^press "([A-Za-z0-9+]+)"$/, async ({ world }, key: string) => {
  await act(`press "${key}"`, () => world.page.keyboard.press(key))
})

/** Runs one interaction, prefixing any failure with the step as the feature wrote it. */
async function act(step: string, action: () => Promise<unknown>): Promise<void> {
  try {
    await action()
  } catch (error) {
    throw new Error(`${step}: ${describe(error)}`)
  }
}

function describe(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}
