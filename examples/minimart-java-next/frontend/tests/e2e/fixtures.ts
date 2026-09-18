// The wiring: Playwright fixtures that give every scenario its own
// infrastructure, its own browser context and its own world.
//
// Three things are worth reading closely.
//
// 1. `baseURL` is overridden rather than the whole `context` fixture. Playwright
//    builds the context from `baseURL` and friends, and it is the BUILT-IN
//    context fixture that starts and saves the trace, the video and the
//    failure screenshot. Replacing it to inject a per-scenario origin would
//    quietly cost all three. Overriding the option it reads costs nothing.
//
// 2. The stack fixture tears down in a `finally`, so a red scenario returns its
//    containers exactly like a green one. A suite that starts containers owes
//    the machine that.
//
// 3. Artefacts are collected in the teardown of the fixture that owns them, and
//    only when the test did not end as expected. The container logs are the one
//    artefact Playwright cannot produce by itself and the one that explains the
//    failures a screenshot cannot: a backend that refused to boot leaves a blank
//    page and a perfectly uninformative trace.

import fs from 'node:fs'
import { createBdd, test as base } from 'playwright-bdd'
import type { TestInfo } from '@playwright/test'
import { STACK_STARTUP_BUDGET_MS, profileFromTags, startScenarioStack, type ScenarioStack } from './stack'
import { World } from './world'

// Derived from the stage timeouts in stack.ts rather than written out: the
// fixture's budget has to cover the sum of the waits inside it, so that a slow
// JVM boot is reported by the wait that names the backend and not by a generic
// fixture timeout. A fixture with its own timeout does not draw on the
// scenario's 120 s step budget in playwright.config.ts.
const STACK_STARTUP_TIMEOUT_MS = STACK_STARTUP_BUDGET_MS

type ScenarioFixtures = {
  stack: ScenarioStack
  world: World
}

export const test = base.extend<ScenarioFixtures>({
  stack: [
    async ({ $tags }, use, testInfo) => {
      const stack = await startScenarioStack(profileFromTags($tags))
      try {
        await use(stack)
      } finally {
        if (testInfo.status !== testInfo.expectedStatus) {
          await attachContainerLogs(stack, testInfo)
        }
        await stack.stop()
      }
    },
    { timeout: STACK_STARTUP_TIMEOUT_MS },
  ],

  // Every scenario is reached on its own port, so the feature files never name
  // an origin: `visit "/products"` resolves against this.
  baseURL: async ({ stack }, use) => {
    await use(stack.baseURL)
  },

  world: async ({ stack, context, page }, use) => {
    await use(new World(stack, context, page))
  },
})

/**
 * Writes each container's output next to the trace and the screenshot, as a file
 * rather than an in-memory attachment, so it is readable with `cat` and not only
 * through the HTML report.
 */
async function attachContainerLogs(stack: ScenarioStack, testInfo: TestInfo): Promise<void> {
  const logs = await stack.collectLogs()
  for (const [name, body] of Object.entries(logs)) {
    const file = testInfo.outputPath(`container-${name}.log`)
    fs.writeFileSync(file, body)
    await testInfo.attach(`container-${name}.log`, { path: file, contentType: 'text/plain' })
  }
  await testInfo.attach('scenario-stack.json', {
    body: JSON.stringify(
      { index: stack.index, profile: stack.profile, port: stack.port, baseURL: stack.baseURL },
      null,
      2,
    ),
    contentType: 'application/json',
  })
}

export const { Given, When, Then, Step } = createBdd(test)
