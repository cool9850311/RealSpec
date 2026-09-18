#!/usr/bin/env node
// ─────────────────────────────────────────────────────────────────────────────
// The build artefact must be static and hostname-free.
//
//   npm run check:bundle        (runs as the second half of `npm run build`)
//
// WHY THIS EXISTS
// ---------------
// Every e2e scenario gets its own PostgreSQL, backend and Caddy on its own
// published port, and they all share ONE prebuilt frontend container. That is
// only safe while the bundle addresses the API relatively — "/api/v1/..."
// against whatever origin the browser loaded it from. A single absolute origin
// baked in at build time (the usual way: a NEXT_PUBLIC_API_URL read at build
// time) would make the shared container answer for one scenario and break
// every other one, and it would break them by *talking to the wrong database*,
// which is the worst way for a test suite to fail.
//
// So the property is asserted rather than trusted, at the moment it is created:
//
//   1. out/ exists, holds a document for every route the app can reach —
//      named the way `serve`'s clean URLs look them up (/login → login.html,
//      which is what `trailingSlash: false` produces) — and the build was a
//      static export, not a server build that happens to have an out/ lying
//      around from an earlier run.
//   2. the client bundle addresses the API by a relative path beginning with
//      /api. This is what proves the scan below looked at the code that talks
//      to the API: a scan that found no API address at all would pass for the
//      wrong reason.
//   3. no absolute URL anywhere in the bundle addresses the API — that is, no
//      http(s) URL whose path is /api or begins with /api/, whatever variable
//      or spelling it arrived under.
import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs'
import { dirname, extname, join, relative, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const frontendDir = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const outDir = join(frontendDir, 'out')
const exportDetail = join(frontendDir, '.next', 'export-detail.json')

const REQUIRED_DOCUMENTS = [
  'index.html',
  'login.html',
  'products.html',
  'orders.html',
  // Linked from every document; a missing icon is a 404 in the browser console.
  'favicon.svg',
]
const TEXT_EXTENSIONS = ['.html', '.js', '.mjs', '.cjs', '.json', '.css', '.map', '.txt', '.svg', '.xml']

const failures = []
const fail = (message) => failures.push(message)

// ── 1. a static artefact ─────────────────────────────────────────────────────

if (!existsSync(outDir)) {
  console.error('check:bundle FAILED — out/ does not exist; run `next build` first')
  process.exit(1)
}
for (const document of REQUIRED_DOCUMENTS) {
  if (!existsSync(join(outDir, document))) fail(`1: out/${document} was not exported`)
}

// Next writes this file only when `output: 'export'` ran, and records where it
// exported to. Without it, out/ could be the leftover of an earlier build while
// the current configuration produces a server.
if (!existsSync(exportDetail)) {
  fail('1: .next/export-detail.json is missing — the last `next build` was not a static export')
} else {
  const detail = JSON.parse(readFileSync(exportDetail, 'utf8'))
  if (detail.success !== true || resolve(detail.outDirectory ?? '') !== outDir) {
    fail(`1: .next/export-detail.json does not record a successful export to out/: ${JSON.stringify(detail)}`)
  }
}

function walk(dir) {
  return readdirSync(dir).flatMap((entry) => {
    const full = join(dir, entry)
    return statSync(full).isDirectory() ? walk(full) : [full]
  })
}

const allFiles = walk(outDir)

// A server has an entrypoint; a static site has only what a browser fetches.
// Every script a browser loads from a Next export lives under _next/static, so
// a script anywhere else in out/ is something that was meant to run on a server.
for (const file of allFiles) {
  const where = relative(outDir, file)
  if (where.split(/[\\/]/)[0] === 'server' || where.split(/[\\/]/).includes('.next')) {
    fail(`1: out/${where} is server build output`)
  } else if (['.js', '.mjs', '.cjs'].includes(extname(file)) && !where.startsWith(join('_next', 'static'))) {
    fail(`1: out/${where} is a script outside _next/static — a static export has no server entrypoint`)
  }
}

// ── the scan ─────────────────────────────────────────────────────────────────

const files = allFiles.filter((file) => TEXT_EXTENSIONS.includes(extname(file)))
const relativeApiRefs = new Set()
const absoluteOrigins = new Set()

for (const file of files) {
  const where = relative(outDir, file)
  const text = readFileSync(file, 'utf8')

  // 2. a relative API address: a string or template literal that starts with
  // /api. lib/api.ts builds every request as `/api/v1${path}`; whatever the
  // minifier makes of that, the literal still opens with a quote and /api.
  if (['.js', '.mjs', '.cjs'].includes(extname(file)) && /[`"']\/api(?:\/|[`"'$])/.test(text)) {
    relativeApiRefs.add(where)
  }

  // 3. an absolute URL that addresses the API.
  //
  // Not a ban on every absolute origin: the framework ships documentation
  // links, W3C namespaces and URL-parser test strings, and an allow-list for
  // those would grow an entry per dependency. The property that matters is
  // narrower and can be stated exactly — the bundle must not carry an absolute
  // destination for the API — and it needs no allow-list, because an inert URL
  // does not have an /api path.
  for (const match of text.matchAll(/https?:\/\/[^\s"'`<>()\\,;]+/g)) {
    let url
    try {
      url = new URL(match[0])
    } catch {
      continue
    }
    absoluteOrigins.add(url.origin)
    if (url.pathname === '/api' || url.pathname.startsWith('/api/')) {
      fail(`3: out/${where} contains an absolute API address: ${match[0]}`)
    }
  }
}

if (relativeApiRefs.size === 0) {
  fail('2: no script in out/ addresses the API by a relative /api path — the check cannot prove anything about it')
}

console.log(`scanned ${files.length} text files under out/`)
console.log(`relative /api references in: ${[...relativeApiRefs].sort().join(', ') || 'none'}`)
console.log(`absolute origins present: ${[...absoluteOrigins].sort().join(', ') || 'none'}`)

if (failures.length > 0) {
  console.error(`\ncheck:bundle FAILED — ${failures.length} problem(s):`)
  for (const message of failures) console.error(`  ${message}`)
  process.exit(1)
}
console.log('\ncheck:bundle passed — the artefact is static and carries no hostname')
