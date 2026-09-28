#!/usr/bin/env node
// ─────────────────────────────────────────────────────────────────────────────
// Every `testid:` a feature names must exist in the markup.
//
//   npm run check:testids
//
// WHY THIS EXISTS
// ---------------
// Two registered assertions cannot tell "the element is absent" from "the
// assertion holds":
//
//   "testid:x" is hidden        toBeHidden() passes for an element that is not
//                               in the DOM at all — deliberately, so a feature
//                               need not know how the app removed it
//                               (see the e2e harness's assertion.steps.ts)
//   list "testid:x" has 0 rows  a count of zero is a count of zero
//
// A typo in either of those passes at run time and asserts nothing. This file
// defends against exactly that, and brings the red light forward for every
// other testid too, by proving the markup can render one before a browser ever
// has to look for it.
//
// WHAT IT READS
// -------------
//   spec/bdd/e2e/*.feature        every `testid:<token>` in a step or a docstring
//   pages/ layouts/ components/   every data-testid the app can render
//
// Not the design document. No check in this repository parses Markdown: prose
// is for reasons, and a grep over prose asserts whatever the prose happened to
// be phrased like.
//
// THE MATCHING RULE
// -----------------
// From spec/bdd/format.yml: "A `testid:` token matches an element whose
// data-testid is EXACTLY the token, or whose data-testid begins with the token
// followed by a hyphen." So one family of rows is both counted
// (`testid:report-day`) and addressed (`testid:report-day-2026-09-18`), and
// this script has to understand both.
//
// The markup side has two shapes, and both are recognised:
//   data-testid="shop-pay-merchant"                 an exact value
//   :data-testid="`report-day-${day.date}`"         a family, prefix report-day-
import { readdirSync, readFileSync, statSync } from 'node:fs'
import { dirname, extname, join, relative, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const frontendDir = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const exampleDir = resolve(frontendDir, '..')

const SOURCE_DIRS = ['pages', 'layouts', 'components'].map((dir) => join(frontendDir, dir))
const FEATURE_DIR = join(exampleDir, 'spec', 'bdd', 'e2e')

/**
 * `spec/bdd/e2e/*.feature` also drives three origins this Nuxt app never
 * renders (shop.feature's own preamble: "Three origins appear in the journey
 * … paygate is not one of them" — and neither is this frontend, for the
 * provider's and the issuer's own pages). `testid:issuer-amount` is rendered by
 * crates/provider-mock's issuer HTML, a separate Rust service under
 * `/provider/issuer/…`; this script only owns `pages/`, `layouts/` and
 * `components/` of THIS app, so that token is excluded here rather than
 * reported as a false miss. Every other testid this registry names belongs to
 * this frontend and is checked normally.
 */
const RENDERED_ELSEWHERE = new Set(['issuer-amount'])

function walk(dir) {
  let entries
  try {
    entries = readdirSync(dir)
  } catch {
    return []
  }
  return entries.flatMap((entry) => {
    const full = join(dir, entry)
    return statSync(full).isDirectory() ? walk(full) : [full]
  })
}

// ── The markup side ──────────────────────────────────────────────────────────

const exact = new Map() // value -> [locations]
const families = new Map() // prefix (ending in '-') -> [locations]

function recordSource(file) {
  const text = readFileSync(file, 'utf8')
  const where = relative(frontendDir, file)
  for (const match of text.matchAll(/(?<!:)\bdata-testid="([^"${}`]+)"/g)) {
    const list = exact.get(match[1]) ?? []
    list.push(where)
    exact.set(match[1], list)
  }
  for (const match of text.matchAll(/:data-testid="`([^`]*)\$\{[^`]*`"/g)) {
    const prefix = match[1]
    if (!prefix.endsWith('-')) {
      throw new Error(
        `${where}: dynamic data-testid \`${prefix}\${…}\` does not end in a hyphen; ` +
          'the family rule in spec/bdd/format.yml is a prefix on a hyphen boundary.',
      )
    }
    const list = families.get(prefix) ?? []
    list.push(where)
    families.set(prefix, list)
  }
}

for (const dir of SOURCE_DIRS) {
  for (const file of walk(dir)) {
    if (['.vue', '.ts', '.js'].includes(extname(file))) recordSource(file)
  }
}

// ── The feature side ─────────────────────────────────────────────────────────

const required = new Map() // token -> Set(locations)

function recordToken(token, where) {
  const set = required.get(token) ?? new Set()
  set.add(where)
  required.set(token, set)
}

function scanForTokens(file) {
  const text = readFileSync(file, 'utf8')
  const where = relative(exampleDir, file)
  // `{merchantTradeNo}` is a context variable: a value only known at run time,
  // so the token around it asks for a family rather than an element.
  for (const match of text.matchAll(/testid:([a-z][a-z0-9-]*(?:\{[a-zA-Z0-9]+\}[a-z0-9-]*)*)/g)) {
    recordToken(match[1], where)
  }
}

for (const file of walk(FEATURE_DIR)) {
  if (extname(file) === '.feature') scanForTokens(file)
}

// ── The comparison ───────────────────────────────────────────────────────────

/**
 * A token with a runtime segment asks for a family: everything up to the
 * placeholder, which must be a prefix the markup actually renders. A fully
 * literal token asks for an element, which the markup may provide exactly, as
 * a member of a family, or as a family of its own.
 */
function isSatisfied(token) {
  const placeholder = token.indexOf('{')
  if (placeholder !== -1) {
    const literal = token.slice(0, placeholder)
    return (
      [...families.keys()].some((prefix) => prefix === literal || prefix.startsWith(literal)) ||
      [...exact.keys()].some((value) => value.startsWith(literal))
    )
  }
  return (
    exact.has(token) ||
    [...exact.keys()].some((value) => value.startsWith(`${token}-`)) ||
    [...families.keys()].some(
      (prefix) => prefix === `${token}-` || prefix.startsWith(`${token}-`) || token.startsWith(prefix),
    )
  )
}

const missing = [...required.entries()].filter(
  ([token]) => !RENDERED_ELSEWHERE.has(token) && !isSatisfied(token),
)

console.log(`markup: ${exact.size} exact data-testid values, ${families.size} families`)
for (const [value, where] of [...exact].sort()) console.log(`  ${value.padEnd(20)} ${where.join(', ')}`)
for (const [prefix, where] of [...families].sort()) console.log(`  ${`${prefix}<id>`.padEnd(20)} ${where.join(', ')}`)
console.log(`features: ${required.size} distinct testid tokens referenced`)
for (const [token, where] of [...required].sort()) {
  const mark = RENDERED_ELSEWHERE.has(token) ? 'ext  ' : isSatisfied(token) ? 'ok   ' : 'MISS '
  console.log(`  ${mark + token.padEnd(24)} ${[...where].join(', ')}`)
}

if (missing.length > 0) {
  console.error(
    `\ncheck:testids FAILED — ${missing.length} testid token(s) referenced by a feature ` +
      'but rendered by no element:',
  )
  for (const [token, where] of missing) console.error(`  testid:${token}  (${[...where].join(', ')})`)
  process.exit(1)
}

console.log('\ncheck:testids passed')
