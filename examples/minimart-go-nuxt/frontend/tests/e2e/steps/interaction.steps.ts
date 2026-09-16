// Navigation and interaction: everything a scenario's `When` can do to the page.
//
// Every one of these is strict. Playwright's own actions refuse to act when a
// locator matches more than one element, and the failures below only add what
// Playwright cannot know: which token the feature wrote, and — for an `@key`
// token — which string that key resolved to in the active locale, which is the
// piece a reader of the failure cannot look up for themselves.

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
  // unauthenticated visitor to /login is a legitimate outcome, and the feature
  // states it with `page URL is`.
  await waitForRender(world)
})

When(/^go back$/, async ({ world }) => {
  // The page being left is allowed to finish first. Not politeness: leaving
  // while a prefetch is in flight cancels it, and a cancelled request is one
  // more thing the next assertion would have to reason about.
  await waitForNetworkQuiet(world)
  const entries = await world.page.evaluate(() => window.history.length)
  if (entries <= 1) {
    throw new Error('go back: there is no previous entry in this browser context history')
  }
  await world.page.goBack()
  await waitForRender(world)
})

/**
 * `click "<locator>"` — press the element, then let the page catch up.
 *
 * Playwright's own click waits for the element to be actionable and, when the
 * press starts a document navigation, for that navigation to commit. Neither
 * covers what this application actually does with a click: the sign-in button
 * sends `POST /auth/login`, then `GET /me`, and only then changes the route from
 * inside the SPA. None of that is a document navigation, so without the wait
 * below the step returns while the login is still in flight and the next line
 * races it. Measured, not guessed: the `visit` that followed aborted the POST
 * the browser was about to take its cookie from, the server logged the 200
 * nobody received, and every later request was anonymous.
 *
 * So a click settles exactly the way `visit` does, on the same two conditions —
 * the network quiet and the app's hydration marker. It is a condition and not a
 * duration: a click that changes nothing costs one quiet window. And it belongs
 * here rather than in the features, because a feature that had to know which
 * clicks need waiting for would be encoding this application's internals in
 * every scenario that clicks.
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
 * click-only test never covers.
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
