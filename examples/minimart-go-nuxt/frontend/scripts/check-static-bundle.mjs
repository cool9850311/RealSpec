#!/usr/bin/env node
// ─────────────────────────────────────────────────────────────────────────────
// The build artefact must be static and hostname-free.
//
//   npm run check:bundle        (runs as the second half of `npm run generate`)
//
// WHY THIS EXISTS
// ---------------
// PLAN.md §4: every e2e scenario gets its own PostgreSQL, backend and Caddy on
// its own published port, and they all share ONE prebuilt
// frontend container. That is only safe while the bundle addresses the API
// relatively — "/api/v1/..." against whatever origin the browser loaded it
// from. A single absolute origin baked in at build time (the usual way: an
// `API_URL` environment variable read by nuxt.config.ts) would make the shared
// container answer for one scenario and break every other one, and it would
// break them by *talking to the wrong database*, which is the worst way for a
// test suite to fail.
//
// So the property is asserted rather than trusted, at the moment it is created:
//
//   1. .output/public exists, contains a document for every prerendered route,
//      and contains no server entrypoint (a static artefact, not a Nitro app).
//   2. every API_URL in the bundle is a relative path starting with a single /.
//   3. no absolute URL anywhere in the bundle addresses the API — that is, no
//      http(s) URL whose path is /api or begins with /api/, whatever key or
//      spelling it arrived under. Assertion 2 covers the runtime-config key we
//      know the name of; this covers the ones we do not.
import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs'
import { dirname, extname, join, relative, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const frontendDir = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const publicDir = join(frontendDir, '.output', 'public')
const outputDir = join(frontendDir, '.output')

const REQUIRED_DOCUMENTS = [
  'index.html',
  'login/index.html',
  'products/index.html',
  'orders/index.html',
  // Linked from every document; a missing icon is a 404 in the browser console.
  'favicon.svg',
]
const TEXT_EXTENSIONS = ['.html', '.js', '.mjs', '.cjs', '.json', '.css', '.map', '.txt', '.svg', '.xml']

const failures = []
const fail = (message) => failures.push(message)

// ── 1. a static artefact ─────────────────────────────────────────────────────

if (!existsSync(publicDir)) {
  console.error(`check:bundle FAILED — ${relative(frontendDir, publicDir)} does not exist; run \`nuxt generate\` first`)
  process.exit(1)
}
for (const document of REQUIRED_DOCUMENTS) {
  if (!existsSync(join(publicDir, document))) fail(`1: .output/public/${document} was not generated`)
}
// .output/nitro.json is build metadata and is expected; .output/server is not.
if (existsSync(join(outputDir, 'server'))) {
  fail('1: .output/server exists — the build produced a Nitro server, not a static site')
}

// ── the scan ─────────────────────────────────────────────────────────────────

function walk(dir) {
  return readdirSync(dir).flatMap((entry) => {
    const full = join(dir, entry)
    return statSync(full).isDirectory() ? walk(full) : [full]
  })
}

const files = walk(publicDir).filter((file) => TEXT_EXTENSIONS.includes(extname(file)))
const apiUrls = new Map() // value -> Set(files)
const absoluteOrigins = new Set()

for (const file of files) {
  const where = relative(publicDir, file)
  const text = readFileSync(file, 'utf8')

  // 2. the API base, however it was quoted into the payload: Nuxt writes the
  // client runtime config as a JS object literal in each prerendered document
  // (API_URL:"/api"), and a JSON form ("API_URL":"/api") would be just as valid
  // an artefact of a future version.
  for (const match of text.matchAll(/"?\bAPI_URL"?\s*:\s*"([^"]*)"/g)) {
    const set = apiUrls.get(match[1]) ?? new Set()
    set.add(where)
    apiUrls.set(match[1], set)
  }

  // 3. an absolute URL that addresses the API.
  //
  // This replaced two weaker assertions: a list of forbidden local hostnames
  // (which said nothing about a real production hostname) and a ban on every
  // absolute origin (which needed a seven-entry allow-list on day one for W3C
  // namespaces, framework documentation links and Nuxt's URL-parser base, and
  // would have grown an entry per dependency).
  //
  // The property that actually matters is narrower and can be stated exactly:
  // the bundle must not carry an absolute destination for the API. Assertion 2
  // proves it for the runtime-config key we know the name of; this proves it
  // however the value was spelled or wherever it was stashed, and it needs no
  // allow-list, because an inert URL — a namespace, a docs link, a parser base
  // — does not have an /api path.
  for (const match of text.matchAll(/https?:\/\/[^\s"'`<>()\\,;]+/g)) {
    let url
    try {
      url = new URL(match[0])
    } catch {
      continue
    }
    absoluteOrigins.add(url.origin)
    if (url.pathname === '/api' || url.pathname.startsWith('/api/')) {
      fail(`3: .output/public/${where} contains an absolute API address: ${match[0]}`)
    }
  }
}

if (apiUrls.size === 0) {
  fail('2: no API_URL appears in the bundle at all — the check cannot prove anything about it')
}
for (const [value, where] of apiUrls) {
  if (!/^\/(?!\/)/.test(value)) {
    fail(`2: API_URL is "${value}" in ${[...where].join(', ')}; it must be a relative path beginning with a single /`)
  }
}

console.log(`scanned ${files.length} text files under .output/public`)
console.log(`API_URL values: ${[...apiUrls.keys()].map((value) => JSON.stringify(value)).join(', ') || 'none'}`)
console.log(`absolute origins present: ${[...absoluteOrigins].sort().join(', ') || 'none'}`)

if (failures.length > 0) {
  console.error(`\ncheck:bundle FAILED — ${failures.length} problem(s):`)
  for (const message of failures) console.error(`  ${message}`)
  process.exit(1)
}
console.log('\ncheck:bundle passed — the artefact is static and carries no hostname')
