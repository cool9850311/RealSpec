import { defineConfig } from 'vitest/config'

// The repository is an npm workspace that contains both a vitest package
// (packages/cli) and a Playwright suite (the example's frontend). Vitest and
// Playwright both claim `*.spec.ts`, and Playwright's runner throws when its
// `test.describe` is called from another runner — so a bare `vitest` at the root
// must be told that only the CLI's tests are its business.
//
// `npm test` already scopes correctly (`npm run test -w @realspec/cli`); this
// file makes the unqualified command correct too, rather than leaving a foot-gun.
export default defineConfig({
  test: {
    include: ['packages/*/test/**/*.test.ts'],
    exclude: ['**/node_modules/**', '**/dist/**', '**/.features-gen/**', '**/.output/**', '**/.nuxt/**'],
  },
})
