/**
 * registry.ts — format.yml loader.
 *
 * The registry is where every rule the validator enforces is declared. The
 * validator itself knows no step id, path prefix or special value: it applies
 * a handful of mechanisms (step patterns, docstring types, substitutions,
 * `must_match`, `allowed_top_level_keys`, `openapi` bindings, `step_ref`,
 * context variables), and format.yml decides what they are applied to.
 *
 * Loading is strict. A rule that is declared but could never take effect — an
 * unknown docstring type, a regex that does not compile, a binding to a capture
 * that does not exist, a docstring rule the docstring type cannot honour — is a
 * {@link RegistryError}, so the run stops instead of silently enforcing less
 * than the file promises. Duplicate YAML keys are rejected for the same reason.
 *
 * `content_pattern`, `schema` and `special_values` are documentation only: they
 * are never read, so they can never disagree with what is enforced.
 *
 * Every regex a registry declares is a native JavaScript RegExp, compiled
 * without flags. There is deliberately no inline-flag syntax such as `(?i)` and
 * no separate flags field; a case-insensitive rule is written with character
 * classes (`[Ss][Ee][Ll][Ee][Cc][Tt]`).
 */

import { parse as parseYaml } from 'yaml';
import { pyCollapseWhitespace } from './parse.js';

/** Docstring types the validator knows how to check. */
const DOCSTRING_TYPES: readonly string[] = ['sql', 'json'];

/** Message used when `must_match` fails and no `must_match_message` is declared. */
export const DEFAULT_MUST_MATCH_MESSAGE = 'docstring does not match required pattern';

/** The `docstring:` block of a step definition. `null` means "not allowed". */
export interface DocstringSpec {
  /** `'sql'` | `'json'`, or `undefined` when the key is absent. */
  readonly type: string | undefined;
  /**
   * Pattern the substituted content must CONTAIN (find semantics, not anchored),
   * compiled without flags; `null` when the content is not constrained. Never
   * global or sticky, so `.test()` carries no state between calls.
   */
  readonly mustMatch: RegExp | null;
  /** Message reported when `mustMatch` fails; `null` selects the default. */
  readonly mustMatchMessage: string | null;
  /**
   * The only top-level keys a JSON object docstring may hold, or `null` when
   * the keys are unrestricted. An empty list is a restriction: only `{}` passes.
   */
  readonly allowedTopLevelKeys: readonly string[] | null;
}

/**
 * One entry of format.yml's top-level `substitutions:` list. It rewrites the
 * docstring of every step whose docstring type is `in`, before that docstring
 * is parsed or checked, so intentionally non-literal placeholders do not trip
 * the syntax checks. Apply it with {@link applySubstitution}.
 */
export interface Substitution {
  /** The docstring type the rule applies to. */
  readonly in: 'sql' | 'json';
  /** Compiled with the `g` flag and no other: every occurrence is replaced. */
  readonly match: RegExp;
  /** Literal replacement text; `$1` and `$&` are never interpreted. */
  readonly with: string;
}

/**
 * A step's `openapi:` binding: which capture groups of the step pattern hold
 * the HTTP method and the request path. Groups are 1-based.
 */
export interface OpenApiBinding {
  readonly methodCapture: string;
  readonly methodGroup: number;
  readonly pathCapture: string;
  readonly pathGroup: number;
}

/**
 * The `step_ref:` block of a step definition — the RealSpec extension that lets
 * one step's JSON docstring NAME other steps of the same registry, so that a
 * combinator (a step that arranges several things at once) can compose the
 * registry's own vocabulary instead of a new step being added for each thing
 * that might take part. `null` when the step declares none, which is every step
 * in a registry that does not use the extension.
 */
export interface StepRefSpec {
  /** An element object whose ONLY key is this one names a step. */
  readonly key: string;
  /** The named step must be declared with every one of these keywords. */
  readonly keywords: readonly string[];
  /** `'none'` requires the named step to take no docstring of its own. */
  readonly docstring: 'none' | 'any';
}

/** One compiled entry of the step registry. */
export interface StepDef {
  readonly id: string;
  readonly keywords: readonly string[];
  /** The anchored, whitespace-normalised pattern source. */
  readonly pattern: string;
  readonly docstring: DocstringSpec | null;
  /**
   * Sticky regex, anchored at index 0 by the `^` every pattern carries.
   * Always reset `lastIndex` to 0 before use — see {@link matchesStepText}.
   */
  readonly regex: RegExp;
  /**
   * Capture names in pattern order; empty unless the step declares `produces`
   * or `openapi`, the only fields that need them.
   */
  readonly captures: readonly string[];
  /**
   * 1-based capture group holding the context variable this step saves,
   * resolved from `produces`; `null` when the step saves nothing.
   */
  readonly producesGroup: number | null;
  /** `openapi:`, or `null` when the step is not checked against the API contract. */
  readonly openapi: OpenApiBinding | null;
  /** `step_ref:`, or `null` when this step's docstring names no steps. */
  readonly stepRef: StepRefSpec | null;
}

/** A loaded format.yml: the step registry plus the rules that apply across steps. */
export interface Registry {
  readonly steps: StepDef[];
  /**
   * Names declared under `context_variables.provided`, or `null` when the
   * registry declares no `context_variables` at all.
   */
  readonly providedVars: ReadonlySet<string> | null;
  /** Docstring rewrites, in the order they are applied. */
  readonly substitutions: readonly Substitution[];
}

/** Raised for any user-facing problem with a format.yml file or its openapi files. */
export class RegistryError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'RegistryError';
  }
}

/** True when at least one step carries an `openapi` binding, i.e. `../openapi` is needed. */
export function usesOpenApi(registry: Registry): boolean {
  return registry.steps.some((step) => step.openapi !== null);
}

/**
 * Apply one substitution to `content`, replacing every match.
 *
 * The replacement goes through a function so it is inserted verbatim: a `$1`,
 * `$&` or `$$` in `with` is text, never a group reference.
 */
export function applySubstitution(substitution: Substitution, content: string): string {
  return content.replace(substitution.match, () => substitution.with);
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

/** Why a user-written regex failed to compile, without the engine's stack. */
function describeRegexError(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

/** Compile a user-declared regex; `errorPrefix` names the field for the message. */
function compileRegex(source: string, flags: string, errorPrefix: string): RegExp {
  try {
    return new RegExp(source, flags);
  } catch (err) {
    throw new RegistryError(`${errorPrefix}${JSON.stringify(source)}: ${describeRegexError(err)}`);
  }
}

/** Capture names in pattern order; only read for steps that need them. */
function captureNames(node: unknown, needer: string, where: string): string[] {
  if (!Array.isArray(node)) {
    throw new RegistryError(`${where}: '${needer}' requires a 'captures' list`);
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

// ── substitutions ───────────────────────────────────────────────────────────

const SUBSTITUTION_KEYS: readonly string[] = ['in', 'match', 'with'];

function requireString(item: Record<string, unknown>, key: string, where: string): string {
  const value = item[key];
  if (typeof value !== 'string') {
    throw new RegistryError(`${where} is missing a string '${key}'`);
  }
  return value;
}

function loadSubstitutions(node: unknown, origin: string): Substitution[] {
  if (node === undefined || node === null) return [];
  if (!Array.isArray(node)) {
    throw new RegistryError(`${origin}: 'substitutions' must be a list`);
  }
  return node.map((item: unknown, i): Substitution => {
    const where = `${origin}: substitutions[${i}]`;
    if (!isRecord(item)) {
      throw new RegistryError(`${where} must be a mapping`);
    }
    for (const key of Object.keys(item)) {
      if (!SUBSTITUTION_KEYS.includes(key)) {
        throw new RegistryError(`${where} has unknown key '${key}' (allowed: in, match, with)`);
      }
    }
    const inType = requireString(item, 'in', where);
    const match = requireString(item, 'match', where);
    const replacement = requireString(item, 'with', where);
    if (inType !== 'sql' && inType !== 'json') {
      throw new RegistryError(`${where}: unknown docstring type '${inType}' (expected sql or json)`);
    }
    return {
      in: inType,
      match: compileRegex(match, 'g', `${where}: invalid 'match' regex `),
      with: replacement,
    };
  });
}

// ── docstring ───────────────────────────────────────────────────────────────

/** Fail when a docstring rule is declared for a docstring type that cannot honour it. */
function requireDocstringType(
  actual: string | undefined,
  field: string,
  expected: readonly string[],
  where: string,
): void {
  if (actual !== undefined && expected.includes(actual)) return;
  throw new RegistryError(
    `${where}: 'docstring.${field}' requires 'docstring.type' to be ${expected.join(' or ')}` +
      (actual === undefined ? ' (no type declared)' : ` (found '${actual}')`),
  );
}

function loadDocstring(node: unknown, where: string): DocstringSpec | null {
  if (node === undefined || node === null) return null;
  if (!isRecord(node)) {
    throw new RegistryError(`${where}: 'docstring' must be a mapping or null`);
  }

  const typeNode = node['type'];
  if (typeNode !== undefined && typeNode !== null && typeof typeNode !== 'string') {
    throw new RegistryError(`${where}: 'docstring.type' must be a string`);
  }
  const type = typeof typeNode === 'string' ? typeNode : undefined;
  if (type !== undefined && !DOCSTRING_TYPES.includes(type)) {
    throw new RegistryError(`${where}: unknown docstring type '${type}' (expected sql or json)`);
  }

  let mustMatch: RegExp | null = null;
  const mustMatchNode = node['must_match'];
  if (mustMatchNode !== undefined && mustMatchNode !== null) {
    requireDocstringType(type, 'must_match', DOCSTRING_TYPES, where);
    if (typeof mustMatchNode !== 'string') {
      throw new RegistryError(`${where}: 'docstring.must_match' must be a string`);
    }
    mustMatch = compileRegex(mustMatchNode, '', `${where}: invalid 'docstring.must_match' regex `);
  }

  let mustMatchMessage: string | null = null;
  const messageNode = node['must_match_message'];
  if (messageNode !== undefined && messageNode !== null) {
    requireDocstringType(type, 'must_match_message', DOCSTRING_TYPES, where);
    if (typeof messageNode !== 'string') {
      throw new RegistryError(`${where}: 'docstring.must_match_message' must be a string`);
    }
    if (mustMatch === null) {
      throw new RegistryError(
        `${where}: 'docstring.must_match_message' requires 'docstring.must_match'`,
      );
    }
    mustMatchMessage = messageNode;
  }

  let allowedTopLevelKeys: string[] | null = null;
  const keysNode = node['allowed_top_level_keys'];
  if (keysNode !== undefined && keysNode !== null) {
    requireDocstringType(type, 'allowed_top_level_keys', ['json'], where);
    if (!Array.isArray(keysNode) || keysNode.some((key: unknown) => typeof key !== 'string')) {
      throw new RegistryError(
        `${where}: 'docstring.allowed_top_level_keys' must be a list of strings`,
      );
    }
    allowedTopLevelKeys = [...(keysNode as string[])];
  }

  return { type, mustMatch, mustMatchMessage, allowedTopLevelKeys };
}

// ── openapi ─────────────────────────────────────────────────────────────────

function openApiCapture(
  binding: Record<string, unknown>,
  key: 'method' | 'path',
  captures: readonly string[],
  where: string,
): string {
  const name = binding[key];
  if (typeof name !== 'string') {
    throw new RegistryError(`${where}: 'openapi' is missing a string '${key}'`);
  }
  if (!captures.includes(name)) {
    throw new RegistryError(`${where} openapi.${key} refers to unknown capture '${name}'`);
  }
  return name;
}

function loadOpenApiBinding(
  node: unknown,
  captures: readonly string[],
  where: string,
): OpenApiBinding {
  if (!isRecord(node)) {
    throw new RegistryError(`${where}: 'openapi' must be a mapping`);
  }
  for (const key of Object.keys(node)) {
    if (key !== 'method' && key !== 'path') {
      throw new RegistryError(`${where}: 'openapi' has unknown key '${key}' (allowed: method, path)`);
    }
  }
  const method = openApiCapture(node, 'method', captures, where);
  const path = openApiCapture(node, 'path', captures, where);
  return {
    methodCapture: method,
    methodGroup: captures.indexOf(method) + 1,
    pathCapture: path,
    pathGroup: captures.indexOf(path) + 1,
  };
}

// ── steps ───────────────────────────────────────────────────────────────────

function loadKeywords(node: unknown, where: string): string[] {
  if (node === undefined || node === null) return [];
  if (!Array.isArray(node) || node.some((keyword: unknown) => typeof keyword !== 'string')) {
    throw new RegistryError(`${where}: 'keywords' must be a list of strings`);
  }
  return [...(node as string[])];
}

function loadStepRef(node: unknown, docstring: DocstringSpec | null, where: string): StepRefSpec | null {
  if (node === undefined || node === null) return null;
  if (!isRecord(node)) {
    throw new RegistryError(`${where}: 'step_ref' must be a mapping`);
  }
  if (docstring === null || docstring.type !== 'json') {
    throw new RegistryError(
      `${where}: 'step_ref' requires a JSON docstring — there is nowhere else a step could be named`,
    );
  }
  const keyNode = node['key'];
  if (typeof keyNode !== 'string' || keyNode === '') {
    throw new RegistryError(`${where}: 'step_ref.key' must be a non-empty string`);
  }
  const kwNode = node['keywords'];
  let refKeywords: string[] = [];
  if (kwNode !== undefined && kwNode !== null) {
    if (!Array.isArray(kwNode) || kwNode.some((k) => typeof k !== 'string')) {
      throw new RegistryError(`${where}: 'step_ref.keywords' must be a list of strings`);
    }
    refKeywords = kwNode as string[];
  }
  const dsNode = node['docstring'];
  if (dsNode !== undefined && dsNode !== null && dsNode !== 'none' && dsNode !== 'any') {
    throw new RegistryError(`${where}: 'step_ref.docstring' must be 'none' or 'any'`);
  }
  return { key: keyNode, keywords: refKeywords, docstring: dsNode === 'none' ? 'none' : 'any' };
}

function loadStep(entry: unknown, where: string): StepDef {
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

  const keywords = loadKeywords(entry['keywords'], where);
  const docstring = loadDocstring(entry['docstring'], where);

  // Multi-line YAML block scalars (> or >-) fold newlines into spaces.
  // Normalise any remaining whitespace runs to a single space, then anchor.
  const pattern = anchor(pyCollapseWhitespace(patternNode));

  let regex: RegExp;
  try {
    regex = new RegExp(pattern, 'y');
  } catch (err) {
    throw new RegistryError(
      `${where}: invalid pattern ${JSON.stringify(pattern)}: ${describeRegexError(err)}`,
    );
  }

  const producesNode = entry['produces'];
  const openapiNode = entry['openapi'];
  const hasProduces = producesNode !== undefined && producesNode !== null;
  const hasOpenApi = openapiNode !== undefined && openapiNode !== null;
  if (hasProduces && typeof producesNode !== 'string') {
    throw new RegistryError(`${where}: 'produces' must be a string`);
  }

  // `produces` and `openapi` both point at capture groups by name, so both need
  // the names, and both need them to line up with the pattern's groups.
  let captures: string[] = [];
  let producesGroup: number | null = null;
  let openapi: OpenApiBinding | null = null;
  if (hasProduces || hasOpenApi) {
    captures = captureNames(entry['captures'], hasProduces ? 'produces' : 'openapi', where);
    const groups = countGroups(pattern);
    if (captures.length !== groups) {
      throw new RegistryError(
        `${where}: 'captures' lists ${captures.length} name(s) but the pattern has ` +
          `${groups} capture group(s)`,
      );
    }
  }
  if (typeof producesNode === 'string') {
    const index = captures.indexOf(producesNode);
    if (index === -1) {
      throw new RegistryError(
        `${where}: 'produces' names ${JSON.stringify(producesNode)}, which is not one of its captures`,
      );
    }
    producesGroup = index + 1;
  }
  if (hasOpenApi) {
    openapi = loadOpenApiBinding(openapiNode, captures, where);
  }

  const stepRef = loadStepRef(entry['step_ref'], docstring, where);

  return { id: idNode, keywords, pattern, docstring, regex, captures, producesGroup, openapi, stepRef };
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
 * @throws RegistryError for any problem with the file
 */
export function loadRegistry(source: string, origin: string): Registry {
  let raw: unknown;
  try {
    // `yaml` rejects duplicate mapping keys by default (uniqueKeys), which is
    // what makes a repeated `docstring:` or `id:` an error rather than a
    // silent last-one-wins.
    raw = parseYaml(source, { uniqueKeys: true });
  } catch (err) {
    const detail = err instanceof Error ? err.message : String(err);
    throw new RegistryError(`${origin} is not valid YAML: ${detail}`);
  }

  if (raw === null || raw === undefined) {
    throw new RegistryError(`${origin} is empty`);
  }
  if (!isRecord(raw)) {
    throw new RegistryError(`${origin} must contain a YAML mapping at the top level`);
  }

  const providedVars = loadProvidedVars(raw, origin);
  const substitutions = loadSubstitutions(raw['substitutions'], origin);

  const stepsNode = raw['steps'];
  if (stepsNode === undefined || stepsNode === null) return { steps: [], providedVars, substitutions };
  if (!Array.isArray(stepsNode)) {
    throw new RegistryError(`${origin}: 'steps' must be a list`);
  }

  const steps = stepsNode.map((entry: unknown, index) =>
    loadStep(entry, `${origin}: steps[${index}]`),
  );
  return { steps, providedVars, substitutions };
}

/** Anchored match, as the pattern's own `^` and `$` demand. */
export function matchesStepText(def: StepDef, text: string): boolean {
  def.regex.lastIndex = 0;
  return def.regex.test(text);
}
