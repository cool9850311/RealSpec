/**
 * report.ts — console report rendering.
 *
 * The report layout is fixed: a ruled heading per file, one entry per
 * violation, a trailing blank line. Paths are displayed the way CPython's
 * `str(pathlib.Path(arg))` normalises them, so a report reads the same
 * whichever way a path was typed.
 */

import type { Violation } from './checks.js';

/** The 68-character rule printed above and below each file heading. */
const BAR = '─'.repeat(68);

/**
 * CPython `str(pathlib.Path(arg))`.
 *
 * `Path` normalises on construction: redundant separators and `.` components
 * are dropped, `..` is preserved, and an empty path renders as `.`.
 * Exactly two leading slashes are significant in POSIX and are preserved.
 */
export function pyPathStr(arg: string): string {
  let rest = arg;
  let root = '';
  if (rest.startsWith('/')) {
    let slashes = 0;
    while (rest[slashes] === '/') slashes += 1;
    root = slashes === 2 ? '//' : '/';
    rest = rest.slice(slashes);
  }
  const parts = rest.split('/').filter((p) => p !== '' && p !== '.');
  const joined = root + parts.join('/');
  return joined === '' ? '.' : joined;
}

/** CPython `format(n, '4d')`. */
function pad4(n: number): string {
  const s = String(n);
  return s.length >= 4 ? s : ' '.repeat(4 - s.length) + s;
}

/** One violation as it appears in a file's report block. */
export function formatViolation(v: Violation): string {
  return `  line ${pad4(v.line)}  ${v.message}\n           → ${v.step}`;
}

/** The per-file block, including its trailing newline. */
export function formatFileReport(pathStr: string, violations: readonly Violation[]): string {
  const status = violations.length === 0 ? 'PASS' : 'FAIL';
  let out = `\n${BAR}\n`;
  out += `  ${status}  ${pathStr}  (${violations.length} violation(s))\n`;
  out += `${BAR}\n`;
  for (const v of violations) out += `${formatViolation(v)}\n`;
  return out;
}

/** The line printed for a path that does not exist. */
export function formatMissingFile(pathStr: string): string {
  return `\nERROR  file not found: ${pathStr}\n`;
}
