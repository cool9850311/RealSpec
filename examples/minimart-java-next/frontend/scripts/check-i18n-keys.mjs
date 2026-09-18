#!/usr/bin/env node
// ─────────────────────────────────────────────────────────────────────────────
// Every `@namespace.key` locator in spec/bdd/e2e/*.feature must exist in the
// i18n catalogue, and the two catalogues must define the same keys.
//
//   npm run check:i18n
//
// A Gherkin step addresses a button as `button:@shop.redeem`, and the
// accessible name that resolves to comes from the catalogue at run time;
// nothing in the TypeScript toolchain models that link, so this file does.
//
// The other two links are split between this file and the compiler:
//   - `t('…')` keys in the app exist in en_US: `npm run check:types`, through
//     the next-intl AppConfig declaration in global.d.ts.
//   - en_US and zh_TW define the same keys: here, both ways. The type of
//     i18n/index.ts's MESSAGES catches a key missing from zh_TW, but not an
//     extra one — an excess key in an imported JSON file is not a type error —
//     and an extra key is a translation nobody can reach, usually a rename
//     that was applied to one catalogue only.
//
// No ESLint here, unlike the Nuxt front end: its i18n rules are a Vue plugin,
// and the two properties they enforced are covered above.
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

const catalogues = new Map(
  LOCALES.map((locale) => [
    locale,
    new Set(flatten(JSON.parse(readFileSync(join(frontendDir, 'i18n', `${locale}.json`), 'utf8'))).keys()),
  ]),
)

const failures = []

// ── the catalogues agree, both ways ──────────────────────────────────────────

for (const [locale, keys] of catalogues) {
  for (const [other, otherKeys] of catalogues) {
    if (other === locale) continue
    for (const key of keys) {
      if (!otherKeys.has(key)) failures.push(`"${key}" is defined in ${locale}.json but not in ${other}.json`)
    }
  }
}

// ── every @key a feature uses is defined, in every locale ────────────────────

let locatorKeys = 0
for (const name of readdirSync(featureDir).filter((f) => f.endsWith('.feature'))) {
  const pattern = /[a-z]+:@([a-z][a-zA-Z0-9]*(?:\.[a-zA-Z][a-zA-Z0-9_]*)+)/g
  for (const [, key] of readFileSync(join(featureDir, name), 'utf8').matchAll(pattern)) {
    locatorKeys += 1
    for (const [locale, keys] of catalogues) {
      if (!keys.has(key)) failures.push(`"${key}" is referenced by ${name} but not defined in ${locale}.json`)
    }
  }
}

console.log(`e2e features reference ${locatorKeys} "@key" locators`)
for (const [locale, keys] of catalogues) console.log(`${locale}.json defines ${keys.size} keys`)
if (failures.length > 0) {
  console.error(`\ncheck:i18n FAILED — ${failures.length} problem(s):`)
  for (const message of failures) console.error(`  ${message}`)
  process.exit(1)
}
console.log('\ncheck:i18n passed')
