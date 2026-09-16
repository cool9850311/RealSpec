/**
 * The §6.A CLI test plan, cases 1–20, plus case 22 for the context-variable
 * check. Case 21 (parity against validate.py) lives in parity.test.ts.
 *
 * Every expectation here was pinned by running the original project's validate.py over
 * the same fixture with the same format.yml — validate.py is the specification.
 */

import { readFileSync } from 'node:fs';
import * as path from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

import { checkSql, pyReprStrList, pyReprStrSet, pySorted, validate, type Violation } from '../src/checks.js';
import { parseSteps } from '../src/parse.js';
import { loadRegistry, loadSteps } from '../src/registry.js';
import { run } from '../src/cli.js';

const FIXTURES = fileURLToPath(new URL('./fixtures/', import.meta.url));

const dir = (name: string): string => path.join(FIXTURES, name);

/** Validate one fixture the way the CLI would, but return the raw violations. */
function violationsOf(name: string, file = 'x.feature'): Violation[] {
  const base = dir(name);
  const registry = loadRegistry(readFileSync(path.join(base, 'format.yml'), 'utf8'), 'format.yml');
  return validate(
    file,
    readFileSync(path.join(base, file), 'utf8'),
    registry.steps,
    registry.providedVars,
  );
}

/** Run the CLI inside a fixture directory, letting it discover format.yml. */
function cli(name: string, args: string[]): ReturnType<typeof run> {
  return run(args, { cwd: dir(name) });
}

const BAR = '─'.repeat(68);

describe('1. step missing from the registry', () => {
  it('reports no matching step definition', () => {
    const v = violationsOf('case01-unknown-step');
    expect(v).toHaveLength(1);
    expect(v[0]?.line).toBe(5);
    expect(v[0]?.message).toBe('no matching step definition found in format.yml');
    expect(v[0]?.step).toBe('something nobody registered');
  });
});

describe('2. keyword not allowed for the matched step', () => {
  it('names the keyword and lists the allowed ones', () => {
    const v = violationsOf('case02-keyword');
    expect(v).toHaveLength(2);
    expect(v[0]?.line).toBe(4);
    expect(v[0]?.message).toBe("keyword 'When' is not allowed here (allowed: ['Given'])");
    expect(v[1]?.line).toBe(5);
    expect(v[1]?.message).toBe("keyword 'Then' is not allowed here (allowed: ['And', 'Given'])");
  });

  it("renders the allowed list as Python's sorted() repr", () => {
    expect(pyReprStrList(pySorted(['Given', 'And']))).toBe("['And', 'Given']");
    expect(pyReprStrList([])).toBe('[]');
  });
});

describe('3. docstring required but absent', () => {
  it('names the step id and the uppercased type', () => {
    const v = violationsOf('case03-missing-docstring');
    expect(v).toHaveLength(1);
    expect(v[0]?.line).toBe(4);
    expect(v[0]?.message).toBe("step 'response_body_contains' requires a JSON docstring");
  });
});

describe('4. docstring supplied where none is allowed', () => {
  it('reports must not have a docstring', () => {
    const v = violationsOf('case04-forbidden-docstring');
    expect(v).toHaveLength(1);
    expect(v[0]?.line).toBe(4);
    expect(v[0]?.message).toBe("step 'response_status' must not have a docstring");
  });
});

describe('5. wrong docstring marker', () => {
  it('reports the marker and skips the content check', () => {
    const v = violationsOf('case05-marker');
    expect(v).toHaveLength(1);
    expect(v[0]?.line).toBe(4);
    expect(v[0]?.message).toBe("docstring marker is 'json' but must be 'sql'");
    // The body is `DELETE FROM users;` — had the content check still run, a
    // second "must begin with SELECT" violation would appear.
    expect(v.some((x) => x.message.includes('SELECT'))).toBe(false);
  });
});

describe('6. invalid JSON docstring', () => {
  it("reports CPython's parse error, against the step's own line", () => {
    const v = violationsOf('case06-bad-json');
    expect(v).toHaveLength(1);
    // The line number is the step's line (4), not a line inside the docstring;
    // the docstring's own line/column appear inside the message text.
    expect(v[0]?.line).toBe(4);
    expect(v[0]?.message).toBe(
      "docstring is not valid JSON after substitution: Expecting ',' delimiter: line 3 column 9 (char 25)",
    );
  });
});

describe('7. context variable substitution', () => {
  it('accepts "{sessionVar}" in JSON and SQL docstrings', () => {
    expect(violationsOf('case07-ctx-var')).toEqual([]);
  });
});

describe('8. <non-null> marker', () => {
  it('accepts "<non-null>" as a JSON string value', () => {
    expect(violationsOf('case08-non-null')).toEqual([]);
  });
});

describe('9. outline parameter in both step text and docstring', () => {
  it('normalises both', () => {
    expect(violationsOf('case09-outline-both')).toEqual([]);
  });
});

describe('10. query-assertion SQL that is not a SELECT', () => {
  it('reports the violation', () => {
    const v = violationsOf('case10-sql-not-select');
    expect(v).toHaveLength(1);
    expect(v[0]?.line).toBe(4);
    expect(v[0]?.message).toBe('SQL in a query-assertion step must begin with SELECT');
  });
});

describe('11. what may precede SELECT', () => {
  it('rejects a leading SQL comment but accepts a plain SELECT', () => {
    // The plan's table said "whitespace and a comment, then SELECT → PASS".
    // validate.py's `^\s*SELECT\b` allows only whitespace, so the commented
    // variant is a violation; validate.py is the authority.
    const v = violationsOf('case11-sql-select-prefix');
    expect(v).toHaveLength(1);
    expect(v[0]?.line).toBe(8);
    expect(v[0]?.message).toBe('SQL in a query-assertion step must begin with SELECT');
  });

  it('accepts leading whitespace before SELECT', () => {
    // A docstring body is .strip()ed before it reaches the check, so this is
    // only reachable at the unit level — but the rule is still the rule.
    expect(checkSql('   \t\n SELECT 1', 'postgresql_query_returns')).toBeNull();
    expect(checkSql('select 1', 'postgresql_query_returns')).toBeNull();
    expect(checkSql('SELECTED 1', 'postgresql_query_returns')).toBe(
      'SQL in a query-assertion step must begin with SELECT',
    );
    expect(checkSql('-- c\nSELECT 1', 'postgresql_query_returns')).toBe(
      'SQL in a query-assertion step must begin with SELECT',
    );
  });

  it('applies the SELECT rule only to query-assertion steps', () => {
    expect(checkSql('DELETE FROM t', 'exec_postgresql')).toBeNull();
    expect(checkSql('DELETE FROM t', 'clickhouse_query_returns')).toBe(
      'SQL in a query-assertion step must begin with SELECT',
    );
  });

  it('rejects an empty SQL docstring', () => {
    expect(checkSql('   ', 'exec_postgresql')).toBe('SQL docstring must not be empty');
  });
});

describe('12. http_request with an unexpected top-level key', () => {
  it("names the key using Python's set repr", () => {
    const v = violationsOf('case12-http-extra-key');
    expect(v).toHaveLength(1);
    expect(v[0]?.line).toBe(4);
    expect(v[0]?.message).toBe(
      "http_request docstring has unexpected top-level keys: {'query'}",
    );
    expect(pyReprStrSet(['query'])).toBe("{'query'}");
  });
});

describe('13. http_request with an empty object', () => {
  it('passes', () => {
    expect(violationsOf('case13-http-empty-object')).toEqual([]);
  });
});

describe('14. outline token inside the request path', () => {
  it('passes after token normalisation', () => {
    expect(violationsOf('case14-outline-path')).toEqual([]);
  });
});

describe('15. Examples table rows', () => {
  it('are not treated as steps', () => {
    const source = readFileSync(path.join(dir('case15-examples-rows'), 'x.feature'), 'utf8');
    const steps = parseSteps(source);
    expect(steps.map((s) => s.text)).toEqual([
      'run migration',
      'GET /api/v1/items/<id>:',
      'response status is 200',
    ]);
    expect(violationsOf('case15-examples-rows')).toEqual([]);
  });
});

describe('16. comments and blank lines', () => {
  it('are skipped', () => {
    const source = readFileSync(path.join(dir('case16-comments-blanks'), 'x.feature'), 'utf8');
    const steps = parseSteps(source);
    expect(steps.map((s) => [s.line, s.keyword, s.text])).toEqual([
      [8, 'Given', 'run migration'],
      [11, 'Then', 'response status is 200'],
    ]);
    expect(violationsOf('case16-comments-blanks')).toEqual([]);
  });
});

describe('17. triple-quote variants inside a docstring body', () => {
  it('behaves exactly as validate.py does', () => {
    const v = violationsOf('case17-triple-quote');
    expect(v).toHaveLength(2);
    // A bare `"""` in the middle of a body line stays part of the body.
    expect(v[0]?.line).toBe(4);
    expect(v[0]?.message).toBe(
      "docstring is not valid JSON after substitution: Expecting ',' delimiter: line 1 column 9 (char 8)",
    );
    // A body line that itself starts with `"""` closes the docstring early.
    expect(v[1]?.line).toBe(10);
    expect(v[1]?.message).toBe(
      'docstring is not valid JSON after substitution: Expecting property name enclosed in double quotes: line 1 column 9 (char 8)',
    );

    const source = readFileSync(path.join(dir('case17-triple-quote'), 'x.feature'), 'utf8');
    const steps = parseSteps(source);
    expect(steps[0]?.dsContent).toBe('{"a": """}');
    expect(steps[1]?.dsContent).toBe('{"a": 1,');
  });
});

describe('18. exit codes', () => {
  it('exits 0 when every file passes', () => {
    const r = cli('case18-pass', ['validate', 'x.feature']);
    expect(r.exitCode).toBe(0);
    expect(r.stderr).toBe('');
    expect(r.stdout).toBe(`\n${BAR}\n  PASS  x.feature  (0 violation(s))\n${BAR}\n\n`);
  });

  it('exits 1 when a file has violations', () => {
    const r = cli('case01-unknown-step', ['validate', 'x.feature']);
    expect(r.exitCode).toBe(1);
  });

  it('accepts the bare form without the validate subcommand', () => {
    expect(cli('case18-pass', ['x.feature']).exitCode).toBe(0);
  });

  it('honours an explicit --format', () => {
    const r = cli('case18-pass', ['validate', '--format', 'format.yml', 'x.feature']);
    expect(r.exitCode).toBe(0);
    expect(cli('case18-pass', ['--format=format.yml', 'x.feature']).exitCode).toBe(0);
  });
});

describe('19. several files at once', () => {
  it('prints one block per file and aggregates the exit code', () => {
    const r = cli('case19-multi', ['validate', 'good.feature', 'bad.feature', 'missing.feature']);
    expect(r.exitCode).toBe(1);
    expect(r.stdout).toBe(
      `\n${BAR}\n` +
        `  PASS  good.feature  (0 violation(s))\n` +
        `${BAR}\n` +
        `\n${BAR}\n` +
        `  FAIL  bad.feature  (2 violation(s))\n` +
        `${BAR}\n` +
        `  line    4  no matching step definition found in format.yml\n` +
        `           → this step does not exist\n` +
        `  line    5  no matching step definition found in format.yml\n` +
        `           → neither does this one\n` +
        `\nERROR  file not found: missing.feature\n` +
        `\n`,
    );
  });
});

describe('20. bad invocations and bad registries', () => {
  it('prints usage and exits 1 with no arguments', () => {
    const r = run([], { cwd: dir('case18-pass') });
    expect(r.exitCode).toBe(1);
    expect(r.stdout).toBe('');
    expect(r.stderr).toContain('Usage: realspec validate <file.feature>');
  });

  it('rejects an unknown option', () => {
    const r = cli('case18-pass', ['--nope', 'x.feature']);
    expect(r.exitCode).toBe(1);
    expect(r.stderr).toContain('unknown option: --nope');
  });

  it('rejects --format without a value', () => {
    const r = cli('case18-pass', ['x.feature', '--format']);
    expect(r.exitCode).toBe(1);
    expect(r.stderr).toContain('--format requires a path argument');
  });

  it('reports a missing --format target', () => {
    const r = cli('case18-pass', ['--format', 'nope.yml', 'x.feature']);
    expect(r.exitCode).toBe(1);
    expect(r.stdout).toBe('');
    expect(r.stderr).toBe('ERROR: format.yml not found at nope.yml\n');
  });

  it('reports a missing --format target even when no feature file exists', () => {
    const r = cli('case18-pass', ['--format', 'nope.yml', 'gone.feature']);
    expect(r.exitCode).toBe(1);
    expect(r.stdout).toBe('');
    expect(r.stderr).toBe('ERROR: format.yml not found at nope.yml\n');
  });

  it('reports an undiscoverable format.yml and stops at the spec/ boundary', () => {
    const r = run(['x.feature'], { cwd: path.join(dir('case20-no-format'), 'spec') });
    expect(r.exitCode).toBe(1);
    expect(r.stdout).toBe('');
    expect(r.stderr).toContain('no format.yml found for x.feature');
    expect(r.stderr).toContain('--format');
  });

  it('reports invalid YAML without a stack trace', () => {
    const r = cli('case20-broken-format', ['x.feature']);
    expect(r.exitCode).toBe(1);
    expect(r.stdout).toBe('');
    expect(r.stderr).toContain('is not valid YAML');
    expect(r.stderr).not.toContain('at Object.');
  });

  it('reports a step definition with no pattern', () => {
    const r = cli('case20-no-pattern', ['x.feature']);
    expect(r.exitCode).toBe(1);
    expect(r.stdout).toBe('');
    expect(r.stderr).toContain("steps[0] is missing a string 'pattern'");
  });

  it('prints help on --help and exits 0', () => {
    const r = run(['--help'], { cwd: dir('case18-pass') });
    expect(r.exitCode).toBe(0);
    expect(r.stdout).toContain('Usage: realspec validate');
  });
});

describe('documentation-only registry fields', () => {
  it('never enforces content_pattern, captures, schema or special_values', () => {
    // The fixture registry gives postgresql_query_returns a content_pattern of
    // `^(?!\s*$)\s*SELECT\s[\s\S]+$`; `SELECT 1` has no whitespace after
    // SELECT... and yet validate.py accepts it, because it ignores the field.
    const steps = loadSteps(
      readFileSync(path.join(dir('case07-ctx-var'), 'format.yml'), 'utf8'),
      'format.yml',
    );
    const source = [
      'Feature: f',
      '',
      '  Scenario: s',
      '    Then in PostgreSQL query returns 1 rows:',
      '      """sql',
      '      SELECT 1',
      '      """',
      '',
    ].join('\n');
    expect(validate('f.feature', source, steps)).toEqual([]);
  });
});

describe('22. context variables', () => {
  const case22 = (file: string): Violation[] => violationsOf('case22-context-vars', file);

  it('accepts a name from context_variables.provided', () => {
    expect(case22('provided-ok.feature')).toEqual([]);
  });

  it('rejects a misspelling of a provided name', () => {
    const v = case22('provided-typo.feature');
    expect(v).toHaveLength(1);
    expect(v[0]?.line).toBe(4);
    expect(v[0]?.message).toBe("unknown context variable '{tokenExpried}'");
  });

  it('rejects a reference that precedes the step saving it', () => {
    const v = case22('use-before-save.feature');
    expect(v).toHaveLength(1);
    expect(v[0]?.line).toBe(4);
    expect(v[0]?.message).toBe(
      "context variable '{orderId}' is used before the step that saves it",
    );
  });

  it('accepts the same reference once the save step precedes it', () => {
    expect(case22('use-after-save.feature')).toEqual([]);
  });

  it('carries a Background save into every scenario, reported once', () => {
    expect(case22('background-save.feature')).toEqual([]);
  });

  it('resolves produces through the named capture, not the first group', () => {
    const steps = loadSteps(
      readFileSync(path.join(dir('case22-context-vars'), 'format.yml'), 'utf8'),
      'format.yml',
    );
    // `field` is group 1 and `varName` group 2; a step saving field "id" as
    // "orderId" must put orderId in scope, never id.
    expect(steps.find((s) => s.id === 'save_response_body_field')?.producesGroup).toBe(2);
  });

  it('does nothing for a registry with neither context_variables nor produces', () => {
    // case07's feature uses {sessionId}, which nothing provides or saves.
    expect(violationsOf('case07-ctx-var')).toEqual([]);
    const registry = loadRegistry(
      readFileSync(path.join(dir('case07-ctx-var'), 'format.yml'), 'utf8'),
      'format.yml',
    );
    expect(registry.providedVars).toBeNull();
    expect(registry.steps.every((s) => s.producesGroup === null)).toBe(true);
  });
});
