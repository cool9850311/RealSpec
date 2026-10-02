/**
 * The OpenAPI cross-check, end to end through the public CLI entry `run`.
 *
 * Every test builds a throw-away tree
 *
 *     <tmp>/spec/bdd/format.yml
 *     <tmp>/spec/bdd/api/x.feature
 *     <tmp>/spec/openapi/*.yaml
 *
 * so the format.yml -> ../openapi relationship is exercised exactly as a user
 * has it. Nothing is cached between runs, which cases 8-10 rely on.
 */

import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';

import { run } from '../src/cli.js';

const BAR = '─'.repeat(68);
const FEATURE = 'spec/bdd/api/x.feature';

/**
 * A registry modelled on the real examples. `http_request`,
 * `http_request_concurrent` and `network_request_responded` bind their method
 * and path captures to the contract; `legacy_request` has the same shape but no
 * binding, so it must never be checked.
 */
const REGISTRY = `
context_variables:
  provided:
    ctx: a value
    orderId: another value

steps:
  - id: http_request
    keywords: [When]
    pattern: "^(GET|POST|PUT|PATCH|DELETE) (/[a-zA-Z0-9/{}:._?=&%-]+):$"
    captures:
      - name: method
      - name: path
    openapi:
      method: method
      path: path
    docstring:
      type: json
    description: Sends an HTTP request.

  - id: http_request_concurrent
    keywords: [When]
    pattern: "^(GET|POST|PUT|PATCH|DELETE) (/[a-zA-Z0-9/{}:._?=&%-]+) is called concurrently:$"
    captures:
      - name: method
      - name: path
    openapi:
      method: method
      path: path
    docstring:
      type: json
    description: Sends the same request from several callers.

  - id: network_request_responded
    keywords: [Then, And]
    pattern: "^network request \\"(GET|POST|PUT|PATCH|DELETE) ([^\\"]+)\\" responded (\\\\d{3})$"
    captures:
      - name: method
      - name: path
      - name: status
    openapi:
      method: method
      path: path
    docstring: ~
    description: Asserts a request the page made.

  - id: legacy_request
    keywords: [When]
    pattern: "^(GET|POST|PUT|PATCH|DELETE) (/[a-zA-Z0-9/{}:._?=&%-]+) via the legacy client$"
    captures:
      - name: method
      - name: path
    docstring: ~
    description: Same shape as http_request, but not bound to the contract.

  - id: response_status
    keywords: [Then, And]
    pattern: "^response status is ([1-5][0-9]{2})$"
    captures:
      - name: statusCode
    docstring: ~
    description: Asserts the status code.
`;

/** A contract: `ops` maps a path template to its HTTP methods. */
function contract(server: string | null, ops: Record<string, string[]>): string {
  let out = 'openapi: 3.0.3\ninfo:\n  title: t\n  version: "1"\n';
  if (server !== null) out += `servers:\n  - url: ${server}\n`;
  out += 'paths:\n';
  for (const [p, methods] of Object.entries(ops)) {
    out += `  ${p}:\n`;
    for (const m of methods) out += `    ${m}:\n      responses:\n        "200":\n          description: ok\n`;
  }
  return out;
}

const V1 = 'https://{host}/api/v1';

interface Built {
  readonly text: string;
  /** 1-based line number of each step, in order. */
  readonly lines: number[];
}

/** One step per block; a block is the step line plus its docstring lines. */
function build(blocks: string[][], outline = false): Built {
  const out: string[] = [`Feature: openapi`, '', outline ? '  Scenario Outline: s' : '  Scenario: s'];
  const lines: number[] = [];
  for (const block of blocks) {
    lines.push(out.length + 1);
    for (const l of block) out.push(`    ${l}`);
  }
  if (outline) out.push('', '    Examples:', '      | id |', '      | 1  |');
  return { text: out.join('\n') + '\n', lines };
}

const req = (line: string): string[] => [`${line}:`, '"""json', '{}', '"""'];
const concurrent = (line: string): string[] => [
  `${line} is called concurrently:`,
  '"""json',
  '[{}, {}]',
  '"""',
];

const pass = (file = FEATURE): string =>
  `\n${BAR}\n  PASS  ${file}  (0 violation(s))\n${BAR}\n\n`;

/** The expected stdout for one failing file. */
function fail(file: string, violations: Array<{ line: number; message: string; step: string }>): string {
  let out = `\n${BAR}\n  FAIL  ${file}  (${violations.length} violation(s))\n${BAR}\n`;
  for (const v of violations) {
    out += `  line ${String(v.line).padStart(4)}  ${v.message}\n           → ${v.step}\n`;
  }
  return `${out}\n`;
}

/** Just the messages of every violation line in a report. */
function messages(stdout: string): string[] {
  return stdout
    .split('\n')
    .filter((l) => /^ {2}line +\d+ {2}/.test(l))
    .map((l) => l.replace(/^ {2}line +\d+ {2}/, ''));
}

let root: string;

beforeEach(() => {
  root = mkdtempSync(path.join(os.tmpdir(), 'realspec-openapi-'));
  write('spec/bdd/format.yml', REGISTRY);
});

afterEach(() => {
  rmSync(root, { recursive: true, force: true });
});

function write(rel: string, content: string): void {
  const abs = path.join(root, rel);
  mkdirSync(path.dirname(abs), { recursive: true });
  writeFileSync(abs, content);
}

/** Each case's registry has more steps than its one feature uses, hence the flag. */
function validate(rel: string = FEATURE): ReturnType<typeof run> {
  return run(['validate', '--allow-unused-steps', rel], { cwd: root });
}

const ORDERS = contract(V1, { '/orders': ['get', 'post'] });

describe('1. a known operation', () => {
  it('passes', () => {
    write('spec/openapi/openapi.yaml', ORDERS);
    write(
      FEATURE,
      build([req('When POST /api/v1/orders'), ['Then response status is 200']]).text,
    );
    expect(validate()).toEqual({ stdout: pass(), stderr: '', exitCode: 0 });
  });
});

describe('2. a path the contract does not define', () => {
  it('is reported on the step line', () => {
    write('spec/openapi/openapi.yaml', ORDERS);
    const b = build([req('When GET /api/v1/orders'), req('When GET /api/v1/nothing')]);
    write(FEATURE, b.text);
    expect(validate()).toEqual({
      stdout: fail(FEATURE, [
        {
          line: b.lines[1]!,
          message: 'openapi: GET /api/v1/nothing is not defined in any file under ../openapi',
          step: 'GET /api/v1/nothing:',
        },
      ]),
      stderr: '',
      exitCode: 1,
    });
  });
});

describe('3. a method the contract does not define for a known path', () => {
  it('lists the methods the path does define, sorted', () => {
    write('spec/openapi/openapi.yaml', contract(V1, { '/orders': ['put', 'get'] }));
    const b = build([req('When POST /api/v1/orders')]);
    write(FEATURE, b.text);
    expect(validate()).toEqual({
      stdout: fail(FEATURE, [
        {
          line: b.lines[0]!,
          message: 'openapi: POST /api/v1/orders is not defined under ../openapi (this path defines: GET, PUT)',
          step: 'POST /api/v1/orders:',
        },
      ]),
      stderr: '',
      exitCode: 1,
    });
  });
});

describe('4. the concurrent step and the network step', () => {
  it('are both checked, for the path and for the method', () => {
    write('spec/openapi/openapi.yaml', ORDERS);
    const b = build([
      concurrent('When GET /api/v1/ghosts'),
      concurrent('When DELETE /api/v1/orders'),
      ['Then network request "GET /api/v1/phantoms" responded 200'],
      ['And network request "PUT /api/v1/orders" responded 200'],
      concurrent('When POST /api/v1/orders'),
      ['And network request "GET /api/v1/orders" responded 200'],
    ]);
    write(FEATURE, b.text);
    expect(validate()).toEqual({
      stdout: fail(FEATURE, [
        {
          line: b.lines[0]!,
          message: 'openapi: GET /api/v1/ghosts is not defined in any file under ../openapi',
          step: 'GET /api/v1/ghosts is called concurrently:',
        },
        {
          line: b.lines[1]!,
          message: 'openapi: DELETE /api/v1/orders is not defined under ../openapi (this path defines: GET, POST)',
          step: 'DELETE /api/v1/orders is called concurrently:',
        },
        {
          line: b.lines[2]!,
          message: 'openapi: GET /api/v1/phantoms is not defined in any file under ../openapi',
          step: 'network request "GET /api/v1/phantoms" responded 200',
        },
        {
          line: b.lines[3]!,
          message: 'openapi: PUT /api/v1/orders is not defined under ../openapi (this path defines: GET, POST)',
          step: 'network request "PUT /api/v1/orders" responded 200',
        },
      ]),
      stderr: '',
      exitCode: 1,
    });
  });
});

describe('5. the network step drops the query string', () => {
  it('passes with a query and reports the path without it', () => {
    write('spec/openapi/openapi.yaml', ORDERS);
    const b = build([
      ['Then network request "GET /api/v1/orders?page=2&size=5" responded 200'],
      ['And network request "POST /api/v1/nothing?x=1#frag" responded 201'],
    ]);
    write(FEATURE, b.text);
    expect(validate()).toEqual({
      stdout: fail(FEATURE, [
        {
          line: b.lines[1]!,
          message: 'openapi: POST /api/v1/nothing is not defined in any file under ../openapi',
          step: 'network request "POST /api/v1/nothing?x=1#frag" responded 201',
        },
      ]),
      stderr: '',
      exitCode: 1,
    });
  });
});

describe('6. steps without an openapi binding', () => {
  it('are never checked', () => {
    write('spec/openapi/openapi.yaml', ORDERS);
    write(
      FEATURE,
      build([
        ['When GET /api/v1/never-defined via the legacy client'],
        ['When DELETE /elsewhere/entirely via the legacy client'],
        ['Then response status is 404'],
      ]).text,
    );
    expect(validate()).toEqual({ stdout: pass(), stderr: '', exitCode: 0 });
  });
});

describe('7. {ctx} and <param> path segments', () => {
  const CONTRACT = contract(V1, {
    '/users/{id}': ['get'],
    '/health/live': ['get'],
  });

  it('match a {param} segment', () => {
    write('spec/openapi/openapi.yaml', CONTRACT);
    write(FEATURE, build([req('When GET /api/v1/users/{ctx}'), req('When GET /api/v1/users/{orderId}')]).text);
    expect(validate()).toEqual({ stdout: pass(), stderr: '', exitCode: 0 });
  });

  it('match a {param} segment when written as an outline <param>', () => {
    write('spec/openapi/openapi.yaml', CONTRACT);
    write(FEATURE, build([req('When GET /api/v1/users/<id>')], true).text);
    expect(validate()).toEqual({ stdout: pass(), stderr: '', exitCode: 0 });
  });

  it('never match a literal segment', () => {
    write('spec/openapi/openapi.yaml', CONTRACT);
    const b = build([req('When GET /api/v1/health/{ctx}')]);
    write(FEATURE, b.text);
    const r = validate();
    expect(r.exitCode).toBe(1);
    expect(r.stderr).toBe('');
    const msgs = messages(r.stdout);
    expect(msgs).toHaveLength(1);
    expect(msgs[0]).toMatch(
      /^openapi: GET \/api\/v1\/health\/\{ctx\} is not defined in any file under \.\.\/openapi/,
    );
  });

  it('never match a literal segment when written as <param>, reporting <param> as written', () => {
    write('spec/openapi/openapi.yaml', CONTRACT);
    write(FEATURE, build([req('When GET /api/v1/health/<id>')], true).text);
    const r = validate();
    expect(r.exitCode).toBe(1);
    const msgs = messages(r.stdout);
    expect(msgs).toHaveLength(1);
    expect(msgs[0]).toMatch(
      /^openapi: GET \/api\/v1\/health\/<id> is not defined in any file under \.\.\/openapi/,
    );
    expect(r.stdout).toContain('→ GET /api/v1/health/<id>:');
  });
});

describe('8. editing the contract between two runs', () => {
  it('is picked up without rebuilding anything', () => {
    write('spec/openapi/openapi.yaml', ORDERS);
    const b = build([req('When GET /api/v1/orders')]);
    write(FEATURE, b.text);
    expect(validate()).toEqual({ stdout: pass(), stderr: '', exitCode: 0 });

    write('spec/openapi/openapi.yaml', contract(V1, { '/invoices': ['get'] }));
    expect(validate()).toEqual({
      stdout: fail(FEATURE, [
        {
          line: b.lines[0]!,
          message: 'openapi: GET /api/v1/orders is not defined in any file under ../openapi',
          step: 'GET /api/v1/orders:',
        },
      ]),
      stderr: '',
      exitCode: 1,
    });

    write('spec/openapi/openapi.yaml', ORDERS);
    expect(validate()).toEqual({ stdout: pass(), stderr: '', exitCode: 0 });
  });
});

describe('9. deleting the only openapi file', () => {
  it('stops the run with an ERROR and prints no report', () => {
    write('spec/openapi/openapi.yaml', ORDERS);
    write(FEATURE, build([req('When GET /api/v1/orders')]).text);
    expect(validate().exitCode).toBe(0);

    rmSync(path.join(root, 'spec/openapi/openapi.yaml'));
    expect(validate()).toEqual({
      stdout: '',
      stderr: 'ERROR: no openapi files found under ../openapi\n',
      exitCode: 1,
    });
  });
});

describe('10. adding a second openapi file', () => {
  it('makes a previously rejected request pass', () => {
    write('spec/openapi/orders.yaml', ORDERS);
    const b = build([req('When GET /api/v1/orders'), req('When GET /api/v1/invoices')]);
    write(FEATURE, b.text);
    expect(validate()).toEqual({
      stdout: fail(FEATURE, [
        {
          line: b.lines[1]!,
          message: 'openapi: GET /api/v1/invoices is not defined in any file under ../openapi',
          step: 'GET /api/v1/invoices:',
        },
      ]),
      stderr: '',
      exitCode: 1,
    });

    write('spec/openapi/invoices.yaml', contract(V1, { '/invoices': ['get'] }));
    expect(validate()).toEqual({ stdout: pass(), stderr: '', exitCode: 0 });
  });
});

describe('11. a file in a subfolder', () => {
  it('is picked up', () => {
    write('spec/openapi/v2/billing/invoices.yaml', contract(V1, { '/invoices': ['get'] }));
    write(FEATURE, build([req('When GET /api/v1/invoices')]).text);
    expect(validate()).toEqual({ stdout: pass(), stderr: '', exitCode: 0 });
  });
});

describe('12. files with different bases', () => {
  it('each accepts only paths under its own base', () => {
    write('spec/openapi/public.yaml', contract(V1, { '/orders': ['get'] }));
    write('spec/openapi/admin.yaml', contract('https://{host}/admin/api', { '/stats': ['get'] }));
    const b = build([
      req('When GET /api/v1/orders'),
      req('When GET /admin/api/stats'),
      req('When GET /api/v1/stats'),
      req('When GET /admin/api/orders'),
    ]);
    write(FEATURE, b.text);
    const r = validate();
    expect(r.exitCode).toBe(1);
    expect(r.stderr).toBe('');
    const msgs = messages(r.stdout);
    expect(msgs).toHaveLength(2);
    expect(msgs[0]).toMatch(/^openapi: GET \/api\/v1\/stats is not defined in any file under \.\.\/openapi/);
    expect(msgs[1]).toMatch(/^openapi: GET \/admin\/api\/orders is not defined in any file under \.\.\/openapi/);
    expect(r.stdout).toContain(`  line ${String(b.lines[2]).padStart(4)}  openapi: GET /api/v1/stats`);
    expect(r.stdout).toContain(`  line ${String(b.lines[3]).padStart(4)}  openapi: GET /admin/api/orders`);
    expect(r.stdout).toContain('(2 violation(s))');
  });
});

describe('13. --format with a feature file outside spec/', () => {
  it('uses the ../openapi next to that format.yml', () => {
    write('spec/openapi/openapi.yaml', ORDERS);
    // A decoy where a naive "next to the feature file" lookup would land.
    write('openapi/decoy.yaml', contract(V1, { '/decoy': ['get'] }));
    const b = build([req('When GET /api/v1/orders'), req('When GET /api/v1/decoy')]);
    write('features/x.feature', b.text);

    const r = run(
      ['validate', '--allow-unused-steps', 'features/x.feature', '--format', 'spec/bdd/format.yml'],
      { cwd: root },
    );
    expect(r).toEqual({
      stdout: fail('features/x.feature', [
        {
          line: b.lines[1]!,
          message: 'openapi: GET /api/v1/decoy is not defined in any file under ../openapi',
          step: 'GET /api/v1/decoy:',
        },
      ]),
      stderr: '',
      exitCode: 1,
    });
  });
});

describe('14. broken openapi YAML', () => {
  it('prints no half report, only an ERROR', () => {
    write('spec/openapi/openapi.yaml', 'openapi: 3.0.3\npaths: [unclosed\n');
    write(FEATURE, build([req('When GET /api/v1/orders')]).text);
    const r = validate();
    expect(r.stdout).toBe('');
    expect(r.stderr.startsWith('ERROR: openapi.yaml: invalid YAML:')).toBe(true);
    expect(r.stderr.endsWith('\n')).toBe(true);
    expect(r.exitCode).toBe(1);
  });
});
