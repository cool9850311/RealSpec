// Tests for check-spec-parity.mjs. Each case copies the real go-nuxt spec twice
// into a temporary directory, mutates one side, and runs the check in-process.
//
//   node --test scripts/check-spec-parity.test.mjs

import { test, before, after } from 'node:test';
import assert from 'node:assert/strict';
import { cp, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { checkSpecParity } from './check-spec-parity.mjs';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = path.resolve(HERE, '..');
const SOURCE_SPEC = path.join(REPO_ROOT, 'examples', 'minimart-go-nuxt', 'spec');

let tmpRoot;
let counter = 0;

before(async () => {
  tmpRoot = await mkdtemp(path.join(os.tmpdir(), 'spec-parity-'));
});

after(async () => {
  if (tmpRoot) await rm(tmpRoot, { recursive: true, force: true });
});

async function fixture() {
  const dir = path.join(tmpRoot, `case-${++counter}`);
  const a = path.join(dir, 'a');
  const b = path.join(dir, 'b');
  await cp(SOURCE_SPEC, path.join(a, 'spec'), { recursive: true });
  await cp(SOURCE_SPEC, path.join(b, 'spec'), { recursive: true });
  return { a, b };
}

async function edit(file, from, to) {
  const text = await readFile(file, 'utf8');
  assert.ok(text.includes(from), `fixture precondition: ${path.basename(file)} contains ${JSON.stringify(from)}`);
  await writeFile(file, text.replace(from, to));
}

test('identical specs pass', async () => {
  const { a, b } = await fixture();
  const r = await checkSpecParity(a, b);
  assert.deepEqual(r.differences, []);
  assert.equal(r.ok, true);
  assert.ok(r.summary.features > 0);
  assert.ok(r.summary.steps > 0);
});

test('a one-byte difference in a feature file fails and names the file', async () => {
  const { a, b } = await fixture();
  const file = path.join(b, 'spec', 'bdd', 'api', 'products.feature');
  const buf = await readFile(file);
  const i = buf.indexOf('Feature');
  assert.ok(i >= 0);
  buf[i] = 'f'.charCodeAt(0);
  await writeFile(file, buf);
  const r = await checkSpecParity(a, b);
  assert.equal(r.ok, false);
  assert.ok(
    r.differences.some((d) => d.includes('spec/bdd/api/products.feature') && d.includes('differ')),
    r.differences.join('\n'),
  );
  assert.equal(r.differences.length, 1, r.differences.join('\n'));
});

test('a changed step pattern fails and names the step id', async () => {
  const { a, b } = await fixture();
  await edit(path.join(b, 'spec', 'bdd', 'format.yml'), 'pattern: "^run migration$"', 'pattern: "^run migrations$"');
  const r = await checkSpecParity(a, b);
  assert.equal(r.ok, false);
  assert.ok(
    r.differences.some((d) => d.includes('format.yml') && d.includes('run_migration') && d.includes('pattern')),
    r.differences.join('\n'),
  );
});

test('differences only in descriptions and comments pass', async () => {
  const { a, b } = await fixture();
  await edit(
    path.join(b, 'spec', 'openapi', 'openapi.yaml'),
    'description: Host and port the Go service listens on.',
    'description: Host and port the Java service listens on.',
  );
  await edit(path.join(b, 'spec', 'openapi', 'openapi.yaml'), 'openapi:', '# a new comment\nopenapi:');
  const fm = path.join(b, 'spec', 'bdd', 'format.yml');
  await edit(fm, 'run by godog', 'run by Cucumber-JVM');
  await edit(fm, 'Applies every migration in backend/migrations', 'Applies every Flyway migration in backend/src/main/resources/db/migration');
  await edit(fm, 'description: HTTP method (uppercase).', 'description: The HTTP method, in upper case.');
  await edit(
    fm,
    'with the corresponding column value from the Examples table',
    'with the matching Examples column value',
  );
  const r = await checkSpecParity(a, b);
  assert.deepEqual(r.differences, []);
  assert.equal(r.ok, true);
});

test('an extra feature file on one side fails and names it', async () => {
  const { a, b } = await fixture();
  await writeFile(path.join(b, 'spec', 'bdd', 'e2e', 'extra.feature'), 'Feature: Extra\n');
  const r = await checkSpecParity(a, b);
  assert.equal(r.ok, false);
  assert.ok(
    r.differences.some((d) => d.includes('spec/bdd/e2e/extra.feature') && d.includes('only in B')),
    r.differences.join('\n'),
  );
});

test('a changed openapi schema `required` list fails', async () => {
  const { a, b } = await fixture();
  // LoginRequest: password stops being required.
  await edit(
    path.join(b, 'spec', 'openapi', 'openapi.yaml'),
    'required: [username, password]',
    'required: [username]',
  );
  const r = await checkSpecParity(a, b);
  assert.equal(r.ok, false);
  assert.ok(
    r.differences.some((d) => d.includes('openapi.yaml') && d.includes('required')),
    r.differences.join('\n'),
  );
});

