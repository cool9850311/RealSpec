#!/usr/bin/env node
// check-spec-parity — fails when two examples that share a spec by contract
// have drifted apart in anything but prose.
//
//   node scripts/check-spec-parity.mjs [exampleA] [exampleB]
//
// Defaults: examples/minimart-go-nuxt and examples/minimart-java-next,
// resolved against the repository root (the parent of this script's directory).
//
// What must be identical:
//   spec/bdd/**/*.feature   same relative paths, byte for byte
//   spec/openapi/openapi.yaml   parsed, every `description` key removed, deep-equal
//   spec/bdd/format.yml     per step: id, keywords, pattern, captures (name,
//                           pattern), docstring (type, content_pattern, schema
//                           keys + required + other non-prose fields,
//                           special_values tokens), produces;
//                           context_variables.provided key set; docstring_types;
//                           outline_substitution minus description
// Comments and `description` text may differ: they name each example's own
// runner, files and language.

import { readFile, readdir } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { isDeepStrictEqual } from 'node:util';
import { parse as parseYaml } from 'yaml';

const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
export const DEFAULT_A = 'examples/minimart-go-nuxt';
export const DEFAULT_B = 'examples/minimart-java-next';

async function listFeatures(dir, base = dir) {
  let entries;
  try {
    entries = await readdir(dir, { withFileTypes: true });
  } catch (err) {
    if (err.code === 'ENOENT') return [];
    throw err;
  }
  const out = [];
  for (const e of entries) {
    const full = path.join(dir, e.name);
    if (e.isDirectory()) out.push(...(await listFeatures(full, base)));
    else if (e.isFile() && e.name.endsWith('.feature')) {
      out.push(path.relative(base, full).split(path.sep).join('/'));
    }
  }
  return out.sort();
}

function stripDescriptions(value) {
  if (Array.isArray(value)) return value.map(stripDescriptions);
  if (value && typeof value === 'object') {
    const out = {};
    for (const [k, v] of Object.entries(value)) {
      if (k === 'description') continue;
      out[k] = stripDescriptions(v);
    }
    return out;
  }
  return value;
}

function fmtPath(segments) {
  let s = '';
  for (const seg of segments) {
    s += typeof seg === 'number' ? `[${seg}]` : s === '' ? seg : `.${seg}`;
  }
  return s || '(root)';
}

function show(v) {
  if (v === undefined) return '<absent>';
  const s = JSON.stringify(v);
  return s.length > 120 ? `${s.slice(0, 117)}...` : s;
}

// Walks two values and reports every leaf-level difference with its path.
function deepDiff(a, b, at, out) {
  if (isDeepStrictEqual(a, b)) return;
  const bothArrays = Array.isArray(a) && Array.isArray(b);
  const bothObjects =
    a && b && typeof a === 'object' && typeof b === 'object' && !Array.isArray(a) && !Array.isArray(b);
  if (bothArrays) {
    if (a.length !== b.length) {
      out.push({ path: at, message: `array length ${a.length} vs ${b.length}` });
    }
    const n = Math.min(a.length, b.length);
    for (let i = 0; i < n; i++) deepDiff(a[i], b[i], [...at, i], out);
    return;
  }
  if (bothObjects) {
    const keys = new Set([...Object.keys(a), ...Object.keys(b)]);
    for (const k of [...keys].sort()) {
      if (!(k in a)) out.push({ path: [...at, k], message: `only in B: ${show(b[k])}` });
      else if (!(k in b)) out.push({ path: [...at, k], message: `only in A: ${show(a[k])}` });
      else deepDiff(a[k], b[k], [...at, k], out);
    }
    return;
  }
  out.push({ path: at, message: `${show(a)} vs ${show(b)}` });
}

async function readText(file) {
  try {
    return await readFile(file);
  } catch (err) {
    if (err.code === 'ENOENT') return null;
    throw err;
  }
}

async function loadYaml(file, rel, problems) {
  const buf = await readText(file);
  if (buf === null) {
    problems.push(`${rel}: missing`);
    return undefined;
  }
  try {
    return parseYaml(buf.toString('utf8'));
  } catch (err) {
    problems.push(`${rel}: YAML parse error: ${err.message}`);
    return undefined;
  }
}

function comparableStep(step) {
  const s = step ?? {};
  const out = {
    keywords: s.keywords,
    pattern: s.pattern,
    captures: Array.isArray(s.captures)
      ? s.captures.map((c) => ({ name: c?.name, pattern: c?.pattern }))
      : s.captures,
    produces: s.produces,
  };
  if (s.docstring && typeof s.docstring === 'object') {
    const d = stripDescriptions(s.docstring);
    if (Array.isArray(s.docstring.special_values)) {
      d.special_values = s.docstring.special_values.map((v) => v?.token);
    }
    out.docstring = d;
  } else {
    out.docstring = s.docstring;
  }
  return out;
}

function compareFormat(a, b, rel, diffs) {
  const push = (p, message) => diffs.push(`${rel}: ${p}: ${message}`);
  const stepsA = Array.isArray(a?.steps) ? a.steps : [];
  const stepsB = Array.isArray(b?.steps) ? b.steps : [];
  const idsA = stepsA.map((s) => s?.id);
  const idsB = stepsB.map((s) => s?.id);
  const byId = (steps) => {
    const m = new Map();
    for (const s of steps) {
      if (m.has(s?.id)) push(`steps`, `duplicate step id "${s?.id}"`);
      m.set(s?.id, s);
    }
    return m;
  };
  const mapA = byId(stepsA);
  const mapB = byId(stepsB);
  for (const id of idsA) if (!mapB.has(id)) push(`steps[id=${id}]`, 'step only in A');
  for (const id of idsB) if (!mapA.has(id)) push(`steps[id=${id}]`, 'step only in B');
  const common = idsA.filter((id) => mapB.has(id));
  const commonB = idsB.filter((id) => mapA.has(id));
  if (!isDeepStrictEqual(common, commonB)) {
    push('steps', `step order differs: ${show(common)} vs ${show(commonB)}`);
  }
  for (const id of common) {
    const out = [];
    deepDiff(comparableStep(mapA.get(id)), comparableStep(mapB.get(id)), [], out);
    for (const d of out) push(`steps[id=${id}].${fmtPath(d.path)}`, d.message);
  }

  const provA = Object.keys(a?.context_variables?.provided ?? {}).sort();
  const provB = Object.keys(b?.context_variables?.provided ?? {}).sort();
  if (!isDeepStrictEqual(provA, provB)) {
    push('context_variables.provided', `key sets differ: ${show(provA)} vs ${show(provB)}`);
  }

  for (const key of ['docstring_types', 'outline_substitution']) {
    const out = [];
    deepDiff(stripDescriptions(a?.[key]), stripDescriptions(b?.[key]), [key], out);
    for (const d of out) push(fmtPath(d.path), d.message);
  }

  const topA = Object.keys(a ?? {}).sort();
  const topB = Object.keys(b ?? {}).sort();
  if (!isDeepStrictEqual(topA, topB)) {
    push('(root)', `top-level keys differ: ${show(topA)} vs ${show(topB)}`);
  }
}

/**
 * Compare the spec of two examples.
 * @param {string} dirA absolute (or cwd-relative) path to the first example
 * @param {string} dirB absolute (or cwd-relative) path to the second example
 * @returns {Promise<{ ok: boolean, differences: string[], summary: object }>}
 */
export async function checkSpecParity(dirA, dirB) {
  const differences = [];
  const summary = { features: 0, steps: 0 };

  // (a) feature files
  const bddA = path.join(dirA, 'spec', 'bdd');
  const bddB = path.join(dirB, 'spec', 'bdd');
  const featA = await listFeatures(bddA);
  const featB = await listFeatures(bddB);
  const setB = new Set(featB);
  const setA = new Set(featA);
  for (const f of featA) if (!setB.has(f)) differences.push(`spec/bdd/${f}: only in A (${dirA})`);
  for (const f of featB) if (!setA.has(f)) differences.push(`spec/bdd/${f}: only in B (${dirB})`);
  for (const f of featA.filter((x) => setB.has(x))) {
    const [ba, bb] = await Promise.all([readFile(path.join(bddA, f)), readFile(path.join(bddB, f))]);
    if (!ba.equals(bb)) {
      let i = 0;
      while (i < ba.length && i < bb.length && ba[i] === bb[i]) i++;
      const line = ba.subarray(0, i).toString('utf8').split('\n').length;
      differences.push(`spec/bdd/${f}: contents differ (first difference at byte ${i}, line ${line})`);
    }
    summary.features++;
  }
  if (featA.length === 0 && featB.length === 0) {
    differences.push('spec/bdd: no .feature files found on either side');
  }

  // (b) openapi.yaml
  const oaRel = 'spec/openapi/openapi.yaml';
  const oaA = await loadYaml(path.join(dirA, oaRel), `${oaRel} (A)`, differences);
  const oaB = await loadYaml(path.join(dirB, oaRel), `${oaRel} (B)`, differences);
  if (oaA !== undefined && oaB !== undefined) {
    const out = [];
    deepDiff(stripDescriptions(oaA), stripDescriptions(oaB), [], out);
    for (const d of out) differences.push(`${oaRel}: ${fmtPath(d.path)}: ${d.message}`);
  }

  // (c) format.yml
  const fmRel = 'spec/bdd/format.yml';
  const fmA = await loadYaml(path.join(dirA, fmRel), `${fmRel} (A)`, differences);
  const fmB = await loadYaml(path.join(dirB, fmRel), `${fmRel} (B)`, differences);
  if (fmA !== undefined && fmB !== undefined) {
    compareFormat(fmA, fmB, fmRel, differences);
    summary.steps = Array.isArray(fmA?.steps) ? fmA.steps.length : 0;
  }

  return { ok: differences.length === 0, differences, summary };
}

async function main(argv) {
  const [a = DEFAULT_A, b = DEFAULT_B] = argv;
  const dirA = path.resolve(REPO_ROOT, a);
  const dirB = path.resolve(REPO_ROOT, b);
  const { ok, differences, summary } = await checkSpecParity(dirA, dirB);
  const label = (d) => {
    const rel = path.relative(REPO_ROOT, d);
    return rel === '' ? '.' : rel.startsWith('..') ? d : rel;
  };
  const la = label(dirA);
  const lb = label(dirB);
  if (!ok) {
    console.error(`spec parity FAILED: A = ${la}, B = ${lb}`);
    for (const d of differences) console.error(`  - ${d}`);
    console.error(`${differences.length} difference(s).`);
    return 1;
  }
  console.log(
    `spec parity OK: ${la} == ${lb} ` +
      `(${summary.features} feature files byte-identical, openapi.yaml equal modulo descriptions, ` +
      `format.yml ${summary.steps} steps equal modulo prose)`,
  );
  return 0;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main(process.argv.slice(2)).then(
    (code) => process.exit(code),
    (err) => {
      console.error(`spec parity ERROR: ${err?.stack ?? err}`);
      process.exit(2);
    },
  );
}
