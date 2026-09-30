/**
 * Every rule below is decided by the format.yml written into a temporary
 * directory. Each test validates the SAME feature under two or more registries
 * and shows the outcome change with nothing but the registry: the validator is
 * never recompiled, reconfigured or told a step id. That is the point of the
 * test — a rule that lives in code cannot be moved by editing a YAML file.
 */

import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import * as path from 'node:path';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';

import { validate, type Violation } from '../src/checks.js';
import { loadRegistry } from '../src/registry.js';

let tmp: string;
beforeEach(() => {
  tmp = mkdtempSync(path.join(tmpdir(), 'realspec-data-driven-'));
});
afterEach(() => {
  rmSync(tmp, { recursive: true, force: true });
});

/** Write both files to disk and validate them the way the CLI would, minus discovery. */
function check(format: string, feature: string): Violation[] {
  writeFileSync(path.join(tmp, 'format.yml'), format);
  writeFileSync(path.join(tmp, 'x.feature'), feature);
  const registry = loadRegistry(readFileSync(path.join(tmp, 'format.yml'), 'utf8'), 'format.yml');
  return validate('x.feature', readFileSync(path.join(tmp, 'x.feature'), 'utf8'), registry);
}

const messages = (violations: readonly Violation[]): string[] => violations.map((v) => v.message);

// ── SQL steps ───────────────────────────────────────────────────────────────

const SELECT_ONLY = 'SQL must begin with SELECT';
const MUST_MATCH = `      must_match: '^\\s*[Ss][Ee][Ll][Ee][Cc][Tt]\\b'\n      must_match_message: ${SELECT_ONLY}\n`;

const SQL_FEATURE = [
  'Feature: f',
  '',
  '  Scenario: s',
  '    Then the database says:',
  '      """sql',
  '      DELETE FROM users',
  '      """',
  '',
].join('\n');

/** One SQL step called `id`, with `docstringExtras` appended to its docstring block. */
function sqlStep(id: string, docstringExtras: string): string {
  return [
    'steps:',
    `  - id: ${id}`,
    '    keywords: [Then]',
    "    pattern: 'the database says:'",
    '    docstring:',
    '      type: sql',
    '',
  ].join('\n') + docstringExtras;
}

/** A documentation-only docstring key: present, so the block is well formed, and enforcing nothing. */
const NO_RULE = "      content_pattern: '.*'";

describe('must_match', () => {
  it('1. follows the rule from one step to another when format.yml moves it', () => {
    const twoSteps = (onFirst: string, onSecond: string): string =>
      [
        'steps:',
        '  - id: first',
        '    keywords: [Then]',
        "    pattern: 'first says:'",
        '    docstring:',
        '      type: sql',
        onFirst,
        '  - id: second',
        '    keywords: [Then]',
        "    pattern: 'second says:'",
        '    docstring:',
        '      type: sql',
        onSecond,
        '',
      ].join('\n');
    const block = (heading: string, body: string): string[] => [
      `    Then ${heading}`,
      '      """sql',
      `      ${body}`,
      '      """',
    ];
    const feature = [
      'Feature: f',
      '',
      '  Scenario: s',
      ...block('first says:', 'DELETE FROM a'),
      ...block('second says:', 'DELETE FROM b'),
      '',
    ].join('\n');

    const onFirst = check(twoSteps(MUST_MATCH.trimEnd(), NO_RULE), feature);
    expect(onFirst.map((v) => [v.line, v.message])).toEqual([[4, SELECT_ONLY]]);

    const onSecond = check(twoSteps(NO_RULE, MUST_MATCH.trimEnd()), feature);
    expect(onSecond.map((v) => [v.line, v.message])).toEqual([[8, SELECT_ONLY]]);
  });

  it('2. gives a step that exists only in format.yml the same SELECT rule', () => {
    const format = [
      'steps:',
      '  - id: fake_query',
      '    keywords: [Then]',
      "    pattern: 'in FakeDB query returns ([0-9]+) rows:'",
      '    docstring:',
      '      type: sql',
      "      must_match: '^\\s*[Ss][Ee][Ll][Ee][Cc][Tt]\\b'",
      '      must_match_message: FakeDB assertions must be SELECT',
      '',
    ].join('\n');
    const feature = [
      'Feature: f',
      '',
      '  Scenario: s',
      '    Then in FakeDB query returns 1 rows:',
      '      """sql',
      '      DELETE FROM users',
      '      """',
      '    Then in FakeDB query returns 2 rows:',
      '      """sql',
      '      select count(*) from users',
      '      """',
      '',
    ].join('\n');
    const v = check(format, feature);
    expect(v.map((x) => [x.line, x.message])).toEqual([[4, 'FakeDB assertions must be SELECT']]);
  });

  it('3. gives a step named like the old hard-coded query steps no special treatment', () => {
    expect(check(sqlStep('postgresql_query_returns', ''), SQL_FEATURE)).toEqual([]);
    expect(check(sqlStep('clickhouse_query_returns', ''), SQL_FEATURE)).toEqual([]);
    // ...and the same id with the rule declared is refused, so it was the rule all along.
    expect(messages(check(sqlStep('postgresql_query_returns', MUST_MATCH), SQL_FEATURE))).toEqual([
      SELECT_ONLY,
    ]);
  });

  it('4. reports the default message when format.yml gives none', () => {
    const format = sqlStep('q', "      must_match: '^SELECT'\n");
    expect(messages(check(format, SQL_FEATURE))).toEqual([
      'docstring does not match required pattern',
    ]);
  });

  it('5. is tested against the content after substitution', () => {
    const feature = SQL_FEATURE.replace('DELETE FROM users', 'PICK 1');
    const bare = sqlStep('q', "      must_match: '^SELECT'\n");
    expect(check(bare, feature)).toHaveLength(1);

    const withSubstitution = `substitutions:\n  - {in: sql, match: '^PICK', with: 'SELECT'}\n${bare}`;
    expect(check(withSubstitution, feature)).toEqual([]);
  });
});

// ── JSON steps ──────────────────────────────────────────────────────────────

function jsonStep(id: string, docstringExtras: string): string {
  return [
    'steps:',
    `  - id: ${id}`,
    '    keywords: [When]',
    "    pattern: 'send:'",
    '    docstring:',
    '      type: json',
    '',
  ].join('\n') + docstringExtras;
}

function jsonFeature(body: string): string {
  return ['Feature: f', '', '  Scenario: s', '    When send:', '      """json', `      ${body}`, '      """', ''].join(
    '\n',
  );
}

describe('allowed_top_level_keys', () => {
  it('6. restricts only when declared', () => {
    const feature = jsonFeature('{"headers": {}, "query": {}}');
    const v = check(jsonStep('send', '      allowed_top_level_keys: [headers, body]\n'), feature);
    expect(v.map((x) => [x.line, x.message])).toEqual([
      [4, "send docstring has unexpected top-level keys: {'query'}"],
    ]);
  });

  it('7. passes once the key is added to the list', () => {
    const feature = jsonFeature('{"headers": {}, "query": {}}');
    expect(check(jsonStep('send', '      allowed_top_level_keys: [headers, body]\n'), feature)).toHaveLength(1);
    expect(
      check(jsonStep('send', '      allowed_top_level_keys: [headers, body, query]\n'), feature),
    ).toEqual([]);
  });

  it('8. does not restrict at all when undeclared, whatever the step is called', () => {
    const feature = jsonFeature('{"anything": 1, "else": 2}');
    expect(check(jsonStep('http_request', ''), feature)).toEqual([]);
  });

  it('9. accepts only {} when the list is empty', () => {
    const format = jsonStep('send', '      allowed_top_level_keys: []\n');
    expect(check(format, jsonFeature('{}'))).toEqual([]);
    expect(messages(check(format, jsonFeature('{"a": 1}')))).toEqual([
      "send docstring has unexpected top-level keys: {'a'}",
    ]);
  });

  it('10. rejects an array docstring only when keys are declared', () => {
    const feature = jsonFeature('[1, 2]');
    expect(check(jsonStep('send', ''), feature)).toEqual([]);
    expect(messages(check(jsonStep('send', '      allowed_top_level_keys: [a]\n'), feature))).toEqual([
      'send docstring must be a JSON object',
    ]);
  });
});

describe('substitutions', () => {
  const UUID = "substitutions:\n  - {in: json, match: '<any-uuid>', with: '\"__uuid__\"'}\n";

  it('11. flip a rejected value when added and restore the rejection when removed', () => {
    const feature = jsonFeature('{"id": <any-uuid>, "again": <any-uuid>}');
    const step = jsonStep('send', '');

    const before = check(step, feature);
    expect(before).toHaveLength(1);
    expect(before[0]?.message.startsWith('docstring is not valid JSON after substitution: ')).toBe(true);

    // Both occurrences are rewritten: one left over would still fail to parse.
    expect(check(UUID + step, feature)).toEqual([]);

    expect(check(step, feature)).toHaveLength(1);
  });

  it('12. apply in the order they are listed', () => {
    const feature = jsonFeature('{"id": AAA}');
    const step = jsonStep('send', '');
    const forward =
      "substitutions:\n  - {in: json, match: 'AAA', with: 'BBB'}\n  - {in: json, match: 'BBB', with: '\"ok\"'}\n";
    const backward =
      "substitutions:\n  - {in: json, match: 'BBB', with: '\"ok\"'}\n  - {in: json, match: 'AAA', with: 'BBB'}\n";
    expect(check(forward + step, feature)).toEqual([]);
    expect(check(backward + step, feature)).toHaveLength(1);
  });

  it('13. apply only to the docstring type they name', () => {
    const feature = jsonFeature('{"id": <any-uuid>}');
    const sqlOnly = "substitutions:\n  - {in: sql, match: '<any-uuid>', with: '\"x\"'}\n";
    expect(check(sqlOnly + jsonStep('send', ''), feature)).toHaveLength(1);
    expect(check(UUID + jsonStep('send', ''), feature)).toEqual([]);
  });

  it('14. insert their replacement as literal text', () => {
    // As a group reference `$1` would expand to `X` and `$&` to the match; the
    // must_match below only holds if both stay exactly as written.
    const format =
      "substitutions:\n  - {in: json, match: '(X)', with: '\"$1$&\"'}\n" +
      jsonStep('send', "      must_match: '\\$1\\$&'\n");
    expect(check(format, jsonFeature('{"id": X}'))).toEqual([]);

    const expanded = format.replace("'\\$1\\$&'", "'XX'");
    expect(messages(check(expanded, jsonFeature('{"id": X}')))).toEqual([
      'docstring does not match required pattern',
    ]);
  });

  it('15. leave step_ref working: names are resolved on the substituted docstring', () => {
    const registry = (substitution: string): string =>
      [
        'substitutions:',
        `  - {in: json, match: '@@WORKER@@', with: '${substitution}'}`,
        'steps:',
        '  - id: customer_pays',
        '    keywords: [When]',
        "    pattern: 'the customer pays'",
        '    docstring: ~',
        '  - id: things_at_one_instant',
        '    keywords: [When]',
        "    pattern: 'these happen at one instant:'",
        '    docstring:',
        '      type: json',
        '    step_ref: {key: step, keywords: [When], docstring: none}',
        '',
      ].join('\n');
    const feature = [
      'Feature: f',
      '',
      '  Scenario: s',
      '    When these happen at one instant:',
      '      """json',
      '      [{"step": "the customer pays"}, {"step": @@WORKER@@}]',
      '      """',
      '',
    ].join('\n');

    // Unsubstituted the docstring is not JSON at all; substituted to a real
    // step name it parses and its names resolve...
    expect(check(registry('"the customer pays"'), feature)).toEqual([]);
    // ...and substituted to a sentence no step matches, step_ref refuses it.
    expect(messages(check(registry('"the worker sprints"'), feature))).toEqual([
      `'step' names "the worker sprints", which is not a step in this registry`,
    ]);
  });
});
