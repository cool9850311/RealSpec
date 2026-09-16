// The e2e runner: playwright-bdd generates Playwright tests from the feature
// files, and the Playwright runner runs them.
//
//   npx bddgen && npx playwright test
//
// `bddgen` is not a convenience. It is the stage that fails when a feature uses
// a step nobody implemented (`missingSteps: 'fail-on-gen'`, playwright-bdd's
// default), which is the half of the enforcement `realspec validate` does not
// do: validate proves a step is in the registry, bddgen proves it is in the
// runner, and between them a feature cannot be half-real.
//
// ── The projects ─────────────────────────────────────────────────────────────
//
//   spec             the 10 scenarios of spec/bdd/e2e — minimart's own e2e suite
//   harness          PLAN §6.B's positive cases, as fixture features
//   harness-negative PLAN §6.B's negative cases: features that MUST fail.
//                    Excluded from a normal run, started as a nested run by
//                    harness-meta, which asserts both that they fail and that
//                    the failure message names the cause.
//   harness-meta     plain Playwright tests that run the two above as child
//                    processes and assert properties no scenario can observe
//                    about itself: parallel isolation, and that nothing leaked.
//
// The fixture features live under tests/e2e/harness, NOT under spec/bdd: they
// test this harness, and nothing about them belongs in minimart's specification.

import path from 'node:path'
import { defineConfig } from '@playwright/test'
import { defineBddProject } from 'playwright-bdd'
import { ASSERTION_TIMEOUT_MS } from './tests/e2e/assertions'
import { ENV, REPORTS_DIR, workerCount } from './tests/e2e/env'

// This suite reaps its own containers: every scenario stops its stack in a
// `finally` and global-teardown sweeps anything a crashed worker left behind,
// matching on this run's own label. Ryuk would be a second authority doing the
// same job, and the harness self-test has to be able to say "after this run,
// nothing of mine is left" without racing another reaper to the answer.
process.env.TESTCONTAINERS_RYUK_DISABLED ??= 'true'

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
    // Each of these starts a whole nested Playwright run. Running two of them at
    // once would have them competing for the same Docker daemon and make the
    // timings they measure meaningless.
    fullyParallel: false,
    // They read the ledger the scenarios wrote, so every scenario has to have
    // finished first.
    dependencies: ['spec', 'harness'],
    timeout: 15 * 60_000,
  },
]

/**
 * A nested run executes exactly one project, named by the parent.
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

  // A scenario's stack is built by a fixture with its own budget (see
  // fixtures.ts); this is the budget for the steps.
  timeout: 120_000,
  expect: { timeout: ASSERTION_TIMEOUT_MS },

  // No retries, ever. A test that passes on the second attempt is a test whose
  // result depends on something nobody has named yet, and hiding that is how a
  // suite stops being evidence. Flakiness is a failure here.
  retries: 0,
  // Parallel on purpose: per-scenario isolation is the claim, and a suite that
  // only passes serially has not demonstrated it. How parallel is the machine's
  // answer, not a constant — see workerCount(), overridable with E2E_WORKERS.
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
    // The certificate is a local one covering 127.0.0.1; this suite does not
    // modify the machine's trust store.
    ignoreHTTPSErrors: true,
    // A trace for every scenario, pass or fail. The HTML report links each one,
    // and a passing scenario's trace is how you check that it passed for the
    // reason you think — which is the whole subject of this suite.
    trace: 'on',
    screenshot: 'only-on-failure',
    video: 'off',
  },
})
