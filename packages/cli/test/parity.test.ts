/**
 * §6.A case 21 — the acceptance gate.
 *
 * Runs the CLI over every `.feature` file of the corpus this validator was
 * ported from and asserts (a) zero violations and (b) stdout byte-identical to
 * `python3 validate.py` over the same files. Absolute paths are passed to both
 * sides, so nothing needs normalising: any difference at all is a real one.
 *
 * That corpus belongs to another project and is not vendored here, so the
 * directory is named by REALSPEC_PARITY_CORPUS. Without it the suite says so
 * and moves on — a parity test with nothing to compare against is not a pass,
 * and pretending otherwise would be the exact failure this repository is about.
 * The corpus is read-only; nothing here writes to it.
 */

import { spawnSync } from 'node:child_process';
import { existsSync, readdirSync } from 'node:fs';
import * as path from 'node:path';
import { describe, expect, it } from 'vitest';

import { run } from '../src/cli.js';

const BDD_DIR = process.env.REALSPEC_PARITY_CORPUS ?? '';
const FORMAT_YML = path.join(BDD_DIR, 'format.yml');
const VALIDATE_PY = path.join(BDD_DIR, 'validate.py');

function featureFiles(): string[] {
  if (BDD_DIR === '') return [];
  return readdirSync(BDD_DIR)
    .filter((name) => name.endsWith('.feature'))
    .sort()
    .map((name) => path.join(BDD_DIR, name));
}

const corpusPresent = BDD_DIR !== '' && existsSync(FORMAT_YML) && existsSync(VALIDATE_PY);

describe.skipIf(!corpusPresent)('21. parity with validate.py over the real corpus', () => {
  it('has the reference corpus available', () => {
    expect(existsSync(FORMAT_YML)).toBe(true);
    expect(existsSync(VALIDATE_PY)).toBe(true);
    expect(featureFiles().length).toBeGreaterThan(0);
  });

  it('reports zero violations across every feature file', () => {
    const files = featureFiles();
    const result = run(['validate', '--format', FORMAT_YML, ...files]);

    expect(result.stderr).toBe('');
    expect(result.exitCode).toBe(0);
    expect(result.stdout).not.toContain('FAIL');
    expect(result.stdout.split('(0 violation(s))').length - 1).toBe(files.length);
  });

  it('produces byte-identical stdout to python3 validate.py', () => {
    const files = featureFiles();

    const reference = spawnSync('python3', [VALIDATE_PY, ...files], {
      cwd: BDD_DIR,
      encoding: 'utf8',
    });
    expect(reference.error).toBeUndefined();
    expect(reference.stderr).toBe('');
    expect(reference.status).toBe(0);

    const ported = run(['validate', '--format', FORMAT_YML, ...files]);

    expect(ported.stdout).toBe(reference.stdout);
    expect(ported.exitCode).toBe(reference.status);
  });
});
