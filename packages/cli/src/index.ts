/**
 * @realspec/cli — public API.
 *
 * A validator whose rules are all data: `format.yml` declares what is enforced
 * (step patterns, docstring types, substitutions, `must_match`,
 * `allowed_top_level_keys`, `step_ref`, `openapi` bindings) and the code only
 * applies those mechanisms.
 */

export { parseSteps, pySplitlines, pyStrip, pyRstrip, pyCollapseWhitespace, type Step } from './parse.js';
export {
  anchor,
  applySubstitution,
  loadRegistry,
  loadSteps,
  matchesStepText,
  RegistryError,
  usesOpenApi,
  type DocstringSpec,
  type OpenApiBinding,
  type Registry,
  type StepDef,
  type StepRefSpec,
  type Substitution,
} from './registry.js';
export {
  checkDocstring,
  checkJson,
  checkSql,
  checkStepRefs,
  checkUnusedSteps,
  pyReprStr,
  pyReprStrList,
  pyReprStrSet,
  pySorted,
  substituteDocstring,
  validate,
  validateFile,
  type FileResult,
  type Violation,
} from './checks.js';
export { loadOpenApi, OpenApiIndex, openApiOperation, type OpenApiOperation } from './openapi.js';
export { formatFileReport, formatMissingFile, formatViolation, pyPathStr } from './report.js';
export { findFormatFile, main, run, type RunOptions, type RunResult } from './cli.js';
