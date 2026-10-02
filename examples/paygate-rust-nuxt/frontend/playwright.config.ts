// The e2e runner: playwright-bdd generates Playwright tests from the feature
// files, and the Playwright runner runs them.
//
//   npx bddgen && npx playwright test
//
// `bddgen` is not a convenience. It is the stage that fails when a feature uses
// a step nobody implemented (`missingSteps: 'fail-on-gen'`, playwright-bdd's
// default), which is half of what `spec.md`, "Layers" calls "Registry parity"
// in its Test plan table — the other half, that nothing EXTRA is implemented,
// is tests/e2e/harness/harness.spec.ts's "registry parity" test, which needs
// no Docker and must pass without it.
//
// ── The projects ─────────────────────────────────────────────────────────────
//
//   spec             the 20 scenarios of spec/bdd/e2e — paygate's own e2e suite
//   harness          this harness's own positive fixtures
//   harness-negative features that MUST fail. Excluded from a normal run,
//                    started as a nested run by harness-meta, which asserts
//                    both that they fail and that the failure message names
//                    the cause.
//   harness-meta     plain Playwright tests that run the two above as child
//                    processes and assert properties no scenario can observe
//                    about itself: parallel isolation, and registry parity.
//
// The fixture features live under tests/e2e/harness, NOT under spec/bdd: they
// test this harness, and nothing about them belongs in paygate's specification.
//
// ── The one deliberate difference from examples/minimart-go-nuxt/frontend ────
//
// That suite disables Ryuk and sweeps its own containers, because its harness
// self-test needs to assert that nothing is left behind without racing another
// reaper for the answer. Here (`spec.md`, "Isolation": containers are
// Testcontainers' to manage, and the Node `testcontainers` package runs Ryuk)
// Ryuk stays enabled and the `testcontainers` package's own reaper is the only
// thing that ever removes a container or a network this suite starts — a
// bespoke sweeper would be exactly the hand-rolled lifecycle management the
// rule forbids. So
// tests/e2e/harness/harness.spec.ts asserts isolation and eventual cleanup by
// POLLING the Docker daemon after a child run exits, never by cleaning it up
// itself. See that file and tests/e2e/docker.ts for the read-only mechanism
// this leaves in place of minimart's sweep.

import path from 'node:path'
import { defineConfig } from '@playwright/test'
import { defineBddProject } from 'playwright-bdd'
import { ASSERTION_TIMEOUT_MS } from './tests/e2e/assertions'
import { ENV, REPORTS_DIR, workerCount } from './tests/e2e/env'

// The step definitions, plus the fixtures file — playwright-bdd reads the
// `test` instance the generated files must import from one of the `steps`
// entries, and ours is produced by `base.extend()` in fixtures.ts.
const STEPS = ['tests/e2e/fixtures.ts', 'tests/e2e/steps/*.steps.ts']

const specProject = defineBddProject({
  name: 'spec',
  features: '../spec/bdd/e2e/*.feature',
  featuresRoot: '../spec/bdd/e2e',
  steps: STEPS,
  outputDir: '.features-gen/spec',
})

const harnessProject = defineBddProject({
  name: 'harness',
  features: 'tests/e2e/harness/features/positive/*.feature',
  featuresRoot: 'tests/e2e/harness/features/positive',
  steps: STEPS,
  outputDir: '.features-gen/harness',
})

const negativeProject = defineBddProject({
  name: 'harness-negative',
  features: 'tests/e2e/harness/features/negative/*.feature',
  featuresRoot: 'tests/e2e/harness/features/negative',
  steps: STEPS,
  outputDir: '.features-gen/harness-negative',
})

const allProjects = [
  { ...specProject },
  { ...harnessProject },
  { ...negativeProject },
  {
    name: 'harness-meta',
    testDir: 'tests/e2e/harness',
    testMatch: /.*\.spec\.ts$/,
    // Each of these starts a whole nested Playwright run (some of them start
    // Docker containers). Running two of them at once would have them
    // competing for the same Docker daemon and make the timings meaningless.
    fullyParallel: false,
    // They read the ledger the scenarios wrote, so every scenario has to have
    // finished first.
    dependencies: ['spec', 'harness'],
    timeout: 20 * 60_000,
  },
]

/**
 * A nested run executes exactly one project, named by the parent
 * (tests/e2e/harness/child-run.ts).
 * A normal run executes everything except the features that are meant to fail.
 */
const childProject = process.env[ENV.childProject]
const projects = childProject
  ? allProjects.filter((project) => project.name === childProject)
  : allProjects.filter((project) => project.name !== 'harness-negative')

if (childProject && projects.length === 0) {
  throw new Error(`${ENV.childProject}=${childProject} names no project in playwright.config.ts`)
}

export default defineConfig({
  testDir: specProject.testDir,
  projects,

  // A scenario's stack is twelve containers, three of which (Kafka,
  // ClickHouse, and the api waiting on both) are slower to become ready than
  // anything examples/minimart-go-nuxt/frontend has to start; this is the
  // budget for the STEPS once the stack fixture (its own, larger budget — see
  // fixtures.ts) has handed one back ready.
  timeout: 180_000,
  expect: { timeout: ASSERTION_TIMEOUT_MS },

  // No retries, ever. A test that passes on the second attempt is a test whose
  // result depends on something nobody has named yet, and hiding that is how a
  // suite stops being evidence. Flakiness is a failure here.
  retries: 0,
  // Parallel on purpose: per-scenario isolation is the claim, and a suite that
  // only passes serially has not demonstrated it. How parallel is the
  // machine's answer, not a constant — see workerCount(), overridable with
  // E2E_WORKERS. spec.md, "Isolation": "half the machine's CPUs."
  workers: workerCount(),
  fullyParallel: true,
  forbidOnly: true,

  globalSetup: './tests/e2e/global-setup.ts',
  globalTeardown: './tests/e2e/global-teardown.ts',

  outputDir: path.join(REPORTS_DIR, 'test-results'),
  reporter: [
    ['list'],
    ['html', { outputFolder: path.join(REPORTS_DIR, 'html'), open: 'never' }],
    ['json', { outputFile: path.join(REPORTS_DIR, 'results.json') }],
  ],

  use: {
    // Caddy's `tls internal` certificate is signed by its own ephemeral CA;
    // this suite deliberately never touches the machine's trust store.
    ignoreHTTPSErrors: true,
    // A trace for every scenario, pass or fail. The HTML report links each
    // one, and a passing scenario's trace is how you check that it passed for
    // the reason you think.
    trace: 'on',
    screenshot: 'only-on-failure',
    video: 'off',
  },
})
