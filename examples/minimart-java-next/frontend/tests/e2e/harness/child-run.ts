// Starting a nested Playwright run, and reading its result back.
//
// Two of PLAN §6.B's cases are about the run as a whole rather than about any
// scenario in it — "the negative features fail, with these messages" and "a
// four-worker run gives every scenario its own port and leaves nothing behind" —
// and neither can be observed from inside the run being judged. So the harness
// self-test starts one as a child process and inspects its JSON report and the
// Docker daemon afterwards.
//
// The child ADOPTS this run's shared infrastructure (one network, one frontend
// container) and is given its OWN run id. That separation is what makes the leak
// assertion exact: everything the child created carries the child's label, so
// "nothing of the child's is left" is a question with a yes/no answer and not a
// matter of excluding the parent's containers from the count.

import { spawn } from 'node:child_process'
import fs from 'node:fs'
import path from 'node:path'
import { createRequire } from 'node:module'
import { ENV, FRONTEND_DIR, REPORTS_DIR, runId } from '../env'

// The runner's own CLI, started with this process's node rather than through
// `npx`: one process instead of three, and no dependence on PATH.
// `cli.js` is not in the package's `exports` map, so it is reached through the
// package directory rather than as a subpath import.
const require_ = createRequire(import.meta.url)
const PLAYWRIGHT_CLI = path.join(path.dirname(require_.resolve('@playwright/test')), 'cli.js')

/** One test of the child run, flattened out of the JSON report's suite tree. */
export interface ChildTest {
  title: string
  file: string
  /** 'passed' | 'failed' | 'timedOut' | 'skipped' | … — the last attempt's. */
  status: string
  /** Every error message the test produced, newest attempt last. */
  errors: string[]
  attachments: Array<{ name: string; path?: string; contentType: string }>
}

export interface ChildRun {
  project: string
  /** The child's own run id; everything it created in Docker carries it. */
  runId: string
  /** Where the child's scenarios recorded their hosts and ports. */
  stateDir: string
  exitCode: number
  /** stdout and stderr, for a failure message that is worth reading. */
  output: string
  tests: ChildTest[]
  /** Worker count the child actually configured. */
  workers: number | undefined
}

/**
 * Runs exactly one project of this same Playwright configuration as a child
 * process and returns what it did.
 *
 * `bddgen` is deliberately not re-run: the generated files are already on disk
 * from the parent's generation step, and regenerating them underneath a running
 * suite would be a race for no benefit.
 */
export async function runChildSuite(
  project: string,
  label: string,
  extraArgs: string[] = [],
): Promise<ChildRun> {
  const childRunId = `${runId()}-${label}`
  const reportFile = path.join(REPORTS_DIR, `child-${label}.json`)
  const outputDir = path.join(REPORTS_DIR, `child-${label}-results`)
  fs.rmSync(reportFile, { force: true })
  fs.rmSync(outputDir, { recursive: true, force: true })

  const args = [
    PLAYWRIGHT_CLI,
    'test',
    `--project=${project}`,
    '--reporter=json',
    `--output=${outputDir}`,
    ...extraArgs,
  ]

  const child = spawn(process.execPath, args, {
    cwd: FRONTEND_DIR,
    env: {
      ...process.env,
      [ENV.childProject]: project,
      [ENV.adoptShared]: '1',
      [ENV.runId]: childRunId,
      PLAYWRIGHT_JSON_OUTPUT_NAME: reportFile,
      // The child must not inherit a parent worker's idea of where artefacts go.
      PLAYWRIGHT_HTML_OPEN: 'never',
    },
    stdio: ['ignore', 'pipe', 'pipe'],
  })

  let output = ''
  child.stdout.on('data', (chunk: Buffer) => {
    output += chunk.toString()
  })
  child.stderr.on('data', (chunk: Buffer) => {
    output += chunk.toString()
  })
  const exitCode = await new Promise<number>((resolve, reject) => {
    child.on('error', reject)
    child.on('close', (code) => resolve(code ?? -1))
  })

  const report = readReport(reportFile)
  return {
    project,
    runId: childRunId,
    stateDir: path.join(REPORTS_DIR, 'run-state', childRunId),
    exitCode,
    output,
    tests: flattenTests(report),
    workers: report?.config?.workers,
  }
}

// ── The JSON report, as much of it as is needed ──────────────────────────────

interface JsonReport {
  config?: { workers?: number }
  suites?: JsonSuite[]
}

interface JsonSuite {
  title?: string
  file?: string
  specs?: JsonSpec[]
  suites?: JsonSuite[]
}

interface JsonSpec {
  title: string
  file?: string
  tests?: Array<{ status?: string; results?: JsonResult[] }>
}

interface JsonResult {
  status?: string
  errors?: Array<{ message?: string }>
  error?: { message?: string }
  attachments?: Array<{ name: string; path?: string; contentType: string }>
}

function readReport(file: string): JsonReport | undefined {
  if (!fs.existsSync(file)) return undefined
  return JSON.parse(fs.readFileSync(file, 'utf8')) as JsonReport
}

function flattenTests(report: JsonReport | undefined): ChildTest[] {
  const collected: ChildTest[] = []
  const walk = (suites: JsonSuite[] | undefined, file: string): void => {
    for (const suite of suites ?? []) {
      const suiteFile = suite.file ?? file
      for (const spec of suite.specs ?? []) {
        const results = (spec.tests ?? []).flatMap((test) => test.results ?? [])
        // The LAST attempt's own status, not the spec's 'expected'/'unexpected'
        // verdict: "did this step fail" is the question the harness asks, and
        // `retries` is 0 so there is exactly one attempt anyway.
        const status = results[results.length - 1]?.status ?? 'unknown'
        collected.push({
          title: spec.title,
          file: spec.file ?? suiteFile,
          status,
          errors: results.flatMap((result) => [
            ...(result.errors ?? []).map((error) => error.message ?? ''),
            ...(result.error?.message ? [result.error.message] : []),
          ]),
          attachments: results.flatMap((result) => result.attachments ?? []),
        })
      }
      walk(suite.suites, suiteFile)
    }
  }
  walk(report?.suites, '')
  return collected
}

/** The one test whose title matches, or a failure naming what was there instead. */
export function testNamed(run: ChildRun, title: string): ChildTest {
  const matches = run.tests.filter((test) => test.title === title)
  if (matches.length === 1) return matches[0]
  const present = run.tests.map((test) => `  - ${test.title} [${test.status}]`).join('\n')
  throw new Error(
    `the ${run.project} run reported ${matches.length} tests titled "${title}".\n` +
      `Tests reported:\n${present || '  (none)'}\nOutput:\n${run.output}`,
  )
}

/** Every error message of one test, joined, for substring assertions. */
export function errorText(test: ChildTest): string {
  return test.errors.join('\n---\n')
}
