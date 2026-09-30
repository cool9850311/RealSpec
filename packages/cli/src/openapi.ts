/**
 * openapi.ts — the API contract a feature file's requests are checked against.
 *
 * {@link loadOpenApi} reads every openapi document under one folder into an
 * {@link OpenApiIndex}, which can then say whether a request written in a
 * feature file (a method plus a raw path) names a declared operation. Nothing
 * is cached or remembered: each load scans the folder again, so an edited,
 * added or removed file takes effect on the next run.
 */

import { readFileSync, readdirSync, statSync, type Dirent } from 'node:fs';
import * as path from 'node:path';
import { YAMLParseError, parse as parseYaml } from 'yaml';
import { RegistryError } from './registry.js';

// ── Operations ───────────────────────────────────────────────────────────────

/** The folder as it is written in messages: the convention relative to format.yml. */
const DIR_DISPLAY = '../openapi';

/** How many "closest" operations a violation message names at most. */
const MAX_CANDIDATES = 3;

/** A `{param}` of an openapi path template. */
const OPENAPI_PARAM = /\{[^{}]*\}/;

/** A variable token of a feature path: `{contextVariable}` or `<outline param>`. */
const FEATURE_VARIABLE = /\{[^{}/]+\}|<[^<>/]+>/;

/** One declared operation. */
export interface OpenApiOperation {
  /** Upper-case HTTP method. */
  readonly method: string;
  /** Server base path plus the path template, e.g. `/api/v1/orders/{id}`. */
  readonly path: string;
  /** The non-empty segments of {@link path}. */
  readonly segments: readonly string[];
  /** How many leading segments of {@link path} come from the server base. */
  readonly baseSegments: number;
}

/** Drops trailing slashes; the empty result stands for the root. */
function normalize(p: string): string {
  let end = p.length;
  while (end > 0 && p.charAt(end - 1) === '/') end--;
  return p.slice(0, end);
}

function segmentsOf(normalizedPath: string): string[] {
  return normalizedPath.split('/').filter((part) => part !== '');
}

/** Builds the operation for `method` on `template`, below the server `base`. */
export function openApiOperation(method: string, base: string, template: string): OpenApiOperation {
  const p = normalize(base + (template.startsWith('/') ? template : '/' + template));
  return {
    method: method.toUpperCase(),
    path: p,
    segments: segmentsOf(p),
    baseSegments: segmentsOf(normalize(base)).length,
  };
}

/** Code-point-order comparison (UTF-16 code units, like Java's `String.compareTo`). */
function compareText(a: string, b: string): number {
  return a < b ? -1 : a > b ? 1 : 0;
}

function label(op: OpenApiOperation): string {
  return `${op.method} ${op.path}`;
}

// ── Matching ─────────────────────────────────────────────────────────────────

function escapeRegExp(literal: string): string {
  return literal.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

/** `text` with every `token` match replaced by `replacement` and the rest matched literally. */
function regexOf(text: string, token: RegExp, replacement: string): string {
  let regex = '';
  let last = 0;
  for (const m of text.matchAll(new RegExp(token.source, 'g'))) {
    regex += escapeRegExp(text.slice(last, m.index)) + replacement;
    last = m.index + m[0].length;
  }
  return regex + escapeRegExp(text.slice(last));
}

function fullMatch(source: string, text: string): boolean {
  return new RegExp(`^(?:${source})$`).test(text);
}

/**
 * A literal openapi segment matches only an equal literal. A segment holding a
 * `{param}` matches any feature segment; a feature segment holding a variable
 * token is a wildcard that may only stand for such a parametrised segment.
 * Mixed segments such as `{name}.json` are compared as patterns: the openapi
 * side as a regex, a feature wildcard as a regex tested against the openapi
 * template text.
 */
function segmentMatches(requested: string, declared: string): boolean {
  const declaredParam = OPENAPI_PARAM.test(declared);
  const requestedVariable = FEATURE_VARIABLE.test(requested);
  if (!declaredParam) return !requestedVariable && requested === declared;
  if (requestedVariable) return fullMatch(regexOf(requested, FEATURE_VARIABLE, '.+'), declared);
  return fullMatch(regexOf(declared, OPENAPI_PARAM, '[^/]+'), requested);
}

function commonPrefix(requested: readonly string[], declared: readonly string[]): number {
  const limit = Math.min(requested.length, declared.length);
  let n = 0;
  while (n < limit && segmentMatches(requested[n]!, declared[n]!)) n++;
  return n;
}

function segmentsMatch(requested: readonly string[], declared: readonly string[]): boolean {
  return requested.length === declared.length && commonPrefix(requested, declared) === declared.length;
}

function stripQueryAndFragment(rawPath: string): string {
  let cut = rawPath.length;
  const query = rawPath.indexOf('?');
  if (query >= 0) cut = query;
  const fragment = rawPath.indexOf('#');
  if (fragment >= 0 && fragment < cut) cut = fragment;
  return rawPath.slice(0, cut);
}

// ── Index ────────────────────────────────────────────────────────────────────

/**
 * The union of every operation declared by the openapi files of one folder.
 * An index is immutable and is built afresh by {@link loadOpenApi} on every run.
 */
export class OpenApiIndex {
  private readonly operations: readonly OpenApiOperation[];

  /** Duplicates (same path and method) collapse; the rest is sorted by path, then method. */
  constructor(operations: Iterable<OpenApiOperation>) {
    const distinct = new Map<string, OpenApiOperation>();
    for (const op of operations) distinct.set(`${op.path}\0${op.method}`, op);
    this.operations = [...distinct.values()].sort(
      (a, b) => compareText(a.path, b.path) || compareText(a.method, b.method),
    );
  }

  /** The number of distinct method-plus-path operations. */
  operationCount(): number {
    return this.operations.length;
  }

  /** The folder as shown in messages, always `../openapi`. */
  dirDisplay(): string {
    return DIR_DISPLAY;
  }

  /**
   * Checks one request. `method` is the HTTP method as written in the step;
   * `rawPath` the path as written in the step, which may carry a query string,
   * a fragment, `{contextVariable}` tokens and `<outline param>` tokens.
   * Returns `null` when the operation is declared, otherwise the violation
   * message.
   */
  check(method: string, rawPath: string): string | null {
    const upper = method.toUpperCase();
    const p = stripQueryAndFragment(rawPath);
    const requested = segmentsOf(normalize(p));

    const methods = new Set<string>();
    for (const op of this.operations) {
      if (segmentsMatch(requested, op.segments)) methods.add(op.method);
    }
    if (methods.has(upper)) return null;
    if (methods.size > 0) {
      const defined = [...methods].sort(compareText).join(', ');
      return `openapi: ${upper} ${p} is not defined under ${DIR_DISPLAY} (this path defines: ${defined})`;
    }
    const closest = this.closest(upper, requested);
    return (
      `openapi: ${upper} ${p} is not defined in any file under ${DIR_DISPLAY}` +
      (closest.length > 0 ? ` (closest: ${closest.join(', ')})` : '')
    );
  }

  /**
   * Up to three distinct operations that share at least the first segment
   * after their server base with the request: same method first, then the
   * longest shared leading prefix, then by label.
   */
  private closest(method: string, requested: readonly string[]): string[] {
    const candidates: { sameMethod: boolean; prefix: number; label: string }[] = [];
    for (const op of this.operations) {
      const prefix = commonPrefix(requested, op.segments);
      if (prefix > op.baseSegments) {
        candidates.push({ sameMethod: op.method === method, prefix, label: label(op) });
      }
    }
    candidates.sort(
      (a, b) =>
        Number(b.sameMethod) - Number(a.sameMethod) ||
        b.prefix - a.prefix ||
        compareText(a.label, b.label),
    );
    return [...new Set(candidates.map((c) => c.label))].slice(0, MAX_CANDIDATES);
  }
}

// ── Loading ──────────────────────────────────────────────────────────────────

const EXTENSIONS: ReadonlySet<string> = new Set(['yaml', 'yml', 'json']);
const HTTP_METHODS: ReadonlySet<string> = new Set([
  'get', 'put', 'post', 'delete', 'options', 'head', 'patch', 'trace',
]);

/** `^scheme://authority` at the start of a server url; the scheme may be a `{variable}`. */
const SCHEME_AUTHORITY = /^(?:\{[^}]*\}|[A-Za-z][A-Za-z0-9+.-]*):\/\/[^/]*/;

/** Generous bounds: real openapi documents are large, but aliases must not explode. */
const MAX_ALIAS_COUNT = 1000;

function firstLine(message: string | undefined): string {
  if (message === undefined || message === '') return 'unknown error';
  const newline = message.indexOf('\n');
  return newline < 0 ? message : message.slice(0, newline);
}

function errorMessage(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

function isDirectory(p: string): boolean {
  try {
    return statSync(p).isDirectory();
  } catch {
    return false;
  }
}

function isRegularFile(entry: Dirent, full: string): boolean {
  if (entry.isFile()) return true;
  if (!entry.isSymbolicLink()) return false;
  try {
    return statSync(full).isFile();
  } catch {
    return false;
  }
}

function hasOpenApiExtension(name: string): boolean {
  const dot = name.lastIndexOf('.');
  return dot >= 0 && EXTENSIONS.has(name.slice(dot + 1).toLowerCase());
}

/** Every openapi file below `dir` as a `/`-separated path relative to `root`, sorted. */
function openApiFiles(root: string, dir = root, prefix = ''): string[] {
  const found: string[] = [];
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    const rel = prefix + entry.name;
    if (entry.isDirectory()) {
      found.push(...openApiFiles(root, full, rel + '/'));
    } else if (isRegularFile(entry, full) && hasOpenApiExtension(entry.name)) {
      found.push(rel);
    }
  }
  return dir === root ? found.sort(compareText) : found;
}

/**
 * Reads the folder recursively. Throws a {@link RegistryError} when the folder
 * is missing, holds no openapi file, or a file cannot be parsed or has no
 * `paths` mapping.
 */
export function loadOpenApi(openapiDir: string): OpenApiIndex {
  if (!isDirectory(openapiDir)) {
    throw new RegistryError(
      `openapi directory not found: ${DIR_DISPLAY} (expected next to the directory holding format.yml)`,
    );
  }
  let files: string[];
  try {
    files = openApiFiles(openapiDir);
  } catch (e) {
    throw new RegistryError(`cannot read ${DIR_DISPLAY}: ${firstLine(errorMessage(e))}`);
  }
  if (files.length === 0) {
    throw new RegistryError(`no openapi files found under ${DIR_DISPLAY}`);
  }
  const operations: OpenApiOperation[] = [];
  for (const rel of files) {
    operations.push(...operationsOf(path.join(openapiDir, ...rel.split('/')), rel));
  }
  return new OpenApiIndex(operations);
}

function operationsOf(file: string, name: string): OpenApiOperation[] {
  const document = parseDocument(file, name);
  const paths = document instanceof Map ? document.get('paths') : undefined;
  if (!(paths instanceof Map)) {
    throw new RegistryError(`${name}: not a valid openapi document (missing 'paths' mapping)`);
  }
  const base = basePath((document as Map<unknown, unknown>).get('servers'));
  const operations: OpenApiOperation[] = [];
  for (const [template, item] of paths) {
    if (typeof template !== 'string' || !template.startsWith('/') || !(item instanceof Map)) continue;
    for (const method of item.keys()) {
      if (typeof method === 'string' && HTTP_METHODS.has(method.toLowerCase())) {
        operations.push(openApiOperation(method, base, template));
      }
    }
  }
  return operations;
}

/**
 * Parses one file. Maps are kept as `Map`s so that only genuine string keys
 * count: a key such as `200` stays a number and is never taken for a path or
 * a method. Duplicate keys are a parse error.
 */
function parseDocument(file: string, name: string): unknown {
  let text: string;
  try {
    text = readFileSync(file, 'utf8');
  } catch (e) {
    throw new RegistryError(`${name}: cannot be read: ${firstLine(errorMessage(e))}`);
  }
  try {
    return parseYaml(text, { mapAsMap: true, maxAliasCount: MAX_ALIAS_COUNT });
  } catch (e) {
    if (e instanceof YAMLParseError) {
      // The library's message carries a code frame after the first line; keep
      // only the problem itself and report the position in our own words.
      const problem = firstLine(e.message).replace(/ at line \d+, column \d+:?$/, '');
      const pos = e.linePos?.[0];
      const where = pos === undefined ? '' : ` (line ${pos.line}, column ${pos.col})`;
      throw new RegistryError(`${name}: invalid YAML: ${problem}${where}`);
    }
    throw new RegistryError(`${name}: invalid YAML: ${firstLine(errorMessage(e))}`);
  }
}

/**
 * The path part of the first server url that has one, without trailing slash.
 * No servers means an empty base.
 */
function basePath(servers: unknown): string {
  if (!Array.isArray(servers)) return '';
  for (const server of servers) {
    if (server instanceof Map) {
      const url = server.get('url');
      if (typeof url === 'string') return pathOfUrl(url.trim());
    }
  }
  return '';
}

/**
 * A leading `scheme://authority` is dropped, as is a url that begins with a
 * `{var}` template up to its first slash; then the query, the fragment and the
 * trailing slashes go.
 */
function pathOfUrl(url: string): string {
  let p = url;
  const scheme = SCHEME_AUTHORITY.exec(url);
  if (scheme !== null) {
    p = url.slice(scheme[0].length);
  } else if (url.startsWith('{')) {
    const slash = url.indexOf('/');
    p = slash < 0 ? '' : url.slice(slash);
  }
  const cut = p.search(/[?#]/);
  if (cut >= 0) p = p.slice(0, cut);
  if (p !== '' && !p.startsWith('/')) p = '/' + p;
  return normalize(p);
}
