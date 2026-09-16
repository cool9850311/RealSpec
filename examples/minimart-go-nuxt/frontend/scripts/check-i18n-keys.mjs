#!/usr/bin/env node
// ─────────────────────────────────────────────────────────────────────────────
// Every `@namespace.key` locator in spec/bdd/e2e/*.feature must exist in the
// i18n catalogue.
//
//   npm run check:i18n
//
// This file made five assertions once. Three are ESLint rules now (see
// ../eslint.config.mjs): `t('…')` keys exist, the locales agree, and unused
// keys — dropped, because an unused key cannot fail. A fourth read a list of
// keys out of the design document's prose; it is gone because no check here
// parses Markdown, and because agreeing with a list was never the guarantee
// worth having. The e2e features assert the translated text itself.
//
// What is left is the part no linter can do. A Gherkin step addresses a button
// as `button:@shop.redeem`, and the accessible name that resolves to comes from
// the catalogue at run time; nothing in the JavaScript ecosystem models that
// link, so this file does.
import { readdirSync, readFileSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const frontendDir = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const featureDir = resolve(frontendDir, '..', 'spec', 'bdd', 'e2e')
const LOCALES = ['en_US', 'zh_TW']

function flatten(value, prefix = '') {
  if (value === null || typeof value !== 'object') return new Map([[prefix, value]])
  const out = new Map()
  for (const [key, child] of Object.entries(value)) {
    for (const [path, leaf] of flatten(child, prefix ? `${prefix}.${key}` : key)) out.set(path, leaf)
  }
  return out
}

// The union across locales, not the intersection: whether the two catalogues
// agree is no longer this file's business — no-missing-keys-in-other-locales
// owns that, and duplicating it here would mean two places to fix.
const defined = new Set(
  LOCALES.flatMap((locale) => [
    ...flatten(JSON.parse(readFileSync(join(frontendDir, 'i18n', `${locale}.json`), 'utf8'))).keys(),
  ]),
)

const failures = []
let locatorKeys = 0
for (const name of readdirSync(featureDir).filter((f) => f.endsWith('.feature'))) {
  const pattern = /[a-z]+:@([a-z][a-zA-Z0-9]*(?:\.[a-zA-Z][a-zA-Z0-9_]*)+)/g
  for (const [, key] of readFileSync(join(featureDir, name), 'utf8').matchAll(pattern)) {
    locatorKeys += 1
    if (!defined.has(key)) failures.push(`"${key}" is referenced by ${name} but defined in no locale file`)
  }
}

console.log(`e2e features reference ${locatorKeys} "@key" locators`)
console.log(`catalogue defines ${defined.size} keys across ${LOCALES.length} locales`)
if (failures.length > 0) {
  console.error(`\ncheck:i18n FAILED — ${failures.length} problem(s):`)
  for (const message of failures) console.error(`  ${message}`)
  process.exit(1)
}
console.log('\ncheck:i18n passed')
