/**
 * Strict loading of format.yml: every rule that could be declared but never take
 * effect must stop the run at load time, with a message that names the exact
 * place in the file. Each test below asserts the complete message.
 *
 * The messages that quote the regex engine's own complaint (an unbalanced
 * bracket, a stray quantifier) build that fragment from the same engine at run
 * time: its wording belongs to V8 and moves between releases, while everything
 * around it is this loader's and is asserted literally.
 */

import { describe, expect, it } from 'vitest';

import {
  applySubstitution,
  loadRegistry,
  RegistryError,
  usesOpenApi,
  type Registry,
} from '../src/registry.js';

/** The load error for `yaml`, failing the test when it loads. */
function loadError(yaml: string): string {
  try {
    loadRegistry(yaml, 'f.yml');
  } catch (err) {
    expect(err).toBeInstanceOf(RegistryError);
    return (err as RegistryError).message;
  }
  throw new Error('expected the registry to be rejected, but it loaded');
}

function load(yaml: string): Registry {
  return loadRegistry(yaml, 'f.yml');
}

/** What V8 says about `source` as a regex with `flags`, for the messages that quote it. */
function engineComplaint(source: string, flags: string): string {
  try {
    new RegExp(source, flags);
  } catch (err) {
    return (err as Error).message;
  }
  throw new Error(`${JSON.stringify(source)} is a valid regex`);
}

/** A one-step registry; `extra` lines (already indented by four spaces) follow the pattern. */
function step(pattern: string, extra: string): string {
  return `steps:\n  - id: a\n    pattern: ${pattern}\n${extra}`;
}

describe('docstring.type', () => {
  it('1. rejects a type that is neither sql nor json, naming the step index', () => {
    expect(loadError(step('a', '    docstring: {type: xml}\n'))).toBe(
      "f.yml: steps[0]: unknown docstring type 'xml' (expected sql or json)",
    );
    expect(
      loadError('steps:\n  - {id: a, pattern: a}\n  - {id: b, pattern: b, docstring: {type: yaml}}\n'),
    ).toBe("f.yml: steps[1]: unknown docstring type 'yaml' (expected sql or json)");
    expect(loadError(step('a', "    docstring: {type: ''}\n"))).toBe(
      "f.yml: steps[0]: unknown docstring type '' (expected sql or json)",
    );
  });
});

describe('substitutions', () => {
  it('2. must be a list of mappings', () => {
    expect(loadError('substitutions: 3\n')).toBe("f.yml: 'substitutions' must be a list");
    expect(loadError('substitutions: {in: sql}\n')).toBe("f.yml: 'substitutions' must be a list");
    expect(loadError('substitutions: [3]\n')).toBe('f.yml: substitutions[0] must be a mapping');
    expect(loadError('substitutions:\n  - {in: sql, match: a, with: b}\n  - [x]\n')).toBe(
      'f.yml: substitutions[1] must be a mapping',
    );
  });

  it('3. needs a string in, match and with', () => {
    expect(loadError('substitutions: [{match: a, with: b}]\n')).toBe(
      "f.yml: substitutions[0] is missing a string 'in'",
    );
    expect(loadError('substitutions: [{in: sql, with: b}]\n')).toBe(
      "f.yml: substitutions[0] is missing a string 'match'",
    );
    expect(loadError('substitutions: [{in: sql, match: a}]\n')).toBe(
      "f.yml: substitutions[0] is missing a string 'with'",
    );
    // Present but not a string is the same as missing.
    expect(loadError('substitutions: [{in: sql, match: 1, with: b}]\n')).toBe(
      "f.yml: substitutions[0] is missing a string 'match'",
    );
    expect(loadError('substitutions: [{in: sql, match: a, with: ~}]\n')).toBe(
      "f.yml: substitutions[0] is missing a string 'with'",
    );
  });

  it('4. refuses an unknown key', () => {
    expect(loadError('substitutions: [{in: sql, match: a, with: b, mach: c}]\n')).toBe(
      "f.yml: substitutions[0] has unknown key 'mach' (allowed: in, match, with)",
    );
  });

  it('5. refuses an "in" that is not a docstring type', () => {
    expect(loadError('substitutions: [{in: yaml, match: a, with: b}]\n')).toBe(
      "f.yml: substitutions[0]: unknown docstring type 'yaml' (expected sql or json)",
    );
  });

  it('6. refuses a match that does not compile', () => {
    expect(loadError('substitutions: [{in: sql, match: "[", with: b}]\n')).toBe(
      `f.yml: substitutions[0]: invalid 'match' regex "[": ${engineComplaint('[', 'g')}`,
    );
  });

  it('7. names the index of the offending item', () => {
    const ok = '  - {in: sql, match: a, with: b}\n';
    expect(loadError(`substitutions:\n${ok}  - {in: json, match: a}\n`)).toBe(
      "f.yml: substitutions[1] is missing a string 'with'",
    );
    expect(loadError(`substitutions:\n${ok}${ok}  - {in: json, match: '(', with: x}\n`)).toBe(
      `f.yml: substitutions[2]: invalid 'match' regex "(": ${engineComplaint('(', 'g')}`,
    );

    // The well-formed items around the bad one load, in order, with a global
    // regex and a literal replacement.
    const [json, sql] = load(
      [
        'substitutions:',
        "  - {in: json, match: 'a+', with: 'x'}",
        "  - {in: sql, match: '(b)', with: '$1$&$$'}",
        '',
      ].join('\n'),
    ).substitutions;
    expect(json?.in).toBe('json');
    expect(json?.match.source).toBe('a+');
    expect(applySubstitution(json!, 'a aa aaa')).toBe('x x x');
    expect(applySubstitution(sql!, 'abab')).toBe('a$1$&$$a$1$&$$');
  });
});

describe('docstring.must_match', () => {
  it('8. needs a docstring type of sql or json', () => {
    expect(loadError(step('a', '    docstring: {must_match: x}\n'))).toBe(
      "f.yml: steps[0]: 'docstring.must_match' requires 'docstring.type' to be sql or json (no type declared)",
    );
  });

  it('9. must be a string', () => {
    expect(loadError(step('a', '    docstring: {type: sql, must_match: 5}\n'))).toBe(
      "f.yml: steps[0]: 'docstring.must_match' must be a string",
    );
  });

  it('10. must compile as a native regex, so an inline (?i) is refused, not translated', () => {
    expect(loadError(step('a', "    docstring: {type: sql, must_match: '('}\n"))).toBe(
      `f.yml: steps[0]: invalid 'docstring.must_match' regex "(": ${engineComplaint('(', '')}`,
    );
    expect(loadError(step('a', "    docstring: {type: sql, must_match: '(?i)select'}\n"))).toBe(
      `f.yml: steps[0]: invalid 'docstring.must_match' regex "(?i)select": ${engineComplaint('(?i)select', '')}`,
    );

    // A valid one loads as a flag-less regex, with its message.
    const spec = load(
      step('a', "    docstring: {type: sql, must_match: '^SELECT', must_match_message: nope}\n"),
    ).steps[0]?.docstring;
    expect(spec?.mustMatch?.source).toBe('^SELECT');
    expect(spec?.mustMatch?.flags).toBe('');
    expect(spec?.mustMatchMessage).toBe('nope');
    expect(load(step('a', '    docstring: {type: sql}\n')).steps[0]?.docstring).toEqual({
      type: 'sql',
      mustMatch: null,
      mustMatchMessage: null,
      allowedTopLevelKeys: null,
    });
  });

  it('11. must_match_message needs a must_match to attach to', () => {
    expect(loadError(step('a', '    docstring: {type: sql, must_match_message: m}\n'))).toBe(
      "f.yml: steps[0]: 'docstring.must_match_message' requires 'docstring.must_match'",
    );
    expect(loadError(step('a', '    docstring: {must_match_message: m}\n'))).toBe(
      "f.yml: steps[0]: 'docstring.must_match_message' requires 'docstring.type' to be sql or json (no type declared)",
    );
  });

  it('12. must_match_message must be a string', () => {
    expect(
      loadError(step('a', '    docstring: {type: sql, must_match: x, must_match_message: 3}\n')),
    ).toBe("f.yml: steps[0]: 'docstring.must_match_message' must be a string");
  });
});

describe('docstring.allowed_top_level_keys', () => {
  it('13. only makes sense for a json docstring', () => {
    expect(loadError(step('a', '    docstring: {type: sql, allowed_top_level_keys: [a]}\n'))).toBe(
      "f.yml: steps[0]: 'docstring.allowed_top_level_keys' requires 'docstring.type' to be json (found 'sql')",
    );
    expect(loadError(step('a', '    docstring: {allowed_top_level_keys: [a]}\n'))).toBe(
      "f.yml: steps[0]: 'docstring.allowed_top_level_keys' requires 'docstring.type' to be json (no type declared)",
    );
  });

  it('14. must be a list of strings', () => {
    const message = "f.yml: steps[0]: 'docstring.allowed_top_level_keys' must be a list of strings";
    expect(loadError(step('a', '    docstring: {type: json, allowed_top_level_keys: a}\n'))).toBe(message);
    expect(loadError(step('a', '    docstring: {type: json, allowed_top_level_keys: [a, 1]}\n'))).toBe(
      message,
    );
  });

  it('15. loads, and an empty list is a real (empty) restriction', () => {
    const keys = (extra: string): readonly string[] | null | undefined =>
      load(step('a', `    docstring: {type: json${extra}}\n`)).steps[0]?.docstring?.allowedTopLevelKeys;
    expect(keys(', allowed_top_level_keys: [headers, body]')).toEqual(['headers', 'body']);
    expect(keys(', allowed_top_level_keys: []')).toEqual([]);
    expect(keys('')).toBeNull();
  });
});

describe('openapi binding', () => {
  const captures = '    captures: [{name: m}, {name: p}]\n';
  const pair = "'(a) (b)'";

  it('16. must be a mapping with only the keys method and path', () => {
    expect(loadError(step(pair, `${captures}    openapi: GET\n`))).toBe(
      "f.yml: steps[0]: 'openapi' must be a mapping",
    );
    expect(loadError(step(pair, `${captures}    openapi: [m, p]\n`))).toBe(
      "f.yml: steps[0]: 'openapi' must be a mapping",
    );
    expect(loadError(step(pair, `${captures}    openapi: {method: m, path: p, verb: m}\n`))).toBe(
      "f.yml: steps[0]: 'openapi' has unknown key 'verb' (allowed: method, path)",
    );
  });

  it('17. needs both a method and a path, as strings', () => {
    expect(loadError(step(pair, `${captures}    openapi: {method: m}\n`))).toBe(
      "f.yml: steps[0]: 'openapi' is missing a string 'path'",
    );
    expect(loadError(step(pair, `${captures}    openapi: {path: p}\n`))).toBe(
      "f.yml: steps[0]: 'openapi' is missing a string 'method'",
    );
    expect(loadError(step(pair, `${captures}    openapi: {method: 1, path: p}\n`))).toBe(
      "f.yml: steps[0]: 'openapi' is missing a string 'method'",
    );
  });

  it('18. must name captures that exist', () => {
    expect(loadError(step(pair, `${captures}    openapi: {method: m, path: q}\n`))).toBe(
      "f.yml: steps[0] openapi.path refers to unknown capture 'q'",
    );
    expect(loadError(step(pair, `${captures}    openapi: {method: x, path: p}\n`))).toBe(
      "f.yml: steps[0] openapi.method refers to unknown capture 'x'",
    );
  });

  it('19. requires a captures list, as produces does', () => {
    expect(loadError(step(pair, '    openapi: {method: m, path: p}\n'))).toBe(
      "f.yml: steps[0]: 'openapi' requires a 'captures' list",
    );
    expect(loadError(step("'(a)'", '    produces: x\n'))).toBe(
      "f.yml: steps[0]: 'produces' requires a 'captures' list",
    );
  });

  it('20. needs as many capture names as the pattern has groups', () => {
    expect(loadError(step("'(a)'", `${captures}    openapi: {method: m, path: p}\n`))).toBe(
      "f.yml: steps[0]: 'captures' lists 2 name(s) but the pattern has 1 capture group(s)",
    );
    expect(loadError(step("'(a)(b)(c)'", `${captures}    openapi: {method: m, path: p}\n`))).toBe(
      "f.yml: steps[0]: 'captures' lists 2 name(s) but the pattern has 3 capture group(s)",
    );
  });

  it('21. resolves names to groups even when the path comes first', () => {
    const swapped = load(
      step(
        "'(/[a-z/]+) via (GET|POST)'",
        '    captures: [{name: target}, {name: verb}]\n    openapi: {method: verb, path: target}\n',
      ),
    ).steps[0];
    expect(swapped?.openapi).toEqual({
      methodCapture: 'verb',
      methodGroup: 2,
      pathCapture: 'target',
      pathGroup: 1,
    });
    expect(swapped?.captures).toEqual(['target', 'verb']);

    const inOrder = load(
      step(
        "'(GET|POST) (/[a-z/]+)'",
        '    captures: [{name: verb}, {name: target}]\n    openapi: {method: verb, path: target}\n',
      ),
    ).steps[0];
    expect(inOrder?.openapi).toEqual({
      methodCapture: 'verb',
      methodGroup: 1,
      pathCapture: 'target',
      pathGroup: 2,
    });
  });

  it('22. is not "used" until some step carries a binding', () => {
    expect(usesOpenApi(load(step('a', '')))).toBe(false);
    expect(usesOpenApi(load('steps: []\n'))).toBe(false);
    // `produces` needs captures too, but is not an openapi binding.
    expect(usesOpenApi(load(step("'(a)'", '    produces: x\n    captures: [{name: x}]\n')))).toBe(false);
    expect(
      usesOpenApi(load(step(pair, `${captures}    openapi: {method: m, path: p}\n`))),
    ).toBe(true);
  });
});

describe('duplicate keys', () => {
  it('23. are invalid YAML, not a silent last-one-wins', () => {
    const top = loadError('steps: []\nsteps: []\n');
    expect(top.startsWith('f.yml is not valid YAML: ')).toBe(true);
    expect(top).toContain('Map keys must be unique');

    const nested = loadError(
      'steps:\n  - id: a\n    pattern: a\n    docstring: {type: sql}\n    docstring: {type: json}\n',
    );
    expect(nested.startsWith('f.yml is not valid YAML: ')).toBe(true);
    expect(nested).toContain('Map keys must be unique');
  });
});
