/**
 * @realspec/cli — public API.
 *
 * A behaviour-compatible TypeScript port of the original
 * `validate.py`.
 */

export { parseSteps, pySplitlines, pyStrip, pyRstrip, pyCollapseWhitespace, type Step } from './parse.js';
export {
  anchor,
  loadRegistry,
  loadSteps,
  matchesStepText,
  RegistryError,
  type DocstringSpec,
  type Registry,
  type StepDef,
} from './registry.js';
export {
  checkHttpBody,
  checkJson,
  checkSql,
  pyReprStr,
  pyReprStrList,
  pyReprStrSet,
  pySorted,
  validate,
  type Violation,
} from './checks.js';
export { formatFileReport, formatMissingFile, formatViolation, pyPathStr } from './report.js';
export { findFormatFile, main, run, type RunOptions, type RunResult } from './cli.js';
