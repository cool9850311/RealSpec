/**
 * Index-level behaviour of `OpenApiIndex.check`: which requests a set of
 * openapi operations accepts, and the exact wording of the violation otherwise.
 */

import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import * as path from 'node:path';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';

import { loadOpenApi, type OpenApiIndex } from '../src/openapi.js';

const SPEC = `openapi: 3.1.0
servers:
  - url: "https://{host}/api/v1"
paths:
  /auth/login:
    post: {}
  /me:
    get: {}
  /products:
    get: {}
  /orders:
    get: {}
    post: {}
  /orders/{id}:
    get: {}
    delete: {}
  /orders/{id}/items/{itemId}:
    get: {}
  /orders/export:
    get: {}
  /files/{name}.json:
    get: {}
  /admin/orders:
    get: {}
`;

let dir: string;

beforeEach(() => {
  dir = mkdtempSync(path.join(tmpdir(), 'openapi-index-'));
});

afterEach(() => {
  rmSync(dir, { recursive: true, force: true });
});

function index(content: string = SPEC): OpenApiIndex {
  writeFileSync(path.join(dir, 'openapi.yaml'), content);
  return loadOpenApi(dir);
}

describe('OpenApiIndex.check', () => {
  it('1. an existing method and path pass', () => {
    const idx = index();
    expect(idx.check('POST', '/api/v1/auth/login')).toBeNull();
    expect(idx.check('GET', '/api/v1/me')).toBeNull();
    expect(idx.check('DELETE', '/api/v1/orders/42')).toBeNull();
  });

  it('2. an unknown path without candidates has no closest list', () => {
    expect(index().check('GET', '/nothing/here')).toBe(
      'openapi: GET /nothing/here is not defined in any file under ../openapi',
    );
  });

  it('3. closest orders same method, then longest prefix, then label', () => {
    const idx = index();
    const expected =
      'openapi: GET /api/v1/orders/42/refund is not defined in any file under ../openapi' +
      ' (closest: GET /api/v1/orders/{id}, GET /api/v1/orders/{id}/items/{itemId},' +
      ' GET /api/v1/orders)';
    expect(idx.check('GET', '/api/v1/orders/42/refund')).toBe(expected);
    expect(idx.check('GET', '/api/v1/orders/42/refund')).toBe(expected);
    expect(idx.check('POST', '/api/v1/orders/42/refund')).toBe(
      'openapi: POST /api/v1/orders/42/refund is not defined in any file under ../openapi' +
        ' (closest: POST /api/v1/orders, DELETE /api/v1/orders/{id},' +
        ' GET /api/v1/orders/{id})',
    );
  });

  it('4. closest is capped at three', () => {
    expect(index().check('GET', '/api/v1/orders/zzz/zzz/zzz')).toBe(
      'openapi: GET /api/v1/orders/zzz/zzz/zzz is not defined in any file under ../openapi' +
        ' (closest: GET /api/v1/orders/{id}, GET /api/v1/orders/{id}/items/{itemId},' +
        ' GET /api/v1/orders)',
    );
  });

  it('5. candidates must share the first segment after the base', () => {
    // shares /api/v1 with everything, but no first segment after it
    expect(index().check('GET', '/api/v1/unknown')).toBe(
      'openapi: GET /api/v1/unknown is not defined in any file under ../openapi',
    );
  });

  it('6. a known path with the wrong method lists the sorted methods', () => {
    const idx = index();
    expect(idx.check('PUT', '/api/v1/orders')).toBe(
      'openapi: PUT /api/v1/orders is not defined under ../openapi (this path defines: GET, POST)',
    );
    expect(idx.check('POST', '/api/v1/orders/7')).toBe(
      'openapi: POST /api/v1/orders/7 is not defined under ../openapi' +
        ' (this path defines: DELETE, GET)',
    );
  });

  it('7. the method is case-insensitive', () => {
    expect(index().check('post', '/api/v1/orders')).toBeNull();
  });

  it('8. a {ctx} segment matches a path parameter', () => {
    const idx = index();
    expect(idx.check('GET', '/api/v1/orders/{orderId}')).toBeNull();
    expect(idx.check('GET', '/api/v1/orders/{orderId}/items/{itemId}')).toBeNull();
  });

  it('9. a <param> segment matches a path parameter', () => {
    const idx = index();
    expect(idx.check('GET', '/api/v1/orders/<id>')).toBeNull();
    expect(idx.check('DELETE', '/api/v1/orders/<id>')).toBeNull();
  });

  it('10. a {ctx} segment never matches a literal segment', () => {
    const idx = index();
    // /me and /products are literal openapi segments
    expect(idx.check('GET', '/api/v1/{resource}')).not.toBeNull();
    expect(idx.check('GET', '/api/v1/<resource>')).not.toBeNull();
    // the variable may not stand for the literal "items" either
    expect(idx.check('GET', '/api/v1/orders/42/{kind}/{itemId}')).not.toBeNull();
    // a partial variable still counts as a wildcard, so it cannot match a literal
    expect(idx.check('GET', '/api/v1/ord{suffix}')).not.toBeNull();
  });

  it('11. a wildcard does not hide a wrong method on a parametrised path', () => {
    expect(index().check('POST', '/api/v1/orders/{orderId}')).toBe(
      'openapi: POST /api/v1/orders/{orderId} is not defined under ../openapi' +
        ' (this path defines: DELETE, GET)',
    );
  });

  it('12. a literal mismatch is rejected', () => {
    const idx = index();
    expect(idx.check('GET', '/api/v1/order/42')).not.toBeNull();
    expect(idx.check('GET', '/api/v1/orders/42/item/1')).not.toBeNull();
    expect(idx.check('GET', '/api/v2/orders')).not.toBeNull();
    expect(idx.check('GET', '/orders')).not.toBeNull();
  });

  it('13. the segment count must agree', () => {
    const idx = index();
    expect(idx.check('GET', '/api/v1/orders/1/items')).not.toBeNull();
    expect(idx.check('GET', '/api/v1')).not.toBeNull();
  });

  it('14. the query string and fragment are stripped', () => {
    const idx = index();
    expect(idx.check('GET', '/api/v1/orders?status=paid&page={page}')).toBeNull();
    expect(idx.check('GET', '/api/v1/orders/{orderId}?expand=items')).toBeNull();
    expect(idx.check('GET', '/api/v1/orders#top')).toBeNull();
    expect(idx.check('GET', '/api/v1/orders/?a=b/c')).toBeNull();
  });

  it('15. the message shows the path without the query', () => {
    const idx = index();
    expect(idx.check('GET', '/api/v1/nowhere?x=1&y=2')).toBe(
      'openapi: GET /api/v1/nowhere is not defined in any file under ../openapi',
    );
    expect(idx.check('PUT', '/api/v1/orders?x=1')).toBe(
      'openapi: PUT /api/v1/orders is not defined under ../openapi (this path defines: GET, POST)',
    );
  });

  it('16. a trailing slash is ignored on both sides', () => {
    const idx = index();
    expect(idx.check('GET', '/api/v1/orders/')).toBeNull();
    expect(idx.check('GET', '/api/v1/orders///')).toBeNull();
    const slashed = index('paths:\n  /things/: {get: {}}\n');
    expect(slashed.check('GET', '/things')).toBeNull();
    expect(slashed.check('GET', '/things/')).toBeNull();
  });

  it('17. a mixed segment {name}.json matches foo.json but not foo.xml', () => {
    const idx = index();
    expect(idx.check('GET', '/api/v1/files/report.json')).toBeNull();
    expect(idx.check('GET', '/api/v1/files/{name}.json')).toBeNull();
    expect(idx.check('GET', '/api/v1/files/<name>.json')).toBeNull();
    expect(idx.check('GET', '/api/v1/files/{name}')).toBeNull();
    expect(idx.check('GET', '/api/v1/files/report.xml')).not.toBeNull();
    expect(idx.check('GET', '/api/v1/files/{name}.xml')).not.toBeNull();
    expect(idx.check('GET', '/api/v1/files/.json')).not.toBeNull();
  });

  it('18. no servers means an empty base', () => {
    const idx = index('paths:\n  /orders: {get: {}}\n');
    expect(idx.check('GET', '/orders')).toBeNull();
    expect(idx.check('GET', '/api/v1/orders')).not.toBeNull();
  });

  it('19. base segments alone do not make a candidate', () => {
    const idx = index("servers: [{url: 'https://h/api/v1'}]\npaths:\n  /orders: {get: {}}\n");
    expect(idx.check('GET', '/api/v1/customers')).toBe(
      'openapi: GET /api/v1/customers is not defined in any file under ../openapi',
    );
    expect(idx.check('GET', '/api/v1/orders/x')).toBe(
      'openapi: GET /api/v1/orders/x is not defined in any file under ../openapi' +
        ' (closest: GET /api/v1/orders)',
    );
  });

  it('20. the root path', () => {
    const idx = index('paths:\n  /: {get: {}}\n');
    expect(idx.check('GET', '/')).toBeNull();
    expect(idx.check('POST', '/')).toBe(
      'openapi: POST / is not defined under ../openapi (this path defines: GET)',
    );
  });
});
