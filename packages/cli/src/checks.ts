/**
 * checks.ts — docstring checks and the core validator loop.
 *
 * Nothing in here knows a step id, a path prefix or a special value. Every rule
 * the validator enforces is read from the {@link Registry}: the substitutions
 * that rewrite a docstring before it is checked, each step's `must_match` and
 * `allowed_top_level_keys`, its `step_ref`, its `openapi` binding and the
 * registry's context variables. Changing what is enforced is a change to
 * format.yml, never to this file.
 *
 * What stays here is message formatting. JSON docstrings are parsed by a
 * faithful re-implementation of CPython's JSON scanner (`Lib/json/decoder.py`
 * plus the `_json` C accelerator that `json.loads` actually uses), so a
 * malformed docstring is reported with CPython-shaped text
 * (`Expecting ',' delimiter: line 3 column 9 (char 25)`), and key sets are
 * rendered the way Python's `repr()` renders them. Both are wording, not rules.
 */

import { parseSteps, pyStrip, type Step } from './parse.js';
import type { OpenApiIndex } from './openapi.js';
import {
  applySubstitution,
  DEFAULT_MUST_MATCH_MESSAGE,
  matchesStepText,
  usesOpenApi,
  type DocstringSpec,
  type OpenApiBinding,
  type Registry,
  type StepDef,
  type StepRefSpec,
} from './registry.js';

/**
 * `<paramName>` in step text (Scenario Outline). Core grammar, not a rule: it is
 * how an outline step is made matchable before any pattern is tried.
 */
const OUTLINE_STEP_RE = /<[a-zA-Z][a-zA-Z0-9]*>/g;
/** What every outline parameter is replaced by before pattern matching. */
const OUTLINE_TOKEN = 'OUTLINE_PARAM';
/** `{varName}` anywhere in a step's text or docstring — the context-variable grammar. */
const CTX_NAME_RE = /\{([a-z][a-zA-Z0-9]+)\}/g;

/** A single rule violation, ready for {@link formatViolation}. */
export interface Violation {
  readonly file: string;
  readonly line: number;
  readonly step: string;
  readonly message: string;
}

// ─────────────────────────────────────────────────────────────────────────────
// CPython JSON emulation
// ─────────────────────────────────────────────────────────────────────────────

type PyJson =
  | { readonly t: 'null' }
  | { readonly t: 'bool'; readonly v: boolean }
  | { readonly t: 'num'; readonly v: number }
  | { readonly t: 'str'; readonly v: string }
  | { readonly t: 'arr'; readonly v: PyJson[] }
  | { readonly t: 'obj'; readonly v: Array<readonly [string, PyJson]> };

/** Mirrors CPython's `json.JSONDecodeError`, including its `str()` form. */
class PyJsonDecodeError extends Error {
  readonly msg: string;
  readonly pos: number;
  readonly lineno: number;
  readonly colno: number;

  constructor(msg: string, cps: readonly string[], pos: number) {
    let lineno = 1;
    let lastNewline = -1;
    for (let k = 0; k < pos && k < cps.length; k += 1) {
      if (cps[k] === '\n') {
        lineno += 1;
        lastNewline = k;
      }
    }
    const colno = pos - lastNewline;
    super(`${msg}: line ${lineno} column ${colno} (char ${pos})`);
    this.name = 'PyJsonDecodeError';
    this.msg = msg;
    this.pos = pos;
    this.lineno = lineno;
    this.colno = colno;
  }
}

/** Mirrors the `StopIteration(idx)` the CPython scanner raises for "no value". */
class ScannerStop {
  constructor(readonly value: number) {}
}

const WHITESPACE = ' \t\n\r';

function isWs(ch: string): boolean {
  return WHITESPACE.includes(ch);
}

function isDigit(ch: string | undefined): boolean {
  return ch !== undefined && ch >= '0' && ch <= '9';
}

const BACKSLASH: Readonly<Record<string, string>> = {
  '"': '"',
  '\\': '\\',
  '/': '/',
  b: '\b',
  f: '\f',
  n: '\n',
  r: '\r',
  t: '\t',
};

const HEX_RE = /^[0-9A-Fa-f]{4}$/;

/**
 * A faithful re-implementation of `json.loads` for `str` input.
 *
 * Operates on an array of code points so that `char` offsets, line and column
 * numbers match CPython (which indexes `str` by code point, not UTF-16 unit).
 */
class PyJsonScanner {
  private readonly cps: readonly string[];
  private readonly len: number;

  constructor(source: string) {
    this.cps = Array.from(source);
    this.len = this.cps.length;
  }

  /** CPython `s[i:i+1]` — an empty string when out of range. */
  private at(i: number): string {
    return this.cps[i] ?? '';
  }

  /** CPython `WHITESPACE.match(s, i).end()`, with `i` clamped to `len(s)`. */
  private wsEnd(i: number): number {
    let k = i < 0 ? 0 : i > this.len ? this.len : i;
    while (k < this.len && isWs(this.cps[k] ?? '')) k += 1;
    return k;
  }

  private startsWith(i: number, literal: string): boolean {
    for (let k = 0; k < literal.length; k += 1) {
      if (this.cps[i + k] !== literal[k]) return false;
    }
    return true;
  }

  private err(msg: string, pos: number): PyJsonDecodeError {
    return new PyJsonDecodeError(msg, this.cps, pos);
  }

  /** Entry point: `json.loads(s)`. */
  decode(): PyJson {
    if (this.cps[0] === '\uFEFF') {
      throw this.err('Unexpected UTF-8 BOM (decode using utf-8-sig)', 0);
    }
    let end = this.wsEnd(0);
    let value: PyJson;
    try {
      [value, end] = this.scanOnce(end);
    } catch (e) {
      if (e instanceof ScannerStop) throw this.err('Expecting value', e.value);
      throw e;
    }
    end = this.wsEnd(end);
    if (end !== this.len) throw this.err('Extra data', end);
    return value;
  }

  private scanOnce(idx: number): [PyJson, number] {
    if (idx >= this.len || idx < 0) throw new ScannerStop(idx);
    const nextchar = this.cps[idx] ?? '';

    if (nextchar === '"') return this.scanString(idx + 1);
    if (nextchar === '{') return this.scanObject(idx + 1);
    if (nextchar === '[') return this.scanArray(idx + 1);
    if (nextchar === 'n' && this.startsWith(idx, 'null')) return [{ t: 'null' }, idx + 4];
    if (nextchar === 't' && this.startsWith(idx, 'true')) return [{ t: 'bool', v: true }, idx + 4];
    if (nextchar === 'f' && this.startsWith(idx, 'false')) return [{ t: 'bool', v: false }, idx + 5];

    const numEnd = this.matchNumber(idx);
    if (numEnd !== null) {
      const text = this.cps.slice(idx, numEnd).join('');
      return [{ t: 'num', v: Number(text) }, numEnd];
    }
    if (nextchar === 'N' && this.startsWith(idx, 'NaN')) return [{ t: 'num', v: NaN }, idx + 3];
    if (nextchar === 'I' && this.startsWith(idx, 'Infinity')) {
      return [{ t: 'num', v: Infinity }, idx + 8];
    }
    if (nextchar === '-' && this.startsWith(idx, '-Infinity')) {
      return [{ t: 'num', v: -Infinity }, idx + 9];
    }
    throw new ScannerStop(idx);
  }

  /** CPython `NUMBER_RE.match(s, idx)`; returns the match end, or `null`. */
  private matchNumber(idx: number): number | null {
    let i = idx;
    if (this.cps[i] === '-') i += 1;
    if (this.cps[i] === '0') {
      i += 1;
    } else if (isDigit(this.cps[i])) {
      i += 1;
      while (isDigit(this.cps[i])) i += 1;
    } else {
      return null;
    }
    if (this.cps[i] === '.' && isDigit(this.cps[i + 1])) {
      i += 2;
      while (isDigit(this.cps[i])) i += 1;
    }
    if (this.cps[i] === 'e' || this.cps[i] === 'E') {
      let j = i + 1;
      if (this.cps[j] === '+' || this.cps[j] === '-') j += 1;
      if (isDigit(this.cps[j])) {
        j += 1;
        while (isDigit(this.cps[j])) j += 1;
        i = j;
      }
    }
    return i;
  }

  /** CPython `scanstring(s, end)`; `end` is the index after the opening quote. */
  private scanString(start: number): [PyJson, number] {
    const begin = start - 1;
    const chunks: string[] = [];
    let end = start;

    for (;;) {
      // STRINGCHUNK: (.*?)(["\\\x00-\x1f]) — find the next terminator.
      let k = end;
      while (k < this.len) {
        const c = this.cps[k] ?? '';
        if (c === '"' || c === '\\' || c <= '\x1f') break;
        k += 1;
      }
      if (k >= this.len) throw this.err('Unterminated string starting at', begin);

      const terminator = this.cps[k] ?? '';
      chunks.push(this.cps.slice(end, k).join(''));
      end = k + 1;

      if (terminator === '"') break;
      if (terminator !== '\\') {
        throw this.err('Invalid control character at', k);
      }
      if (end >= this.len) throw this.err('Unterminated string starting at', begin);

      const esc = this.cps[end] ?? '';
      if (esc !== 'u') {
        const decoded = BACKSLASH[esc];
        if (decoded === undefined) throw this.err('Invalid \\escape', k);
        chunks.push(decoded);
        end += 1;
      } else {
        let uni = this.decodeUxxxx(end);
        end += 5;
        if (uni >= 0xd800 && uni <= 0xdbff && this.cps[end] === '\\' && this.cps[end + 1] === 'u') {
          const uni2 = this.decodeUxxxx(end + 1);
          if (uni2 >= 0xdc00 && uni2 <= 0xdfff) {
            uni = 0x10000 + (((uni - 0xd800) << 10) | (uni2 - 0xdc00));
            end += 6;
          }
        }
        chunks.push(String.fromCodePoint(uni));
      }
    }

    return [{ t: 'str', v: chunks.join('') }, end];
  }

  /** CPython `_decode_uXXXX(s, pos)`; `pos` is the index of the `u`. */
  private decodeUxxxx(pos: number): number {
    const hex = this.cps.slice(pos + 1, pos + 5).join('');
    if (hex.length === 4 && HEX_RE.test(hex)) return parseInt(hex, 16);
    throw this.err('Invalid \\uXXXX escape', pos);
  }

  /** CPython `JSONObject((s, end), ...)`; `end` is the index after `{`. */
  private scanObject(start: number): [PyJson, number] {
    const pairs: Array<readonly [string, PyJson]> = [];
    let end = start;
    let nextchar = this.at(end);

    if (nextchar !== '"') {
      // Note: in CPython `'' in ' \t\n\r'` is True, so an exhausted input also
      // takes this branch.
      if (nextchar === '' || isWs(nextchar)) {
        end = this.wsEnd(end);
        nextchar = this.at(end);
      }
      if (nextchar === '}') return [{ t: 'obj', v: pairs }, end + 1];
      if (nextchar !== '"') {
        throw this.err('Expecting property name enclosed in double quotes', end);
      }
    }
    end += 1;

    for (;;) {
      const [key, afterKey] = this.scanString(end);
      end = afterKey;

      if (this.at(end) !== ':') {
        end = this.wsEnd(end);
        if (this.at(end) !== ':') throw this.err("Expecting ':' delimiter", end);
      }
      end += 1;

      // CPython's fast path, guarded by `try: ... except IndexError: pass`.
      if (end < this.len && isWs(this.cps[end] ?? '')) {
        end += 1;
        if (end < this.len && isWs(this.cps[end] ?? '')) end = this.wsEnd(end + 1);
      }

      let value: PyJson;
      try {
        [value, end] = this.scanOnce(end);
      } catch (e) {
        if (e instanceof ScannerStop) throw this.err('Expecting value', e.value);
        throw e;
      }
      pairs.push([key.t === 'str' ? key.v : '', value]);

      // try: nextchar = s[end]; if ws: end = _w(s, end+1).end(); nextchar = s[end]
      // except IndexError: nextchar = ''
      if (end >= this.len) {
        nextchar = '';
      } else {
        nextchar = this.cps[end] ?? '';
        if (isWs(nextchar)) {
          end = this.wsEnd(end + 1);
          nextchar = end >= this.len ? '' : this.cps[end] ?? '';
        }
      }
      end += 1;

      if (nextchar === '}') break;
      if (nextchar !== ',') throw this.err("Expecting ',' delimiter", end - 1);
      const commaIdx = end - 1;
      end = this.wsEnd(end);
      nextchar = this.at(end);
      end += 1;
      if (nextchar !== '"') {
        if (nextchar === '}') {
          throw this.err('Illegal trailing comma before end of object', commaIdx);
        }
        throw this.err('Expecting property name enclosed in double quotes', end - 1);
      }
    }

    return [{ t: 'obj', v: pairs }, end];
  }

  /** CPython `JSONArray((s, end), ...)`; `end` is the index after `[`. */
  private scanArray(start: number): [PyJson, number] {
    const values: PyJson[] = [];
    let end = start;
    let nextchar = this.at(end);

    if (nextchar === '' || isWs(nextchar)) {
      end = this.wsEnd(end + 1);
      nextchar = this.at(end);
    }
    if (nextchar === ']') return [{ t: 'arr', v: values }, end + 1];

    for (;;) {
      let value: PyJson;
      try {
        [value, end] = this.scanOnce(end);
      } catch (e) {
        if (e instanceof ScannerStop) throw this.err('Expecting value', e.value);
        throw e;
      }
      values.push(value);

      nextchar = this.at(end);
      if (nextchar === '' || isWs(nextchar)) {
        end = this.wsEnd(end + 1);
        nextchar = this.at(end);
      }
      end += 1;

      if (nextchar === ']') break;
      if (nextchar !== ',') throw this.err("Expecting ',' delimiter", end - 1);
      const commaIdx = end - 1;

      // try: if s[end] in ws: end += 1; if s[end] in ws: end = _w(s, end+1).end()
      //      nextchar = s[end:end+1]
      // except IndexError: pass  (nextchar keeps its previous value)
      let indexError = false;
      if (end >= this.len) {
        indexError = true;
      } else if (isWs(this.cps[end] ?? '')) {
        end += 1;
        if (end >= this.len) {
          indexError = true;
        } else if (isWs(this.cps[end] ?? '')) {
          end = this.wsEnd(end + 1);
        }
      }
      if (!indexError) nextchar = this.at(end);

      if (nextchar === ']') {
        throw this.err('Illegal trailing comma before end of array', commaIdx);
      }
    }

    return [{ t: 'arr', v: values }, end];
  }
}

/**
 * Parse `source` the way CPython's `json.loads` does.
 *
 * @returns the parsed value, or the exact `str(JSONDecodeError)` text on
 *          failure — discriminated by the `ok` flag.
 */
function pyJsonLoads(source: string): { ok: true; value: PyJson } | { ok: false; error: string } {
  try {
    return { ok: true, value: new PyJsonScanner(source).decode() };
  } catch (e) {
    if (e instanceof PyJsonDecodeError) return { ok: false, error: e.message };
    throw e;
  }
}

// ─────────────────────────────────────────────────────────────────────────────
// CPython repr emulation
// ─────────────────────────────────────────────────────────────────────────────

/**
 * Characters for which CPython's `str.isprintable()` is false: every Unicode
 * "Other" or "Separator" category, with the ASCII space as the sole exception.
 */
const NON_PRINTABLE_RE = /[\p{Cc}\p{Cf}\p{Cs}\p{Co}\p{Cn}\p{Zl}\p{Zp}\p{Zs}]/u;

/** CPython `repr()` of a `str`. */
export function pyReprStr(s: string): string {
  const quote = s.includes("'") && !s.includes('"') ? '"' : "'";
  let out = quote;
  for (const ch of s) {
    if (ch === '\\') out += '\\\\';
    else if (ch === quote) out += '\\' + quote;
    else if (ch === '\n') out += '\\n';
    else if (ch === '\r') out += '\\r';
    else if (ch === '\t') out += '\\t';
    else if (ch !== ' ' && NON_PRINTABLE_RE.test(ch)) {
      const code = ch.codePointAt(0) ?? 0;
      if (code < 0x100) out += '\\x' + code.toString(16).padStart(2, '0');
      else if (code < 0x10000) out += '\\u' + code.toString(16).padStart(4, '0');
      else out += '\\U' + code.toString(16).padStart(8, '0');
    } else {
      out += ch;
    }
  }
  return out + quote;
}

/** CPython `repr()` of a `list[str]`. */
export function pyReprStrList(items: readonly string[]): string {
  return '[' + items.map(pyReprStr).join(', ') + ']';
}

/**
 * CPython `repr()` of a non-empty `set[str]`.
 *
 * CPython's set iteration order is hash-based and, for `str`, randomised per
 * process; insertion order is the only reproducible choice here. Every message
 * this is used for in practice carries a single element, where the two agree.
 */
export function pyReprStrSet(items: readonly string[]): string {
  if (items.length === 0) return 'set()';
  return '{' + items.map(pyReprStr).join(', ') + '}';
}

/** CPython `sorted()` on strings: ascending code point order. */
export function pySorted(items: readonly string[]): string[] {
  return [...items].sort((a, b) => {
    const ca = Array.from(a);
    const cb = Array.from(b);
    const n = Math.min(ca.length, cb.length);
    for (let i = 0; i < n; i += 1) {
      const pa = (ca[i] ?? '').codePointAt(0) ?? 0;
      const pb = (cb[i] ?? '').codePointAt(0) ?? 0;
      if (pa !== pb) return pa - pb;
    }
    return ca.length - cb.length;
  });
}

// ─────────────────────────────────────────────────────────────────────────────
// Content validators
// ─────────────────────────────────────────────────────────────────────────────

/** Apply, in listed order, every substitution the registry declares for `type`. */
export function substituteDocstring(content: string, type: string, registry: Registry): string {
  let out = content;
  for (const substitution of registry.substitutions) {
    if (substitution.in === type) out = applySubstitution(substitution, out);
  }
  return out;
}

/**
 * Return an error string if `content` is not valid JSON.
 *
 * `content` is what the parser will see, so a caller that owns substitutions
 * applies them first (see {@link checkDocstring}).
 */
export function checkJson(content: string): string | null {
  const result = pyJsonLoads(content);
  if (!result.ok) return `docstring is not valid JSON after substitution: ${result.error}`;
  return null;
}

/** Return an error string if the SQL content is empty. */
export function checkSql(content: string): string | null {
  return pyStrip(content) === '' ? 'SQL docstring must not be empty' : null;
}

/** The violation for a `must_match` that found nothing in `content`, or `null`. */
function checkMustMatch(spec: DocstringSpec, content: string): string | null {
  if (spec.mustMatch === null) return null;
  // Compiled without g or y, so `.test` is stateless: this is "find", not "match".
  if (spec.mustMatch.test(content)) return null;
  return spec.mustMatchMessage ?? DEFAULT_MUST_MATCH_MESSAGE;
}

/**
 * Check the docstring `content` of a step matched to `def`, using only what
 * the step's docstring spec and the registry declare.
 *
 * JSON: parse the substituted content, then `must_match`, then
 * `allowed_top_level_keys`, then `step_ref`. SQL: the raw content must not be
 * empty, then the substituted content must satisfy `must_match`.
 *
 * @returns the violation message, or `null` when the content is acceptable
 */
export function checkDocstring(def: StepDef, content: string, registry: Registry): string | null {
  const spec = def.docstring;
  if (spec === null || spec.type === undefined) return null;
  const substituted = substituteDocstring(content, spec.type, registry);

  if (spec.type === 'json') {
    const parsed = pyJsonLoads(substituted);
    if (!parsed.ok) return `docstring is not valid JSON after substitution: ${parsed.error}`;

    const mismatch = checkMustMatch(spec, substituted);
    if (mismatch !== null) return mismatch;

    if (spec.allowedTopLevelKeys !== null) {
      const root = parsed.value;
      if (root.t !== 'obj') return `${def.id} docstring must be a JSON object`;
      const allowed = spec.allowedTopLevelKeys;
      const seen = new Set<string>();
      const extra: string[] = [];
      for (const [key] of root.v) {
        if (seen.has(key)) continue;
        seen.add(key);
        if (!allowed.includes(key)) extra.push(key);
      }
      if (extra.length > 0) {
        return `${def.id} docstring has unexpected top-level keys: ${pyReprStrSet(extra)}`;
      }
    }

    return def.stepRef === null ? null : checkStepRefs(substituted, def.stepRef, registry.steps);
  }

  const empty = checkSql(content);
  if (empty !== null) return empty;
  return checkMustMatch(spec, substituted);
}

/**
 * `step_ref`: a JSON docstring may NAME other steps of the same registry, so a
 * step that arranges several things at once composes the registry's existing
 * vocabulary rather than a new step being added for every actor that might take
 * part.
 *
 * A name is an ELEMENT of the docstring's top-level array whose only key is
 * `spec.key`. Nothing nested inside an element is looked at, so a request body
 * that happens to carry a field of that name is still a body; and a step that
 * declares no `step_ref` is never scanned at all. Both limits are deliberate:
 * this check must be unable to invent a violation out of ordinary data.
 *
 * The named step has to resolve, which is what keeps a registry that uses this
 * extension closed: the set of things nameable here is exactly the set of steps
 * it declares.
 *
 * `content` is the docstring AFTER the registry's substitutions, the text that
 * was parsed, so a placeholder a substitution rewrote cannot hide a name.
 */
export function checkStepRefs(
  content: string,
  spec: StepRefSpec,
  steps: readonly StepDef[],
): string | null {
  const result = pyJsonLoads(content);
  if (!result.ok) return null; // the parse failure is reported by the caller

  const root = result.value;
  if (root.t !== 'arr') return null; // nothing to name

  const names: string[] = [];
  for (const element of root.v) {
    if (element.t !== 'obj') continue;
    if (element.v.length !== 1 || element.v[0]![0] !== spec.key) continue;
    const value = element.v[0]![1];
    if (value.t === 'str') names.push(value.v);
  }

  for (const name of names) {
    const named = steps.find((def) => matchesStepText(def, name));
    if (named === undefined) {
      return `'${spec.key}' names ${JSON.stringify(name)}, which is not a step in this registry`;
    }
    const missing = spec.keywords.filter((k) => !named.keywords.includes(k));
    if (missing.length > 0) {
      return (
        `'${spec.key}' names step '${named.id}', which is not declared with ` +
        `${pyReprStrList(missing)} — only such steps may be named here`
      );
    }
    if (spec.docstring === 'none' && named.docstring !== null) {
      return (
        `'${spec.key}' names step '${named.id}', which takes a docstring of its own — ` +
        `a step named here carries no body, so it cannot be one that needs one`
      );
    }
  }
  return null;
}

// ─────────────────────────────────────────────────────────────────────────────
// Core validator
// ─────────────────────────────────────────────────────────────────────────────

/**
 * Step text with Scenario Outline parameters (`<name>`) replaced by
 * {@link OUTLINE_TOKEN}, so they do not block pattern matching, remembering
 * what each token stood for so a captured group can be turned back into the
 * text the author wrote.
 */
class StepText {
  /** The text to match step patterns against. */
  readonly normalised: string;
  private readonly spans: ReadonlyArray<{ start: number; end: number; original: string }>;

  constructor(text: string) {
    const spans: Array<{ start: number; end: number; original: string }> = [];
    let out = '';
    let last = 0;
    for (const m of text.matchAll(OUTLINE_STEP_RE)) {
      out += text.slice(last, m.index);
      const start = out.length;
      out += OUTLINE_TOKEN;
      spans.push({ start, end: out.length, original: m[0] });
      last = m.index + m[0].length;
    }
    out += text.slice(last);
    this.normalised = out;
    this.spans = spans;
  }

  /**
   * The substring `[start, end)` of {@link normalised} with every outline token
   * it contains restored to the parameter it replaced. A span the range only
   * partly covers is restored whole, exactly as the author wrote it.
   */
  original(start: number, end: number): string {
    let out = '';
    let pos = start;
    for (const span of this.spans) {
      if (span.end <= start) continue;
      if (span.start >= end) break;
      if (span.start > pos) out += this.normalised.slice(pos, span.start);
      out += span.original;
      pos = span.end;
    }
    if (pos < end) out += this.normalised.slice(pos, end);
    return out;
  }
}

/**
 * Step 6: the request a step makes must exist in the API contract. Returns the
 * index's message, or `null` when the request is defined (or cannot be read).
 */
function openApiMessage(
  def: StepDef,
  binding: OpenApiBinding,
  text: StepText,
  openapi: OpenApiIndex,
): string | null {
  // The step's own regex is sticky and reports no offsets; the same source with
  // `d` says where each group sits, which is what restoring `<param>` needs.
  const m = new RegExp(def.regex.source, 'yd').exec(text.normalised);
  if (m === null) return null;
  const capture = (group: number): string => {
    const span = m.indices?.[group];
    return span === undefined ? '' : text.original(span[0], span[1]);
  };
  return openapi.check(capture(binding.methodGroup).toUpperCase(), capture(binding.pathGroup));
}

/**
 * Validate one `.feature` source against a loaded registry.
 *
 * @param openapi the API contract index; may be `null` only for a registry
 *                that binds no step to it (see {@link usesOpenApi})
 * @throws Error  when the registry has openapi-bound steps and no index was given
 */
export function validate(
  filePath: string,
  source: string,
  registry: Registry,
  openapi: OpenApiIndex | null = null,
): Violation[] {
  if (openapi === null && usesOpenApi(registry)) {
    throw new Error(
      `the registry has openapi-bound steps, so an OpenApiIndex is required to validate ${filePath}`,
    );
  }

  const compiledSteps = registry.steps;
  const violations: Violation[] = [];
  const steps = parseSteps(source);
  /** The StepDef each step matched, aligned with `steps`; `null` when none. */
  const matched: Array<StepDef | null> = steps.map(() => null);

  for (const [i, step] of steps.entries()) {
    const at = (message: string): Violation => ({
      file: filePath,
      line: step.line,
      step: step.text,
      message,
    });
    // Normalise Scenario Outline tokens so they don't block pattern matching
    const text = new StepText(step.text);

    // ── 1. Find steps whose pattern matches ──────────────────────────────
    const candidates = compiledSteps.filter((cs) => matchesStepText(cs, text.normalised));

    if (candidates.length === 0) {
      violations.push(at('no matching step definition found in format.yml'));
      continue;
    }

    // ── 2. Among candidates pick one that allows this keyword ────────────
    const match = candidates.find((cs) => cs.keywords.includes(step.keyword));

    if (match === undefined) {
      const allowed = pySorted([...new Set(candidates.flatMap((cs) => [...cs.keywords]))]);
      violations.push(
        at(`keyword '${step.keyword}' is not allowed here (allowed: ${pyReprStrList(allowed)})`),
      );
      continue;
    }

    matched[i] = match;

    // ── 3. Docstring presence ────────────────────────────────────────────
    const expectsDs = match.docstring; // null → no docstring allowed

    if (expectsDs === null && step.dsContent !== null) {
      violations.push(at(`step '${match.id}' must not have a docstring`));
      continue;
    }

    if (expectsDs !== null && step.dsContent === null) {
      violations.push(
        at(`step '${match.id}' requires a ${(expectsDs.type ?? '').toUpperCase()} docstring`),
      );
      continue;
    }

    if (step.dsContent !== null && expectsDs !== null) {
      const expectedType = expectsDs.type;

      if (expectedType !== undefined && step.dsMarker !== expectedType) {
        // ── 4. Docstring marker ──────────────────────────────────────────
        // A wrong marker skips the content check: the wrong parser would only
        // give misleading errors.
        violations.push(at(`docstring marker is '${step.dsMarker}' but must be '${expectedType}'`));
      } else {
        // ── 5. Docstring content ─────────────────────────────────────────
        const err = checkDocstring(match, step.dsContent, registry);
        if (err !== null) violations.push(at(err));
      }
    }

    // ── 6. The request must exist in the API contract ────────────────────
    // Reached whatever became of the docstring: a bad body does not excuse a
    // request to an endpoint that does not exist.
    if (match.openapi !== null && openapi !== null) {
      const err = openApiMessage(match, match.openapi, text, openapi);
      if (err !== null) violations.push(at(err));
    }
  }

  const ctx = checkContextVariables(filePath, steps, matched, compiledSteps, registry.providedVars);
  return ctx.length === 0 ? violations : mergeByLine(violations, ctx);
}

// ─────────────────────────────────────────────────────────────────────────────
// Context variables
// ─────────────────────────────────────────────────────────────────────────────

/** Distinct `{name}` tokens in a step's text and docstring, in order. */
function referencedNames(step: Step): string[] {
  const names = new Set<string>();
  for (const text of [step.text, step.dsContent ?? '']) {
    CTX_NAME_RE.lastIndex = 0;
    for (let m = CTX_NAME_RE.exec(text); m !== null; m = CTX_NAME_RE.exec(text)) {
      if (m[1] !== undefined) names.add(m[1]);
    }
  }
  return [...names];
}

/** The variable name a step saves, read out of its `produces` capture group. */
function producedName(def: StepDef | null, step: Step): string | null {
  if (def === null || def.producesGroup === null) return null;
  def.regex.lastIndex = 0;
  const m = def.regex.exec(step.text.replace(OUTLINE_STEP_RE, OUTLINE_TOKEN));
  return m?.[def.producesGroup] ?? null;
}

/**
 * Every `{name}` a step uses must already be in scope: declared under
 * `context_variables.provided`, or saved by an earlier step of the same
 * scenario (Background steps count as earlier).
 *
 * Inert unless the registry declares `context_variables` or at least one
 * `produces`, so a registry that uses neither never sees it.
 */
function checkContextVariables(
  filePath: string,
  steps: readonly Step[],
  matched: readonly (StepDef | null)[],
  compiledSteps: readonly StepDef[],
  providedVars: ReadonlySet<string> | null,
): Violation[] {
  if (providedVars === null && !compiledSteps.some((d) => d.producesGroup !== null)) return [];
  const provided = providedVars ?? new Set<string>();

  /** Each step paired with the name it saves, if any. */
  const walk = steps.map((step, i) => ({ step, saves: producedName(matched[i] ?? null, step) }));
  type Entry = (typeof walk)[number];

  const background: Entry[] = [];
  const scenarios = new Map<number, Entry[]>();
  for (const entry of walk) {
    if (entry.step.background) {
      background.push(entry);
      continue;
    }
    const bucket = scenarios.get(entry.step.scenario);
    if (bucket === undefined) scenarios.set(entry.step.scenario, [entry]);
    else bucket.push(entry);
  }

  // One violation per (step, name): a Background step is walked once per
  // scenario, and a reader needs to be told about it once.
  const seen = new Map<string, Violation>();

  for (const own of scenarios.values()) {
    const order = [...background, ...own];
    const savedEver = new Set(order.map((e) => e.saves));
    const saved = new Set<string>();

    for (const { step, saves } of order) {
      for (const name of referencedNames(step)) {
        if (provided.has(name) || saved.has(name)) continue;
        const key = `${step.line}\u0000${name}`;
        if (seen.has(key)) continue;
        seen.set(key, {
          file: filePath,
          line: step.line,
          step: step.text,
          message: savedEver.has(name)
            ? `context variable '{${name}}' is used before the step that saves it`
            : `unknown context variable '{${name}}'`,
        });
      }
      if (saves !== null) saved.add(saves);
    }
  }

  return [...seen.values()].sort((a, b) => a.line - b.line);
}

/** Merge two line-ordered violation lists; the stable sort keeps `a` first on a tie. */
function mergeByLine(a: readonly Violation[], b: readonly Violation[]): Violation[] {
  return [...a, ...b].sort((x, y) => x.line - y.line);
}
