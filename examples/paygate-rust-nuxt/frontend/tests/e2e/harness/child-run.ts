// Starting a nested Playwright run, and reading its result back.
//
// Two properties the harness self-test needs cannot be observed from inside
// the run being judged: "the negative features fail, with these messages" and
// "a run gives every scenario its own port and its own database, shares one
// network, and Ryuk eventually reaps every container". So the self-test starts
// a whole run as a child process and inspects its JSON report and the Docker
// daemon afterwards.
//
// Unlike examples/minimart-go-nuxt/frontend, there is no run-wide frontend
// container for the child to adopt, and no environment variable to hand it
// the parent's shared network either: the child needs only its own run id, to
// keep its containers' labels distinct from the parent's — its own
// global-setup finds the very same network the parent is using by reading
// `env.ts`'s `networkStateFile()`, a path outside either run's own state
// (stack.ts, `ensureRunNetwork`).

import { spawn } from 'node:child_process'
import fs from 'node:fs'
import path from 'node:path'
import { createRequire } from 'node:module'
import { ENV, FRONTEND_DIR, REPORTS_DIR, runId } from '../env'

// The runner's own CLI, started with this process's node rather than through
// `npx`. `cli.js` is not in the package's `exports` map, so it is reached
// through the package directory rather than as a subpath import.
const require_ = createRequire(import.meta.url)
const PLAYWRIGHT_CLI = path.join(path.dirname(require_.resolve('@playwright/test')), 'cli.js')

/** One test of the child run, flattened out of the JSON report's suite tree. */
export interface ChildTest {
  title: string
  file: string
  status: string
  errors: string[]
  attachments: Array<{ name: string; path?: string; contentType: string }>
}

export interface ChildRun {
  project: string
  /** The child's own run id; every container/network it creates carries it. */
  runId: string
  /** Where the child's scenarios recorded their addresses and networks. */
  stateDir: string
  exitCode: number
  output: string
  tests: ChildTest[]
  workers: number | undefined
}

/**
 * Runs exactly one project of this same Playwright configuration as a child
 * process and returns what it did.
 *
 * `bddgen` is deliberately not re-run: the generated files are already on disk
 * from the parent's generation step.
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
      [ENV.runId]: childRunId,
      PLAYWRIGHT_JSON_OUTPUT_NAME: reportFile,
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
