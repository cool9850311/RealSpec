/**
 * The shipped examples must validate clean under their own format.yml, with the
 * OpenAPI cross-check against their own spec/openapi included.
 *
 * Two surfaces per example: the spec features (registry discovered by walking
 * up) and the e2e harness features (registry passed with --format).
 */

import { existsSync, readdirSync } from 'node:fs';
import * as path from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

import { run } from '../src/cli.js';

const REPO_ROOT = path.resolve(fileURLToPath(new URL('.', import.meta.url)), '..', '..', '..');

const EXAMPLES = ['minimart-go-nuxt', 'minimart-java-next', 'paygate-rust-nuxt'] as const;

/** Every `.feature` under `dir`, as repo-relative POSIX paths, sorted. */
function featuresUnder(dir: string): string[] {
  const found: string[] = [];
  const walk = (abs: string): void => {
    for (const entry of readdirSync(abs, { withFileTypes: true })) {
      if (entry.name === 'node_modules') continue;
      const full = path.join(abs, entry.name);
      if (entry.isDirectory()) walk(full);
      else if (entry.isFile() && entry.name.endsWith('.feature')) {
        found.push(path.relative(REPO_ROOT, full).split(path.sep).join('/'));
      }
    }
  };
  walk(path.join(REPO_ROOT, dir));
  return found.sort();
}

function expectAllPass(args: string[], fileCount: number): void {
  const r = run(args, { cwd: REPO_ROOT });
  expect(r.stderr).toBe('');
  expect(r.stdout).not.toContain('FAIL');
  expect(r.exitCode).toBe(0);
  expect(r.stdout.split('(0 violation(s))').length - 1).toBe(fileCount);
}

describe.each(EXAMPLES)('example %s', (example) => {
  const specDir = `examples/${example}/spec/bdd`;
  const harnessDir = `examples/${example}/frontend/tests/e2e/harness/features`;
  const format = `${specDir}/format.yml`;

  it('has a format.yml', () => {
    expect(existsSync(path.join(REPO_ROOT, format))).toBe(true);
  });

  it('validates every spec feature', () => {
    const files = featuresUnder(specDir);
    expect(files.length).toBeGreaterThan(0);
    expectAllPass(['validate', ...files], files.length);
  });

  it('validates every e2e harness feature against the spec registry', () => {
    // All three examples ship a harness today; a vanished directory is a bug
    // in the example, not something to skip silently.
    expect(existsSync(path.join(REPO_ROOT, harnessDir))).toBe(true);
    const files = featuresUnder(harnessDir);
    expect(files.length).toBeGreaterThan(0);
    expectAllPass(['validate', ...files, '--format', format], files.length);
  });
});
