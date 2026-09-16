import { defineConfig } from 'vitest/config'

// Scoped to this package so `npm run test -w @realspec/cli` does not inherit the
// repository-root config, whose globs are written relative to the root.
export default defineConfig({
  test: {
    include: ['test/**/*.test.ts'],
  },
})
