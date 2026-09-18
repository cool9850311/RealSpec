#!/usr/bin/env node
// ─────────────────────────────────────────────────────────────────────────────
// Every `testid:` a feature names must exist in the markup.
//
//   npm run check:testids
//
// WHY THIS EXISTS
// ---------------
// Not for the reason it is tempting to give. It is NOT what stops the bug PLAN
// §1.2 describes — 14 data-testid values referenced by tests that no element
// ever carried, surviving because each was one alternative in a comma-separated
// fallback chain that quietly matched `tbody tr` instead. The locator grammar
// already makes that unwritable, and under it a testid matching nothing simply
// times out: red, loudly, no help needed.
//
// The real reason is narrower, and it is a genuine hole. Two registered
// assertions cannot tell "the element is absent" from "the assertion holds":
//
//   "testid:x" is hidden        toBeHidden() passes for an element that is not
//                               in the DOM at all — deliberately, so a feature
//                               need not know how the app removed it
//                               (see tests/e2e/steps/assertion.steps.ts)
//   list "testid:x" has 0 rows  a count of zero is a count of zero
//
// A typo in either of those passes at run time and asserts nothing. minimart's
// own features use them 5 and 1 times. Those six assertions are what this file
// defends; for the other forty-odd it is only bringing a red light forward,
// which is worth having but is not the argument.
//
// WHAT IT READS
// -------------
//   spec/bdd/e2e/*.feature        every `testid:<token>` in a step or a docstring
//   app/ components/ lib/         every data-testid the app can render (.tsx, .ts)
//
// Not the design document. No check in this repository parses Markdown: prose
// is for reasons, and a grep over prose asserts whatever the prose happened to
// be phrased like. The only tokens it ever contributed here were three
// artefacts of its own `<id>` placeholder notation.
//
// THE MATCHING RULE
// -----------------
// From format.yml: "A `testid:` token matches an element whose data-testid is
// EXACTLY the token, or whose data-testid begins with the token followed by a
// hyphen." So one family of rows is both counted (`testid:product-row`) and
// addressed (`testid:product-row-3`), and this script has to understand both.
//
// The markup side has two shapes in JSX, and both are recognised:
//   data-testid="product-name"                      an exact value
//   data-testid={`product-row-${product.id}`}       a family, prefix product-row-
// A data-testid written any other way — a variable, a call, a string built
// with + — is reported as an error rather than skipped: this file cannot see
// what it evaluates to, and silently not seeing it is how a typo would pass.
import { readdirSync, readFileSync, statSync } from 'node:fs'
import { dirname, extname, join, relative, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const frontendDir = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const exampleDir = resolve(frontendDir, '..')

const SOURCE_DIRS = ['app', 'components', 'lib'].map((dir) => join(frontendDir, dir))
const FEATURE_DIR = join(exampleDir, 'spec', 'bdd', 'e2e')

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
  for (const match of text.matchAll(/\bdata-testid=("([^"]*)"|\{`([^`]*)`\}|\S*)/g)) {
    const [, raw, value, template] = match
    if (value !== undefined) {
      const list = exact.get(value) ?? []
      list.push(where)
      exact.set(value, list)
      continue
    }
    if (template === undefined) {
      throw new Error(
        `${where}: data-testid=${raw} is neither a string literal nor a template literal; ` +
          'write it as data-testid="x" or data-testid={`x-${…}`} so this check can read it.',
      )
    }
    if (!template.includes('${')) {
      const list = exact.get(template) ?? []
      list.push(where)
      exact.set(template, list)
      continue
    }
    const prefix = template.slice(0, template.indexOf('${'))
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
    if (['.tsx', '.ts'].includes(extname(file))) recordSource(file)
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
  // `{orderId}` is a context variable: a value only known at run time, so the
  // token around it asks for a family rather than an element.
  for (const match of text.matchAll(/testid:([a-z][a-z0-9-]*(?:\{[a-zA-Z0-9]+\}[a-z0-9-]*)*)/g)) {
    recordToken(match[1], where)
  }
}

for (const file of walk(FEATURE_DIR)) {
  if (extname(file) === '.feature') scanForTokens(file)
}

// ── The comparison ───────────────────────────────────────────────────────────

/**
 * A token with a runtime segment (`order-row-{orderId}`) asks for a family:
 * everything up to the placeholder, which must be a prefix the
 * markup actually renders. A fully literal token asks for an element, which the
 * markup may provide exactly, as a member of a family, or as a family of its
 * own (`product-row` is satisfied by rows named product-row-<id>).
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

const missing = [...required.entries()].filter(([token]) => !isSatisfied(token))

console.log(`markup: ${exact.size} exact data-testid values, ${families.size} families`)
for (const [value, where] of [...exact].sort()) console.log(`  ${value.padEnd(20)} ${where.join(', ')}`)
for (const [prefix, where] of [...families].sort()) console.log(`  ${`${prefix}<id>`.padEnd(20)} ${where.join(', ')}`)
console.log(`features: ${required.size} distinct testid tokens referenced`)
for (const [token, where] of [...required].sort()) {
  console.log(`  ${(isSatisfied(token) ? 'ok   ' : 'MISS ') + token.padEnd(24)} ${[...where].join(', ')}`)
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
