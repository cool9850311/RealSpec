/**
 * The CLI test plan, cases 1–20, plus case 22 for the context-variable check,
 * case 23 for `step_ref` and case 24 for unused registry steps.
 *
 * Each case is a directory under fixtures/ holding a format.yml and one or more
 * feature files. The expectations are the validator's own contract: the message
 * texts, the line each violation is reported against and the exit codes. Every
 * rule a case exercises is declared in that case's format.yml (the SELECT rule
 * of case 10 and 11, the allowed keys of case 12), which is why the rules can be
 * checked here without the validator knowing any of them.
 */

import { readFileSync } from 'node:fs';
import * as path from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

import { pyReprStrList, pyReprStrSet, pySorted, validate, type Violation } from '../src/checks.js';
import { parseSteps } from '../src/parse.js';
import { loadRegistry, loadSteps, type Registry } from '../src/registry.js';
import { run } from '../src/cli.js';

const FIXTURES = fileURLToPath(new URL('./fixtures/', import.meta.url));

const dir = (name: string): string => path.join(FIXTURES, name);

/** Validate one fixture the way the CLI would, but return the raw violations. */
function violationsOf(name: string, file = 'x.feature'): Violation[] {
  const base = dir(name);
  const registry = loadRegistry(readFileSync(path.join(base, 'format.yml'), 'utf8'), 'format.yml');
  return validate(file, readFileSync(path.join(base, file), 'utf8'), registry);
}

/** Load a fixture's registry, for the tests that validate a source built in place. */
function registryOf(name: string): Registry {
  return loadRegistry(readFileSync(path.join(dir(name), 'format.yml'), 'utf8'), 'format.yml');
}

/**
 * Validate a one-step feature whose docstring is `body`, through the registry
 * of fixture `name`. `header` is the step line, e.g. `Then in PostgreSQL:`.
 */
function docstringViolations(name: string, header: string, marker: string, body: string): Violation[] {
  const source = [
    'Feature: f',
    '',
    '  Scenario: s',
    `    ${header}`,
    `      """${marker}`,
    ...body.split('\n').map((line) => `      ${line}`),
    '      """',
    '',
  ].join('\n');
  return validate('f.feature', source, registryOf(name));
}

/**
 * Run the CLI inside a fixture directory, letting it discover format.yml.
 *
 * A fixture's registry holds more steps than its features use, and every case
 * but 24 is about something else, so unused steps are allowed here; case 24
 * calls {@link run} itself.
 */
function cli(name: string, args: string[]): ReturnType<typeof run> {
  return run([...args, '--allow-unused-steps'], { cwd: dir(name) });
}

/** Run the CLI in a fixture directory exactly as given — unused steps fail. */
function strict(name: string, args: string[]): ReturnType<typeof run> {
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
  const QUERY = 'Then in PostgreSQL query returns 1 rows:';
  const MESSAGE = 'SQL in a query-assertion step must begin with SELECT';

  it('rejects a leading SQL comment but accepts a plain SELECT', () => {
    // `must_match` is `^\s*[Ss][Ee][Ll][Ee][Cc][Tt]\b`: whitespace may precede
    // SELECT, a comment may not.
    const v = violationsOf('case11-sql-select-prefix');
    expect(v).toHaveLength(1);
    expect(v[0]?.line).toBe(8);
    expect(v[0]?.message).toBe(MESSAGE);
  });

  it('accepts SELECT in any letter case, and rejects a word that merely starts with it', () => {
    const fixture = 'case11-sql-select-prefix';
    expect(docstringViolations(fixture, QUERY, 'sql', 'select 1')).toEqual([]);
    expect(docstringViolations(fixture, QUERY, 'sql', 'SeLeCt 1')).toEqual([]);
    expect(docstringViolations(fixture, QUERY, 'sql', 'SELECTED 1').map((v) => v.message)).toEqual([
      MESSAGE,
    ]);
    expect(docstringViolations(fixture, QUERY, 'sql', '-- c\nSELECT 1').map((v) => v.message)).toEqual([
      MESSAGE,
    ]);
  });

  it('applies the SELECT rule only to the step whose format.yml entry declares it', () => {
    const fixture = 'case11-sql-select-prefix';
    expect(docstringViolations(fixture, 'Given in PostgreSQL:', 'sql', 'DELETE FROM t')).toEqual([]);
    expect(docstringViolations(fixture, QUERY, 'sql', 'DELETE FROM t').map((v) => v.message)).toEqual([
      MESSAGE,
    ]);
  });

  it('rejects an empty SQL docstring', () => {
    const v = docstringViolations('case11-sql-select-prefix', 'Given in PostgreSQL:', 'sql', '   ');
    expect(v.map((x) => x.message)).toEqual(['SQL docstring must not be empty']);
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
  it('keeps a mid-line """ in the body and closes on a line that starts with """', () => {
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

describe('24. unused registry steps', () => {
  const formatBlock = (body: string, count: number): string =>
    `\n${BAR}\n  ${count === 0 ? 'PASS' : 'FAIL'}  format.yml  (${count} violation(s))\n${BAR}\n${body}`;

  it('reports each registry step none of the given files uses, at its id line', () => {
    const r = strict('case24-unused-steps', ['validate', 'a.feature', 'b.feature']);
    expect(r.exitCode).toBe(1);
    expect(r.stderr).toBe('');
    expect(r.stdout).toBe(
      `\n${BAR}\n  PASS  a.feature  (0 violation(s))\n${BAR}\n` +
        `\n${BAR}\n  PASS  b.feature  (0 violation(s))\n${BAR}\n` +
        formatBlock(
          `  line   19  step 'reset_clock' is used by none of the 2 feature file(s) validated against it\n` +
            `           → ^the clock is reset$\n`,
          1,
        ) +
        `\n`,
    );
  });

  it('pools the files: a step one file uses is used', () => {
    const r = strict('case24-unused-steps', ['a.feature']);
    expect(r.stdout).toContain("step 'response_status' is used by none of the 1 feature file(s)");
    expect(r.stdout).not.toContain("'run_migration'");
  });

  it('does not count a step written under a keyword it does not allow', () => {
    const r = strict('case24-unused-steps', ['a.feature', 'b.feature', 'wrong.feature']);
    expect(r.exitCode).toBe(1);
    expect(r.stdout).toContain("keyword 'Then' is not allowed here");
    expect(r.stdout).toContain("step 'reset_clock' is used by none of the 3 feature file(s)");
  });

  it('prints a PASS block for the registry when every step is used', () => {
    const r = strict('case24-unused-steps', ['a.feature', 'b.feature', 'c.feature']);
    expect(r.exitCode).toBe(0);
    expect(r.stdout).toContain(formatBlock('', 0));
  });

  it('is turned off by --allow-unused-steps, for validating a subset', () => {
    const r = strict('case24-unused-steps', ['--allow-unused-steps', 'a.feature']);
    expect(r.exitCode).toBe(0);
    expect(r.stdout).not.toContain('format.yml');
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
    const r = strict('case18-pass', ['x.feature', '--format']);
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
  const feature = (lines: string[]): string =>
    ['Feature: f', '', '  Scenario: s', ...lines, ''].join('\n');

  it('never enforces content_pattern, schema or special_values', () => {
    const registry = registryOf('case07-ctx-var');

    // content_pattern of postgresql_query_returns is `^(?!\s*$)\s*SELECT\s[\s\S]+$`;
    // `SELECT 1` has no whitespace after SELECT... and passes, because the field
    // is documentation. `must_match` is what enforces, and it is satisfied.
    expect(
      validate(
        'f.feature',
        feature(['    Then in PostgreSQL query returns 1 rows:', '      """sql', '      SELECT 1', '      """']),
        registry,
      ),
    ).toEqual([]);

    // response_body_contains has a content_pattern that rejects blank content
    // and a special_values token; neither is looked at. A value that the token
    // does not describe passes, and so does `{}`.
    expect(
      validate(
        'f.feature',
        feature(['    Then response body contains:', '      """json', '      {"anything": "at all"}', '      """']),
        registry,
      ),
    ).toEqual([]);

    // http_request declares a `schema` requiring nothing; a key the schema does
    // not mention is refused only because allowed_top_level_keys says so.
    const v = validate(
      'f.feature',
      feature(['    When GET /api/v1/items:', '      """json', '      {"query": {}}', '      """']),
      registry,
    );
    expect(v.map((x) => x.message)).toEqual([
      "http_request docstring has unexpected top-level keys: {'query'}",
    ]);
  });

  it('is never read at load time, so an unparseable content_pattern is harmless', () => {
    const steps = loadSteps(
      [
        'steps:',
        '  - id: a',
        '    keywords: [Given]',
        '    pattern: a',
        '    docstring:',
        '      type: sql',
        "      content_pattern: '('",
        '      schema: {x: 1}',
        '      special_values: [{token: t}]',
      ].join('\n'),
      'format.yml',
    );
    expect(steps[0]?.docstring?.mustMatch).toBeNull();
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

describe('case 23 — step_ref: a docstring that names steps of the same registry', () => {
  const case23 = (file: string): Violation[] => violationsOf('case23-step-ref', file);

  it('accepts an element naming an action of this registry', () => {
    expect(case23('x.feature')).toEqual([]);
  });

  it('refuses a sentence no step matches', () => {
    expect(case23('unknown.feature')).toEqual([
      {
        file: 'unknown.feature',
        line: 4,
        step: 'these things happen at one instant:',
        message: `'step' names "the worker sprints", which is not a step in this registry`,
      },
    ]);
  });

  it('refuses an assertion: only an action can take part in a race', () => {
    expect(case23('assertion.feature')[0]?.message).toBe(
      `'step' names step 'response_status', which is not declared with ['When'] — ` +
        'only such steps may be named here',
    );
  });

  it('refuses a step that carries a docstring of its own', () => {
    expect(case23('carries-a-body.feature')[0]?.message).toBe(
      `'step' names step 'customer_pays', which takes a docstring of its own — ` +
        'a step named here carries no body, so it cannot be one that needs one',
    );
  });

  it('refuses the combinator naming itself, for the same reason', () => {
    // It has a docstring, so the docstring rule excludes it without a rule of
    // its own — recursion is impossible by construction, not by prohibition.
    expect(case23('names-itself.feature')[0]?.message).toContain(
      `names step 'things_at_one_instant', which takes a docstring of its own`,
    );
  });

  it('leaves a request body alone, even one carrying a field of that name', () => {
    expect(case23('not-a-name.feature')).toEqual([]);
    expect(case23('inside-an-envelope.feature')).toEqual([]);
  });

  it('is inert for a registry that declares no step_ref', () => {
    const registry = loadRegistry(
      readFileSync(path.join(dir('case13-http-empty-object'), 'format.yml'), 'utf8'),
      'format.yml',
    );
    expect(registry.steps.every((s) => s.stepRef === null)).toBe(true);
  });

  it('refuses a step_ref on a step with no JSON docstring to name anything in', () => {
    const bad = [
      'steps:',
      '  - id: nowhere',
      '    keywords: [When]',
      '    pattern: "^nowhere$"',
      '    docstring: ~',
      '    step_ref: { key: step }',
      '    description: x',
    ].join('\n');
    expect(() => loadSteps(bad, 'format.yml')).toThrow(/requires a JSON docstring/);
  });
});
