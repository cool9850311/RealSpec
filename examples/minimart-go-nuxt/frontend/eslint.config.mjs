// ESLint, scoped deliberately narrowly.
//
// WHAT THIS FILE IS FOR
// ---------------------
// Two assertions about the i18n catalogue that used to be regular expressions
// in scripts/check-i18n-keys.mjs. They belong here instead because they are
// per-file, per-node questions, and this is the tool that answers those with an
// AST rather than a grep — it sees `t('shop.title')` in a `<script setup
// lang="ts">` block the way the compiler does, and it reports the finding on the
// line that caused it, in the editor, while you type.
//
// The rules come from @intlify, the same people who make vue-i18n, so they track
// the library rather than lagging behind a hand-written scanner.
//
// WHAT THIS FILE IS DELIBERATELY NOT FOR
// --------------------------------------
// Code style. No `vue/*`, no `@typescript-eslint/*`, no formatting rules, and no
// `@nuxt/eslint` — that module is another 272 packages and a whole opinion about
// how Vue should be written, which has nothing to do with what this example
// demonstrates. A reader here is trying to understand a specification standard;
// every rule that is not load-bearing for that is a rule they have to read past.
//
// If this project ever wants general linting, that is a separate decision and a
// separate config block. Do not let it arrive by accident, one "while we're here"
// rule at a time.
//
// WHAT IS *NOT* HERE, AND WHY
// ---------------------------
// Two checks stayed as standalone scripts because their inputs are not
// JavaScript and ESLint therefore cannot see them at all:
//
//   scripts/check-testids.mjs        reads spec/bdd/e2e/*.feature
//   scripts/check-static-bundle.mjs  reads the built .output/public
//
// And scripts/check-i18n-keys.mjs keeps the one assertion with the same
// property: that every i18n key a feature file names exists. No linter has the
// concept of a Gherkin file naming an i18n key.
import vueI18n from '@intlify/eslint-plugin-vue-i18n'
import vueParser from 'vue-eslint-parser'
import tsParser from '@typescript-eslint/parser'

export default [
  {
    ignores: [
      '.nuxt/**',
      '.output/**',
      '.features-gen/**',
      'dist/**',
      'node_modules/**',
    ],
  },

  ...vueI18n.configs['flat/base'],

  // Shared by every block below. `localeDir` must be visible to the JSON block
  // as well as the source block — the catalogue rules read it to find the other
  // locales, and without it they report a configuration error instead of doing
  // their job.
  {
    settings: {
      'vue-i18n': {
        localeDir: './i18n/*.json',
        messageSyntaxVersion: '^11.0.0',
      },
    },
  },

  {
    files: ['**/*.{js,ts,vue}'],

    languageOptions: {
      // vue-eslint-parser reads the SFC; the TypeScript parser reads the script
      // block inside it. Without the second one every `<script setup lang="ts">`
      // is a parse error, and a rule that cannot parse a file silently checks
      // nothing in it.
      parser: vueParser,
      parserOptions: {
        parser: tsParser,
        ecmaVersion: 'latest',
        sourceType: 'module',
      },
    },

    rules: {
      // Was assertion D. A key handed to the translator must exist in the
      // catalogue. Without this the page renders the raw key, which no locator
      // matches — a red e2e test, eventually, on whichever screen happens to be
      // covered.
      //
      // This one reports at the `t('…')` call, so it belongs on the source glob.
      '@intlify/vue-i18n/no-missing-keys': 'error',
    },
  },

  // The other two rules report on the CATALOGUE, not on the code — the finding
  // is "this JSON file is missing a key" or "this JSON file defines a key
  // nobody wants", and the line number points into en_US.json. So they have to
  // be enabled for the locale files, which flat/base parses with
  // jsonc-eslint-parser.
  //
  // Getting this wrong is silent: with these rules on the `{js,ts,vue}` block
  // instead, ESLint exits 0 and looks like it is working while checking
  // nothing. That is how the first draft of this file shipped, and it was
  // caught by deliberately breaking each rule in turn and demanding a failure —
  // which is the only way to know a check is not a no-op.
  {
    files: ['i18n/*.json'],

    rules: {
      // Was assertion A. The locales must define the same keys. Runtime only
      // catches this in a locale some scenario actually exercises, so a locale
      // nobody tests can rot silently.
      '@intlify/vue-i18n/no-missing-keys-in-other-locales': 'error',

      // `no-unused-keys` is DELIBERATELY ABSENT. It was enabled, and it reported
      // eight false positives immediately — `auth.invalidCredentials`,
      // `shop.insufficientPoints`, `orders.loadFailed` and five siblings. All
      // eight are used; they are assigned as string literals to an error-key ref
      // and translated later:
      //
      //     INSUFFICIENT_POINTS: 'shop.insufficientPoints',   // pages/products.vue
      //     listErrorKey.value = 'orders.loadFailed'          // pages/orders.vue
      //
      // The rule looks for keys passed to `t()` and cannot follow that
      // indirection. Silencing it would mean eight `ignores` entries on day one
      // of a three-page application, which is precisely the maintenance rot that
      // makes a check get switched off later, for real findings as well as
      // false ones.
      //
      // It is not worth that, because an unused key cannot make anything fail.
      // It is tidiness, and tidiness does not get to cost correctness signal.
      // The keys it flags are covered anyway: `no-missing-keys` proves every key
      // the code asks for exists, which is the direction that matters.
    },
  },
]
