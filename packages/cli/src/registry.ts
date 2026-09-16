/**
 * registry.ts — format.yml loader.
 *
 * Port of `load_steps()` / `_anchor()` in the original
 * `validate.py`.
 *
 * Note that `content_pattern`, `schema` and `special_values` in format.yml are
 * documentation only: validate.py never enforces them, and neither does this
 * port. Adding enforcement here would break parity.
 *
 * `context_variables` and `produces` are RealSpec extensions that validate.py
 * has no equivalent for; both are absent from the original project's registry, so the check
 * they drive stays inert there.
 */

import { parse as parseYaml } from 'yaml';
import { pyCollapseWhitespace } from './parse.js';

/** The `docstring:` block of a step definition. `null` means "not allowed". */
export interface DocstringSpec {
  /** `'sql'` | `'json'`, or `undefined` when the key is absent. */
  readonly type: string | undefined;
}

/** One compiled entry of the step registry. */
export interface StepDef {
  readonly id: string;
  readonly keywords: readonly string[];
  /** The anchored, whitespace-normalised pattern source. */
  readonly pattern: string;
  readonly docstring: DocstringSpec | null;
  /**
   * Sticky regex reproducing Python's `re.match()` (anchored at index 0).
   * Always reset `lastIndex` to 0 before use — see {@link matchesStepText}.
   */
  readonly regex: RegExp;
  /**
   * 1-based capture group holding the context variable this step saves,
   * resolved from `produces`; `null` when the step saves nothing.
   */
  readonly producesGroup: number | null;
}

/** A loaded format.yml: the step registry plus its context-variable rules. */
export interface Registry {
  readonly steps: StepDef[];
  /**
   * Names declared under `context_variables.provided`, or `null` when the
   * registry declares no `context_variables` at all.
   */
  readonly providedVars: ReadonlySet<string> | null;
}

/** Raised for any user-facing problem with a format.yml file. */
export class RegistryError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'RegistryError';
  }
}

/** Ensure the pattern is fully anchored with `^` and `$`. */
export function anchor(pattern: string): string {
  let p = pattern;
  if (!p.startsWith('^')) p = '^' + p;
  if (!p.endsWith('$')) p = p + '$';
  return p;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

/** Number of capture groups in `pattern`, via the trailing empty alternative. */
function countGroups(pattern: string): number {
  const probe = new RegExp(`${pattern}|`).exec('');
  return probe === null ? 0 : probe.length - 1;
}

/** Capture names in pattern order; only read for steps that declare `produces`. */
function captureNames(node: unknown, where: string): string[] {
  if (!Array.isArray(node)) {
    throw new RegistryError(`${where}: 'produces' requires a 'captures' list`);
  }
  return node.map((capture: unknown, i) => {
    if (!isRecord(capture) || typeof capture['name'] !== 'string') {
      throw new RegistryError(`${where}: captures[${i}] is missing a string 'name'`);
    }
    return capture['name'];
  });
}

/** Read `context_variables.provided`; `null` when the key is absent. */
function loadProvidedVars(raw: Record<string, unknown>, origin: string): ReadonlySet<string> | null {
  const node = raw['context_variables'];
  if (node === undefined || node === null) return null;
  if (!isRecord(node)) {
    throw new RegistryError(`${origin}: 'context_variables' must be a mapping`);
  }
  const providedNode = node['provided'];
  if (providedNode === undefined || providedNode === null) return new Set<string>();
  if (!isRecord(providedNode)) {
    throw new RegistryError(
      `${origin}: 'context_variables.provided' must be a mapping of name to description`,
    );
  }
  return new Set(Object.keys(providedNode));
}

/** Parse a format.yml source and return its compiled step definitions. */
export function loadSteps(source: string, origin: string): StepDef[] {
  return loadRegistry(source, origin).steps;
}

/**
 * Parse a format.yml source.
 *
 * @param source   raw YAML text
 * @param origin   path shown in error messages
 */
export function loadRegistry(source: string, origin: string): Registry {
  let raw: unknown;
  try {
    raw = parseYaml(source);
  } catch (err) {
    const detail = err instanceof Error ? err.message : String(err);
    throw new RegistryError(`${origin} is not valid YAML: ${detail}`);
  }

  if (raw === null || raw === undefined) {
    // yaml.safe_load('') is None; raw.get('steps', []) would then explode in
    // Python, so report it rather than pretending the registry is empty.
    throw new RegistryError(`${origin} is empty`);
  }
  if (!isRecord(raw)) {
    throw new RegistryError(`${origin} must contain a YAML mapping at the top level`);
  }

  const providedVars = loadProvidedVars(raw, origin);

  const stepsNode = raw['steps'];
  if (stepsNode === undefined || stepsNode === null) return { steps: [], providedVars };
  if (!Array.isArray(stepsNode)) {
    throw new RegistryError(`${origin}: 'steps' must be a list`);
  }

  const compiled: StepDef[] = [];
  for (let index = 0; index < stepsNode.length; index += 1) {
    const entry: unknown = stepsNode[index];
    const where = `${origin}: steps[${index}]`;
    if (!isRecord(entry)) {
      throw new RegistryError(`${where} must be a mapping`);
    }

    const patternNode = entry['pattern'];
    if (typeof patternNode !== 'string') {
      throw new RegistryError(`${where} is missing a string 'pattern'`);
    }

    const idNode = entry['id'];
    if (typeof idNode !== 'string') {
      throw new RegistryError(`${where} is missing a string 'id'`);
    }

    const keywordsNode = entry['keywords'];
    const keywords: string[] = [];
    if (keywordsNode !== undefined && keywordsNode !== null) {
      if (!Array.isArray(keywordsNode)) {
        throw new RegistryError(`${where}: 'keywords' must be a list of strings`);
      }
      for (const keyword of keywordsNode) {
        if (typeof keyword !== 'string') {
          throw new RegistryError(`${where}: 'keywords' must be a list of strings`);
        }
        keywords.push(keyword);
      }
    }

    const docstringNode = entry['docstring'];
    let docstring: DocstringSpec | null = null;
    if (docstringNode !== undefined && docstringNode !== null) {
      if (!isRecord(docstringNode)) {
        throw new RegistryError(`${where}: 'docstring' must be a mapping or null`);
      }
      const typeNode = docstringNode['type'];
      if (typeNode !== undefined && typeNode !== null && typeof typeNode !== 'string') {
        throw new RegistryError(`${where}: 'docstring.type' must be a string`);
      }
      docstring = { type: typeof typeNode === 'string' ? typeNode : undefined };
    }

    // Multi-line YAML block scalars (> or >-) fold newlines into spaces.
    // Normalise any remaining whitespace runs to a single space, then anchor.
    const pattern = anchor(pyCollapseWhitespace(patternNode));

    let regex: RegExp;
    try {
      regex = new RegExp(pattern, 'y');
    } catch (err) {
      const detail = err instanceof Error ? err.message : String(err);
      throw new RegistryError(`${where}: invalid pattern ${JSON.stringify(pattern)}: ${detail}`);
    }

    const producesNode = entry['produces'];
    let producesGroup: number | null = null;
    if (producesNode !== undefined && producesNode !== null) {
      if (typeof producesNode !== 'string') {
        throw new RegistryError(`${where}: 'produces' must be a string`);
      }
      const names = captureNames(entry['captures'], where);
      if (names.length !== countGroups(pattern)) {
        throw new RegistryError(
          `${where}: 'captures' lists ${names.length} name(s) but the pattern has ` +
            `${countGroups(pattern)} capture group(s)`,
        );
      }
      const index = names.indexOf(producesNode);
      if (index === -1) {
        throw new RegistryError(
          `${where}: 'produces' names ${JSON.stringify(producesNode)}, which is not one of its captures`,
        );
      }
      producesGroup = index + 1;
    }

    compiled.push({ id: idNode, keywords, pattern, docstring, regex, producesGroup });
  }

  return { steps: compiled, providedVars };
}

/** Python `re.match()` semantics: anchored at index 0, not a full match. */
export function matchesStepText(def: StepDef, text: string): boolean {
  def.regex.lastIndex = 0;
  return def.regex.test(text);
}
