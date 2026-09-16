/**
 * parse.ts — Gherkin tokeniser.
 *
 * A line-for-line port of `parse_steps()` in the original
 * `validate.py`. Every quirk of the Python original is reproduced
 * deliberately, including CPython's `str.splitlines()` / `str.strip()`
 * semantics, so that the two implementations tokenise byte-identically.
 */

/** A single Gherkin step together with its optional docstring. */
export interface Step {
  /** 1-based line number of the step keyword. */
  readonly line: number;
  /** `Given` | `When` | `Then` | `And` | `But`. */
  readonly keyword: string;
  /** Step text with the keyword stripped, right-trimmed. */
  readonly text: string;
  /** `'sql'`, `'json'`, `''`, or `null` when the step has no docstring. */
  readonly dsMarker: string | null;
  /** Raw docstring body (stripped), or `null` when absent. */
  readonly dsContent: string | null;
  /** True when the step sits inside a `Background:` block. */
  readonly background: boolean;
  /** 0-based index of the enclosing `Scenario:`; -1 before the first one. */
  readonly scenario: number;
}

// ── Gherkin tokeniser constants (mirrors validate.py) ────────────────────────

const STEP_RE = /^(Given|When|Then|And|But)\s+(.+)$/;
const SECTION_RE = /^(Feature:|Background:|Scenario Outline:|Scenario:|Examples:)/;
const TABLE_RE = /^\|/;
const DS_OPEN_RE = /^"""/;

// ── CPython string primitives ────────────────────────────────────────────────

/**
 * Line boundaries recognised by CPython's `str.splitlines()`.
 * JavaScript has no direct equivalent, so the full set is spelled out.
 */
const LINE_BOUNDARY_RE = /\r\n|[\n\r\v\f\u001c\u001d\u001e\u0085\u2028\u2029]/g;

/** Characters for which CPython's `str.isspace()` returns true. */
const PY_SPACE_CLASS =
  '\\t\\n\\v\\f\\r\\u001c-\\u001f \\u0085\\u00a0\\u1680\\u2000-\\u200a\\u2028\\u2029\\u202f\\u205f\\u3000';

const PY_LSTRIP_RE = new RegExp(`^[${PY_SPACE_CLASS}]+`);
const PY_RSTRIP_RE = new RegExp(`[${PY_SPACE_CLASS}]+$`);
const PY_SPLIT_RE = new RegExp(`[${PY_SPACE_CLASS}]+`);
const PY_IS_SPACE_RE = new RegExp(`^[${PY_SPACE_CLASS}]*$`);

/** CPython `str.splitlines()`: no trailing empty element for a trailing newline. */
export function pySplitlines(text: string): string[] {
  const out: string[] = [];
  let start = 0;
  LINE_BOUNDARY_RE.lastIndex = 0;
  let m = LINE_BOUNDARY_RE.exec(text);
  while (m !== null) {
    out.push(text.slice(start, m.index));
    start = m.index + m[0].length;
    LINE_BOUNDARY_RE.lastIndex = start;
    m = LINE_BOUNDARY_RE.exec(text);
  }
  if (start < text.length) out.push(text.slice(start));
  return out;
}

/** CPython `str.strip()` (no argument). */
export function pyStrip(s: string): string {
  return s.replace(PY_LSTRIP_RE, '').replace(PY_RSTRIP_RE, '');
}

/** CPython `str.rstrip()` (no argument). */
export function pyRstrip(s: string): string {
  return s.replace(PY_RSTRIP_RE, '');
}

/** CPython `' '.join(s.split())`: collapse whitespace runs, trim the ends. */
export function pyCollapseWhitespace(s: string): string {
  const trimmed = pyStrip(s);
  if (trimmed === '') return '';
  return trimmed.split(PY_SPLIT_RE).join(' ');
}

/** CPython `not s.strip()` — true for empty or all-whitespace strings. */
function isBlank(s: string): boolean {
  return PY_IS_SPACE_RE.test(s);
}

// ── .feature parser ──────────────────────────────────────────────────────────

/**
 * Return every step in a `.feature` source.
 *
 * Skips Feature/Scenario/Background/Examples headers, Examples table rows,
 * comments and blank lines. Docstrings are collected in full (marker + body).
 */
export function parseSteps(source: string): Step[] {
  const lines = pySplitlines(source);
  const steps: Step[] = [];
  let inExamples = false;
  let inBackground = false;
  let scenario = -1;
  let i = 0;

  while (i < lines.length) {
    const raw = lines[i] ?? '';
    const stripped = pyStrip(raw);

    if (stripped === '' || stripped.startsWith('#')) {
      i += 1;
      continue;
    }

    if (SECTION_RE.test(stripped)) {
      inExamples = stripped.startsWith('Examples:');
      // `Examples:` belongs to the Scenario Outline above it, so it changes
      // neither flag.
      if (stripped.startsWith('Background:')) {
        inBackground = true;
      } else if (stripped.startsWith('Scenario')) {
        inBackground = false;
        scenario += 1;
      } else if (stripped.startsWith('Feature:')) {
        inBackground = false;
      }
      i += 1;
      continue;
    }

    if (inExamples && TABLE_RE.test(stripped)) {
      i += 1;
      continue;
    }

    const m = STEP_RE.exec(stripped);
    if (m === null) {
      i += 1;
      continue;
    }

    const keyword = m[1] ?? '';
    const stepText = pyRstrip(m[2] ?? '');
    const lineNo = i + 1;

    // ── Look ahead for a docstring ───────────────────────────────────────
    let j = i + 1;
    while (j < lines.length && isBlank(lines[j] ?? '')) {
      j += 1;
    }

    let dsMarker: string | null = null;
    let dsContent: string | null = null;

    if (j < lines.length && DS_OPEN_RE.test(pyStrip(lines[j] ?? ''))) {
      const opener = pyStrip(lines[j] ?? ''); // e.g. '"""json'
      dsMarker = pyStrip(opener.slice(3)); // 'sql', 'json', or ''
      const bodyLines: string[] = [];
      j += 1;
      while (j < lines.length && !DS_OPEN_RE.test(pyStrip(lines[j] ?? ''))) {
        bodyLines.push(lines[j] ?? '');
        j += 1;
      }
      dsContent = pyStrip(bodyLines.join('\n'));
      i = j + 1; // skip the closing """
    } else {
      i += 1;
    }

    steps.push({
      line: lineNo,
      keyword,
      text: stepText,
      dsMarker,
      dsContent,
      background: inBackground,
      scenario,
    });
  }

  return steps;
}
