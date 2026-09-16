/**
 * checks.ts — content validators and the core validator loop.
 *
 * Port of `_check_json()`, `_check_http_body()`, `_check_sql()` and
 * `validate()` in the original `validate.py`.
 *
 * The JSON error messages produced here must be byte-identical to CPython's
 * `json.JSONDecodeError`, so this module carries a faithful re-implementation
 * of CPython's JSON scanner (`Lib/json/decoder.py` + the `_json` C
 * accelerator, which is the one actually used by `json.loads`).
 *
 * The context-variable check at the end is a RealSpec addition with no
 * counterpart in validate.py; it is inert unless the registry declares
 * `context_variables` or `produces`, which is what keeps parity intact.
 */

import { parseSteps, pyStrip, type Step } from './parse.js';
import { matchesStepText, type StepDef } from './registry.js';

// ── Docstring substitution patterns (mirrors validate.py) ────────────────────
// These tokens are replaced with neutral values before JSON/SQL parsing so the
// validator does not reject intentionally non-literal placeholders.

/** `"{wagerId}"` → context variable inside a JSON string value. */
const CTX_JSON_RE = /"\{[a-z][a-zA-Z0-9]+\}"/g;
/** `"<non-null>"` → special assertion marker inside a JSON string value. */
const NON_NULL_RE = /"<non-null>"/g;
/** `"<paramName>"` → Scenario Outline parameter inside a JSON string value. */
const OUTLINE_JSON_RE = /"<[a-zA-Z][a-zA-Z0-9]*>"/g;
/** `'{wagerId}'` → context variable inside a SQL single-quoted literal. */
const CTX_SQL_RE = /'\{[a-z][a-zA-Z0-9]+\}'/g;
/** `<paramName>` in step text (Scenario Outline). */
const OUTLINE_STEP_RE = /<[a-zA-Z][a-zA-Z0-9]*>/g;
/** `{varName}` anywhere in a step's text or docstring. */
const CTX_NAME_RE = /\{([a-z][a-zA-Z0-9]+)\}/g;

/** Step ids whose SQL docstring must be a SELECT query. */
const QUERY_ASSERTION_IDS = new Set(['postgresql_query_returns', 'clickhouse_query_returns']);

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

function substituteJson(content: string): string {
  return content
    .replace(CTX_JSON_RE, '"__ctx__"')
    .replace(NON_NULL_RE, '"__non_null__"')
    .replace(OUTLINE_JSON_RE, '"__outline__"');
}

/** Return an error string if `content` is not valid JSON after substitution. */
export function checkJson(content: string): string | null {
  const result = pyJsonLoads(substituteJson(content));
  if (!result.ok) return `docstring is not valid JSON after substitution: ${result.error}`;
  return null;
}

/**
 * For `http_request` steps the docstring must be a JSON object whose only
 * top-level keys are `headers` and `body`. Both are optional: the runner sends
 * no request body when `body` is absent, `{}` or `null`, so a GET that needs
 * neither may use an empty `{}` docstring.
 */
export function checkHttpBody(content: string): string | null {
  const err = checkJson(content);
  if (err) return err;

  const result = pyJsonLoads(substituteJson(content));
  if (!result.ok) return `docstring is not valid JSON after substitution: ${result.error}`;

  const obj = result.value;
  if (obj.t !== 'obj') return 'http_request docstring must be a JSON object';

  const allowed = new Set(['headers', 'body']);
  const seen = new Set<string>();
  const extra: string[] = [];
  for (const [key] of obj.v) {
    if (seen.has(key)) continue;
    seen.add(key);
    if (!allowed.has(key)) extra.push(key);
  }
  if (extra.length > 0) {
    return `http_request docstring has unexpected top-level keys: ${pyReprStrSet(extra)}`;
  }
  return null;
}

// Python's `\s` and `\b` are Unicode-aware for `str` patterns while
// JavaScript's are not, so both are spelled out here.
const PY_SPACE_CLASS =
  '\\t\\n\\v\\f\\r\\u001c-\\u001f \\u0085\\u00a0\\u1680\\u2000-\\u200a\\u2028\\u2029\\u202f\\u205f\\u3000';
const SQL_SELECT_RE = new RegExp(`^[${PY_SPACE_CLASS}]*SELECT`, 'i');
const WORD_CHAR_RE = /[\p{L}\p{N}_]/u;

/** Return an error string if the SQL content violates format rules. */
export function checkSql(content: string, stepId: string): string | null {
  if (pyStrip(content) === '') return 'SQL docstring must not be empty';
  const cleaned = content.replace(CTX_SQL_RE, "'__ctx__'");
  // Assertion steps must be SELECT queries
  if (QUERY_ASSERTION_IDS.has(stepId)) {
    const m = SQL_SELECT_RE.exec(cleaned);
    let matched = m !== null;
    if (m !== null) {
      const after = cleaned.slice(m[0].length);
      if (after !== '') {
        const first = String.fromCodePoint(after.codePointAt(0) ?? 0);
        if (WORD_CHAR_RE.test(first)) matched = false; // \b requires a boundary
      }
    }
    if (!matched) {
      return 'SQL in a query-assertion step must begin with SELECT';
    }
  }
  return null;
}

// ─────────────────────────────────────────────────────────────────────────────
// Core validator
// ─────────────────────────────────────────────────────────────────────────────

/**
 * Validate one `.feature` source against the compiled step registry.
 *
 * @param providedVars `context_variables.provided`, or `null` when the registry
 *                     declares none — see {@link checkContextVariables}.
 */
export function validate(
  filePath: string,
  source: string,
  compiledSteps: readonly StepDef[],
  providedVars: ReadonlySet<string> | null = null,
): Violation[] {
  const violations: Violation[] = [];
  const steps = parseSteps(source);
  /** The StepDef each step matched, aligned with `steps`; `null` when none. */
  const matched: Array<StepDef | null> = steps.map(() => null);

  for (const [i, step] of steps.entries()) {
    // Normalise Scenario Outline tokens so they don't block pattern matching
    const normalised = step.text.replace(OUTLINE_STEP_RE, 'OUTLINE_PARAM');

    // ── 1. Find steps whose pattern matches ──────────────────────────────
    const candidates = compiledSteps.filter((cs) => matchesStepText(cs, normalised));

    if (candidates.length === 0) {
      violations.push({
        file: filePath,
        line: step.line,
        step: step.text,
        message: 'no matching step definition found in format.yml',
      });
      continue;
    }

    // ── 2. Among candidates pick one that allows this keyword ────────────
    const match = candidates.find((cs) => cs.keywords.includes(step.keyword));

    if (match === undefined) {
      const allowed = pySorted([...new Set(candidates.flatMap((cs) => [...cs.keywords]))]);
      violations.push({
        file: filePath,
        line: step.line,
        step: step.text,
        message: `keyword '${step.keyword}' is not allowed here (allowed: ${pyReprStrList(allowed)})`,
      });
      continue;
    }

    matched[i] = match;

    // ── 3. Docstring presence ────────────────────────────────────────────
    const expectsDs = match.docstring; // null → no docstring allowed

    if (expectsDs === null && step.dsContent !== null) {
      violations.push({
        file: filePath,
        line: step.line,
        step: step.text,
        message: `step '${match.id}' must not have a docstring`,
      });
      continue;
    }

    if (expectsDs !== null && step.dsContent === null) {
      violations.push({
        file: filePath,
        line: step.line,
        step: step.text,
        message: `step '${match.id}' requires a ${(expectsDs.type ?? '').toUpperCase()} docstring`,
      });
      continue;
    }

    if (step.dsContent === null) continue; // no docstring — nothing more to check

    // ── 4. Docstring marker ──────────────────────────────────────────────
    const expectedType = expectsDs === null ? undefined : expectsDs.type;

    if (expectedType !== undefined && expectedType !== '' && step.dsMarker !== expectedType) {
      violations.push({
        file: filePath,
        line: step.line,
        step: step.text,
        message: `docstring marker is '${step.dsMarker}' but must be '${expectedType}'`,
      });
      continue; // skip content check — wrong parser would give misleading errors
    }

    // ── 5. Docstring content ─────────────────────────────────────────────
    let err: string | null = null;
    if (expectedType === 'json') {
      err = match.id === 'http_request' ? checkHttpBody(step.dsContent) : checkJson(step.dsContent);
    } else if (expectedType === 'sql') {
      err = checkSql(step.dsContent, match.id);
    }

    if (err !== null) {
      violations.push({ file: filePath, line: step.line, step: step.text, message: err });
    }
  }

  const ctx = checkContextVariables(filePath, steps, matched, compiledSteps, providedVars);
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
  const m = def.regex.exec(step.text.replace(OUTLINE_STEP_RE, 'OUTLINE_PARAM'));
  return m?.[def.producesGroup] ?? null;
}

/**
 * Every `{name}` a step uses must already be in scope: declared under
 * `context_variables.provided`, or saved by an earlier step of the same
 * scenario (Background steps count as earlier).
 *
 * Inert unless the registry declares `context_variables` or at least one
 * `produces` — which is what keeps this check out of the original project's parity corpus.
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
