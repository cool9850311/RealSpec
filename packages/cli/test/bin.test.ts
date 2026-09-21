/**
 * The built CLI must run when launched through a symlink, the way
 * node_modules/.bin/realspec and `npx realspec` launch it. Node resolves the
 * symlink for import.meta.url but not for process.argv[1]; comparing the two
 * naively made a bin launch do nothing and exit 0 — a false green.
 *
 * This runs dist/cli.js, so it needs `npm run build` first. A missing build
 * fails the test rather than skipping it.
 */

import { spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, rmSync, symlinkSync } from 'node:fs';
import { tmpdir } from 'node:os';
import * as path from 'node:path';
import { fileURLToPath } from 'node:url';
import { afterAll, describe, expect, it } from 'vitest';

const DIST_CLI = fileURLToPath(new URL('../dist/cli.js', import.meta.url));

describe('launch through a symlink', () => {
  const tmp = mkdtempSync(path.join(tmpdir(), 'realspec-bin-'));
  afterAll(() => rmSync(tmp, { recursive: true, force: true }));

  it('runs main() instead of silently exiting 0', () => {
    expect(existsSync(DIST_CLI), `${DIST_CLI} missing — run \`npm run build\` first`).toBe(true);

    const link = path.join(tmp, 'realspec');
    symlinkSync(DIST_CLI, link);

    const result = spawnSync(process.execPath, [link], { encoding: 'utf8' });
    expect(result.stdout + result.stderr).toContain('Usage: realspec validate');
    expect(result.status).toBe(1);
  });
});
