#!/usr/bin/env node
/**
 * cli.ts — `realspec validate <file.feature> ...`
 *
 * The registry that governs a feature file is found by walking up from the
 * file's directory to the nearest `format.yml` (stopping at a `spec/` boundary
 * or the filesystem root); `--format <path>` overrides this.
 *
 * Everything that can stop a run is settled before the first report line is
 * written: a format.yml that does not load, and — for a registry that binds
 * steps to an API contract — the `../openapi` directory next to it, which is
 * read once per format.yml alongside the registry. Either failure is one
 * `ERROR:` line on stderr and exit 1, never a half-written report.
 */

import * as fs from 'node:fs';
import * as path from 'node:path';
import { pathToFileURL } from 'node:url';

import { validate } from './checks.js';
import { loadOpenApi, type OpenApiIndex } from './openapi.js';
import { loadRegistry, RegistryError, usesOpenApi, type Registry } from './registry.js';
import { formatFileReport, formatMissingFile, pyPathStr } from './report.js';

const USAGE = 'Usage: realspec validate <file.feature> [<file.feature> ...]';

const HELP = `${USAGE}

Validates Gherkin .feature files against a bdd/format.yml step registry.

Options:
  --format <path>   Use this format.yml instead of discovering one.
  -h, --help        Show this message.

Exit codes:
  0  every file passes
  1  at least one violation, missing file, or usage error
`;

/** Stand-in for a file no registry was resolved for (unreachable once every file resolves). */
const EMPTY_REGISTRY: Registry = { steps: [], providedVars: null, substitutions: [] };

/** A loaded format.yml together with the API contract its steps are bound to. */
interface Loaded {
  readonly registry: Registry;
  /** `null` when no step of the registry carries an `openapi` binding. */
  readonly openapi: OpenApiIndex | null;
}

/** The outcome of one CLI invocation. */
export interface RunResult {
  readonly stdout: string;
  readonly stderr: string;
  readonly exitCode: number;
}

/** Options for {@link run}, mainly so tests can pin the working directory. */
export interface RunOptions {
  readonly cwd?: string;
}

/**
 * Locate the `format.yml` that governs a feature file.
 *
 * Walks up from `startDir`, stopping after a directory named `spec` (the
 * registry never lives above the spec root) or at the filesystem root.
 */
export function findFormatFile(startDir: string): string | null {
  let dir = path.resolve(startDir);
  for (;;) {
    const candidate = path.join(dir, 'format.yml');
    if (fs.existsSync(candidate)) return candidate;
    if (path.basename(dir) === 'spec') return null;
    const parent = path.dirname(dir);
    if (parent === dir) return null;
    dir = parent;
  }
}

interface ParsedArgs {
  readonly files: string[];
  readonly formatOverride: string | null;
  readonly help: boolean;
}

function parseArgs(argv: readonly string[]): ParsedArgs | { readonly error: string } {
  const files: string[] = [];
  let formatOverride: string | null = null;
  let help = false;
  let sawSubcommand = false;

  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i] ?? '';

    if (arg === '--format') {
      const next = argv[i + 1];
      if (next === undefined) return { error: '--format requires a path argument' };
      formatOverride = next;
      i += 1;
      continue;
    }
    if (arg.startsWith('--format=')) {
      const value = arg.slice('--format='.length);
      if (value === '') return { error: '--format requires a path argument' };
      formatOverride = value;
      continue;
    }
    if (arg === '-h' || arg === '--help') {
      help = true;
      continue;
    }
    if (arg.startsWith('-') && arg !== '-') {
      return { error: `unknown option: ${arg}` };
    }
    if (!sawSubcommand && files.length === 0 && arg === 'validate') {
      sawSubcommand = true;
      continue;
    }
    files.push(arg);
  }

  return { files, formatOverride, help };
}

/** Run the CLI over `argv` (the arguments after the program name). */
export function run(argv: readonly string[], options: RunOptions = {}): RunResult {
  const cwd = options.cwd ?? process.cwd();
  const parsed = parseArgs(argv);

  if ('error' in parsed) {
    return { stdout: '', stderr: `ERROR: ${parsed.error}\n${USAGE}\n`, exitCode: 1 };
  }
  if (parsed.help) {
    return { stdout: HELP, stderr: '', exitCode: 0 };
  }
  if (parsed.files.length === 0) {
    return { stdout: '', stderr: `${USAGE}\n`, exitCode: 1 };
  }

  // ── Resolve every registry (and its openapi directory) up front, so a bad
  //    format.yml or contract never produces a half-written report.
  const loadedByFormat = new Map<string, Loaded>();
  const perFile = new Map<string, Loaded>();

  let override: string | null = null;
  if (parsed.formatOverride !== null) {
    override = path.resolve(cwd, parsed.formatOverride);
    if (!fs.existsSync(override)) {
      return {
        stdout: '',
        stderr: `ERROR: format.yml not found at ${pyPathStr(parsed.formatOverride)}\n`,
        exitCode: 1,
      };
    }
  }

  for (const arg of parsed.files) {
    const abs = path.resolve(cwd, arg);
    if (!fs.existsSync(abs)) continue; // reported as "file not found" below

    let formatPath: string;
    if (override !== null) {
      formatPath = override;
    } else {
      const found = findFormatFile(path.dirname(abs));
      if (found === null) {
        return {
          stdout: '',
          stderr:
            `ERROR: no format.yml found for ${pyPathStr(arg)}\n` +
            `       searched ${path.dirname(abs)} and its parents; ` +
            `pass --format <path> to point at one\n`,
          exitCode: 1,
        };
      }
      formatPath = found;
    }

    let loaded = loadedByFormat.get(formatPath);
    if (loaded === undefined) {
      let source: string;
      try {
        source = fs.readFileSync(formatPath, 'utf8');
      } catch (err) {
        const detail = err instanceof Error ? err.message : String(err);
        return { stdout: '', stderr: `ERROR: cannot read ${formatPath}: ${detail}\n`, exitCode: 1 };
      }
      try {
        const registry = loadRegistry(source, formatPath);
        // The contract lives beside the registry: <spec>/bdd/format.yml and
        // <spec>/openapi. It is read only when a step is bound to it.
        const openapi = usesOpenApi(registry)
          ? loadOpenApi(path.resolve(path.dirname(formatPath), '../openapi'))
          : null;
        loaded = { registry, openapi };
      } catch (err) {
        if (err instanceof RegistryError) {
          return { stdout: '', stderr: `ERROR: ${err.message}\n`, exitCode: 1 };
        }
        throw err;
      }
      loadedByFormat.set(formatPath, loaded);
    }
    perFile.set(abs, loaded);
  }

  // ── Report ────────────────────────────────────────────────────────────────
  let stdout = '';
  let stderr = '';
  let anyFailure = false;

  for (const arg of parsed.files) {
    const shown = pyPathStr(arg);
    const abs = path.resolve(cwd, arg);

    if (!fs.existsSync(abs)) {
      stdout += formatMissingFile(shown);
      anyFailure = true;
      continue;
    }

    let source: string;
    try {
      source = fs.readFileSync(abs, 'utf8');
    } catch (err) {
      const detail = err instanceof Error ? err.message : String(err);
      stderr += `ERROR: cannot read ${shown}: ${detail}\n`;
      anyFailure = true;
      continue;
    }

    const loaded = perFile.get(abs);
    const violations =
      loaded === undefined
        ? validate(shown, source, EMPTY_REGISTRY)
        : validate(shown, source, loaded.registry, loaded.openapi);
    if (violations.length > 0) anyFailure = true;
    stdout += formatFileReport(shown, violations);
  }

  stdout += '\n';
  return { stdout, stderr, exitCode: anyFailure ? 1 : 0 };
}

/** Process entry point. */
export function main(argv: readonly string[] = process.argv.slice(2)): void {
  const result = run(argv);
  if (result.stdout !== '') process.stdout.write(result.stdout);
  if (result.stderr !== '') process.stderr.write(result.stderr);
  process.exitCode = result.exitCode;
}

// Node resolves symlinks before loading an ES module, so import.meta.url is the
// real path while argv[1] may be a symlink (node_modules/.bin/realspec, npx).
// Compare real paths, or a bin launch silently does nothing and exits 0.
const entry = process.argv[1];
if (entry !== undefined && import.meta.url === pathToFileURL(fs.realpathSync(entry)).href) {
  main();
}
