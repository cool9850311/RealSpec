/**
 * `loadOpenApi`: which files are read, how a broken one is reported, how the
 * server base is derived, and that nothing is cached between two loads.
 */

import { mkdirSync, mkdtempSync, rmSync, unlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import * as path from 'node:path';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';

import { loadOpenApi } from '../src/openapi.js';
import { RegistryError } from '../src/registry.js';

const ORDERS = `openapi: 3.1.0
servers:
  - url: "https://{host}/api/v1"
paths:
  /orders:
    get: {}
    post: {}
  /orders/{id}:
    get: {}
`;

const MISSING_PATHS = "not a valid openapi document (missing 'paths' mapping)";

let dir: string;

beforeEach(() => {
  dir = mkdtempSync(path.join(tmpdir(), 'openapi-loader-'));
});

afterEach(() => {
  rmSync(dir, { recursive: true, force: true });
});

function write(relative: string, content: string): void {
  const file = path.join(dir, relative);
  mkdirSync(path.dirname(file), { recursive: true });
  writeFileSync(file, content);
}

/** A minimal document: `serversBlock` (may be empty) and one `get` per path. */
function doc(serversBlock: string, ...paths: string[]): string {
  return `openapi: 3.1.0\n${serversBlock}paths:\n${paths.map((p) => `  ${p}: {get: {}}\n`).join('')}`;
}

function loadError(target: string = dir): RegistryError {
  try {
    loadOpenApi(target);
  } catch (e) {
    expect(e).toBeInstanceOf(RegistryError);
    return e as RegistryError;
  }
  throw new Error('expected loadOpenApi to throw');
}

describe('loadOpenApi', () => {
  it('1. a missing directory is a RegistryError', () => {
    expect(loadError(path.join(dir, 'nope')).message).toBe(
      'openapi directory not found: ../openapi (expected next to the directory holding format.yml)',
    );
  });

  it('2. a path that is a file is treated as a missing directory', () => {
    write('f.yaml', ORDERS);
    expect(loadError(path.join(dir, 'f.yaml')).message).toBe(
      'openapi directory not found: ../openapi (expected next to the directory holding format.yml)',
    );
  });

  it('3. an empty directory is a RegistryError', () => {
    expect(loadError().message).toBe('no openapi files found under ../openapi');
  });

  it('4. a directory with only foreign extensions has no openapi files', () => {
    write('README.md', '# docs');
    write('orders.yaml.bak', ORDERS);
    expect(loadError().message).toBe('no openapi files found under ../openapi');
  });

  it('5. bad YAML names the file, line and column, and carries no stack', () => {
    write('good.yaml', ORDERS);
    write('sub/broken.yaml', 'paths:\n  /a: [unclosed\n');
    const error = loadError();
    expect(error.message).toBe(
      'sub/broken.yaml: invalid YAML: Flow sequence in block collection must be sufficiently' +
        ' indented and end with a ] (line 3, column 1)',
    );
    expect(error.message).not.toContain('\n');
    expect(error.message).not.toContain('at ');
    expect(error.cause).toBeUndefined();
  });

  it('6. a missing paths key names the file', () => {
    write('a.yaml', ORDERS);
    write('b.yaml', 'openapi: 3.1.0\ninfo: {title: x}\n');
    expect(loadError().message).toBe(`b.yaml: ${MISSING_PATHS}`);
  });

  it('7. paths that is not a mapping is rejected', () => {
    write('a.yaml', 'paths: [1, 2]\n');
    expect(loadError().message).toBe(`a.yaml: ${MISSING_PATHS}`);
  });

  it('8. empty and scalar documents are rejected', () => {
    write('a.yaml', '');
    expect(loadError().message).toBe(`a.yaml: ${MISSING_PATHS}`);
    write('a.yaml', 'just text\n');
    expect(loadError().message).toBe(`a.yaml: ${MISSING_PATHS}`);
  });

  it('9. non-openapi extensions are ignored even when broken', () => {
    write('orders.yaml', ORDERS);
    write('notes.md', 'paths: [broken');
    write('orders.yaml.bak', '::: not yaml');
    write('orders.txt', 'paths: [broken');
    expect(loadOpenApi(dir).operationCount()).toBe(3);
  });

  it('10. extensions are case-insensitive and .YML and .json are read', () => {
    write('a.YAML', doc('', '/one'));
    write('b.YML', doc('', '/two'));
    write('c.json', '{"openapi": "3.0.0", "paths": {"/three": {"post": {}}}}');
    const index = loadOpenApi(dir);
    expect(index.operationCount()).toBe(3);
    expect(index.check('GET', '/one')).toBeNull();
    expect(index.check('GET', '/two')).toBeNull();
    expect(index.check('POST', '/three')).toBeNull();
  });

  it('11. subfolders are scanned recursively', () => {
    write('a/b/c/deep.yaml', doc('', '/deep'));
    write('top.yaml', doc('', '/top'));
    const index = loadOpenApi(dir);
    expect(index.check('GET', '/deep')).toBeNull();
    expect(index.check('GET', '/top')).toBeNull();
  });

  it('12. only HTTP method keys become operations', () => {
    write(
      'a.yaml',
      `paths:
  /x:
    summary: shared
    parameters: []
    x-ext: 1
    get: {}
    PATCH: {}
  x-vendor: {get: {}}
  /y: ~
`,
    );
    const index = loadOpenApi(dir);
    expect(index.operationCount()).toBe(2);
    expect(index.check('PATCH', '/x')).toBeNull();
    expect(index.check('GET', '/y')).toBe(
      'openapi: GET /y is not defined in any file under ../openapi',
    );
  });

  it('13. https://{host}/api/v1 gives the base /api/v1', () => {
    write('a.yaml', ORDERS);
    const index = loadOpenApi(dir);
    expect(index.check('GET', '/api/v1/orders')).toBeNull();
    expect(index.check('GET', '/orders')).not.toBeNull();
  });

  it('14. other server shapes are handled', () => {
    write('v2.yaml', doc("servers: [{url: 'http://localhost:8080/v2'}]\n", '/a'));
    write('tmpl.yaml', doc("servers: [{url: '{scheme}://x/api/'}]\n", '/b'));
    write('lead.yaml', doc("servers: [{url: '{base}/c-base'}]\n", '/c'));
    write('nopath.yaml', doc("servers: [{url: 'https://example.com'}]\n", '/d'));
    write('root.yaml', doc("servers: [{url: '/'}]\n", '/e'));
    write('none.yaml', doc('', '/f'));
    write('relative.yaml', doc("servers: [{url: 'rel/base'}]\n", '/g'));
    write('http.yaml', doc("servers: [{url: 'http://h'}]\n", '/h'));
    write('cut.yaml', doc("servers: [{url: 'api/v2?x#y'}]\n", '/i'));
    const index = loadOpenApi(dir);
    expect(index.check('GET', '/v2/a')).toBeNull();
    expect(index.check('GET', '/api/b')).toBeNull();
    expect(index.check('GET', '/c-base/c')).toBeNull();
    expect(index.check('GET', '/d')).toBeNull();
    expect(index.check('GET', '/e')).toBeNull();
    expect(index.check('GET', '/f')).toBeNull();
    expect(index.check('GET', '/rel/base/g')).toBeNull();
    expect(index.check('GET', '/h')).toBeNull();
    expect(index.check('GET', '/api/v2/i')).toBeNull();
    expect(index.check('GET', '/a')).not.toBeNull();
  });

  it('15. only the first server counts', () => {
    write('a.yaml', doc("servers: [{url: 'https://h/first'}, {url: 'https://h/second'}]\n", '/a'));
    const index = loadOpenApi(dir);
    expect(index.check('GET', '/first/a')).toBeNull();
    expect(index.check('GET', '/second/a')).not.toBeNull();
  });

  it('16. the trailing slash of the server url is stripped', () => {
    write('a.yaml', doc("servers: [{url: 'https://h/api/v1/'}]\n", '/a'));
    expect(loadOpenApi(dir).check('GET', '/api/v1/a')).toBeNull();
  });

  it('17. each file keeps its own base', () => {
    write('a.yaml', doc("servers: [{url: 'https://h/api/v1'}]\n", '/orders'));
    write('b.yaml', doc("servers: [{url: 'https://h/api/v2'}]\n", '/payments'));
    const index = loadOpenApi(dir);
    expect(index.check('GET', '/api/v1/orders')).toBeNull();
    expect(index.check('GET', '/api/v2/payments')).toBeNull();
    expect(index.check('GET', '/api/v2/orders')).not.toBeNull();
    expect(index.check('GET', '/api/v1/payments')).not.toBeNull();
  });

  it('18. the same operation in several files is counted once', () => {
    write('a.yaml', doc('', '/x'));
    write('b.yaml', doc('', '/x'));
    expect(loadOpenApi(dir).operationCount()).toBe(1);
  });

  it('19. load order does not influence the result', () => {
    write('z/one.yaml', doc('', '/shared/a', '/shared/b'));
    write('a/two.yaml', doc('', '/shared/c', '/shared/d'));
    write('m.yaml', doc('', '/shared/e'));
    const first = loadOpenApi(dir).check('GET', '/shared/zzz');
    const second = loadOpenApi(dir).check('GET', '/shared/zzz');
    const expected =
      'openapi: GET /shared/zzz is not defined in any file under ../openapi' +
      ' (closest: GET /shared/a, GET /shared/b, GET /shared/c)';
    expect(first).toBe(expected);
    expect(second).toBe(expected);
  });

  it('20. editing a file between two loads is seen by the second load', () => {
    write('a.yaml', doc('', '/first'));
    const before = loadOpenApi(dir);
    expect(before.check('GET', '/second')).not.toBeNull();

    write('a.yaml', doc('', '/first', '/second'));
    write('b.yaml', doc('', '/third'));
    const after = loadOpenApi(dir);
    expect(after.check('GET', '/second')).toBeNull();
    expect(after.check('GET', '/third')).toBeNull();
    expect(after.operationCount()).toBe(3);
    // an index already built is immutable
    expect(before.check('GET', '/second')).not.toBeNull();

    unlinkSync(path.join(dir, 'b.yaml'));
    expect(loadOpenApi(dir).check('GET', '/third')).not.toBeNull();
  });
});
