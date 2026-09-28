// ESLint, scoped deliberately narrowly.
//
// WHAT THIS FILE IS FOR
// ---------------------
// Two assertions about the i18n catalogue that would otherwise be regular
// expressions in scripts/check-i18n-keys.mjs. They belong here instead because
// they are per-file, per-node questions, and this is the tool that answers
// those with an AST rather than a grep — it sees `t('shop.pay')` in a
// `<script setup lang="ts">` block the way the compiler does, and it reports
// the finding on the line that caused it, in the editor, while you type.
//
// The rules come from @intlify, the same people who make vue-i18n, so they
// track the library rather than lagging behind a hand-written scanner. This
// file mirrors examples/minimart-go-nuxt/frontend/eslint.config.mjs, which
// explains the choices below at greater length.
//
// WHAT THIS FILE IS DELIBERATELY NOT FOR
// --------------------------------------
// Code style. No `vue/*`, no `@typescript-eslint/*`, no formatting rules, and
// no `@nuxt/eslint`. A reader here is trying to understand a specification
// standard; every rule that is not load-bearing for that is a rule they have
// to read past.
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
      parser: vueParser,
      parserOptions: {
        parser: tsParser,
        ecmaVersion: 'latest',
        sourceType: 'module',
      },
    },

    rules: {
      // A key handed to the translator must exist in the catalogue. Without
      // this the page renders the raw key, which no locator matches — a red
      // e2e test, eventually, on whichever screen happens to be covered.
      '@intlify/vue-i18n/no-missing-keys': 'error',
    },
  },

  // The other rule reports on the CATALOGUE, not on the code: the finding is
  // "this JSON file is missing a key", and the line number points into
  // en_US.json or zh_TW.json, so it has to be enabled for the locale files,
  // which flat/base parses with jsonc-eslint-parser.
  {
    files: ['i18n/*.json'],

    rules: {
      // The locales must define the same keys. Runtime only catches this in a
      // locale some scenario actually exercises, so a locale nobody tests can
      // rot silently.
      '@intlify/vue-i18n/no-missing-keys-in-other-locales': 'error',

      // `no-unused-keys` is DELIBERATELY ABSENT — see minimart-go-nuxt's own
      // config for the false positives it produced there (a key assigned as a
      // string literal to an error-key ref and translated later, which the
      // rule cannot follow). It is not worth eight `ignores` entries on day
      // one; `no-missing-keys` already proves every key the code asks for
      // exists, which is the direction that matters.
    },
  },
]
