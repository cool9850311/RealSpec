#!/usr/bin/env node
// ─────────────────────────────────────────────────────────────────────────────
// The build artefact must be static and hostname-free.
//
//   npm run check:bundle        (runs as the second half of `npm run generate`)
//
// WHY THIS EXISTS
// ---------------
// Every e2e scenario gets its own PostgreSQL, backend and Caddy on its own
// published port, and they all share ONE prebuilt frontend container. That is
// only safe while the bundle addresses BOTH APIs it calls relatively — `/api`
// for paygate's own dashboard, `/demo-merchant/api` for the demo shop —
// against whatever origin the browser loaded it from. A single absolute origin
// baked in at build time would make the shared container answer for one
// scenario and break every other one, and it would break them by *talking to
// the wrong database*, which is the worst way for a test suite to fail.
//
// So the property is asserted rather than trusted, at the moment it is
// created:
//
//   1. .output/public exists, contains a document for every STATIC route this
//      app prerendered, an SPA fallback for the one route it could not
//      (`/shop/pay/[merchantTradeNo]`'s id space is unbounded), and no server
//      entrypoint (a static artefact, not a Nitro app).
//   2. every API_URL / SHOP_API_URL in the bundle is a relative path starting
//      with a single /.
//   3. no absolute URL anywhere in the bundle addresses either API — that is,
//      no http(s) URL whose path is /api or /demo-merchant/api or begins with
//      either followed by /, whatever key or spelling it arrived under.
//      Assertion 2 covers the two runtime-config keys we know the names of;
//      this covers any others.
import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs'
import { dirname, extname, join, relative, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const frontendDir = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const publicDir = join(frontendDir, '.output', 'public')
const outputDir = join(frontendDir, '.output')

const REQUIRED_DOCUMENTS = [
  'index.html',
  'login/index.html',
  'reports/index.html',
  'shop/index.html',
  'shop/result/index.html',
  // Linked from every document; a missing icon is a 404 in the browser console.
  'favicon.svg',
]
// `/shop/pay/<merchantTradeNo>` cannot be enumerated ahead of time (an
// unbounded merchant order number), so nuxt's own SPA fallback serves it —
// written automatically because `ssr: false` (nuxt.config.ts). Whichever of
// these this Nuxt version writes, at least one must exist for local/Caddyfile
// to fall back to.
const FALLBACK_CANDIDATES = ['200.html', '404.html']
const TEXT_EXTENSIONS = ['.html', '.js', '.mjs', '.cjs', '.json', '.css', '.map', '.txt', '.svg', '.xml']
const API_KEYS = ['API_URL', 'SHOP_API_URL']
const API_PREFIXES = ['/api', '/demo-merchant/api']

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
if (!FALLBACK_CANDIDATES.some((name) => existsSync(join(publicDir, name)))) {
  fail(
    `1: none of ${FALLBACK_CANDIDATES.map((name) => `.output/public/${name}`).join(' or ')} exists — ` +
      '/shop/pay/<merchantTradeNo> would have no document to fall back to',
  )
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
const apiUrls = new Map() // "KEY=value" -> Set(files)
const absoluteOrigins = new Set()

for (const file of files) {
  const where = relative(publicDir, file)
  const text = readFileSync(file, 'utf8')

  // 2. the API bases, however they were quoted into the payload: Nuxt writes
  // the client runtime config as a JS object literal in each prerendered
  // document (API_URL:"/api"), and a JSON form ("API_URL":"/api") would be
  // just as valid an artefact of a future version.
  for (const key of API_KEYS) {
    for (const match of text.matchAll(new RegExp(`"?\\b${key}"?\\s*:\\s*"([^"]*)"`, 'g'))) {
      const id = `${key}=${match[1]}`
      const set = apiUrls.get(id) ?? new Set()
      set.add(where)
      apiUrls.set(id, set)
    }
  }

  // 3. an absolute URL that addresses either API.
  for (const match of text.matchAll(/https?:\/\/[^\s"'`<>()\\,;]+/g)) {
    let url
    try {
      url = new URL(match[0])
    } catch {
      continue
    }
    absoluteOrigins.add(url.origin)
    for (const prefix of API_PREFIXES) {
      if (url.pathname === prefix || url.pathname.startsWith(`${prefix}/`)) {
        fail(`3: .output/public/${where} contains an absolute API address: ${match[0]}`)
      }
    }
  }
}

for (const key of API_KEYS) {
  const found = [...apiUrls.keys()].filter((id) => id.startsWith(`${key}=`))
  if (found.length === 0) {
    fail(`2: ${key} does not appear in the bundle at all — the check cannot prove anything about it`)
    continue
  }
  for (const id of found) {
    const value = id.slice(key.length + 1)
    if (!/^\/(?!\/)/.test(value)) {
      const where = [...apiUrls.get(id)].join(', ')
      fail(`2: ${key} is "${value}" in ${where}; it must be a relative path beginning with a single /`)
    }
  }
}

console.log(`scanned ${files.length} text files under .output/public`)
console.log(`API values: ${[...apiUrls.keys()].map((id) => JSON.stringify(id)).join(', ') || 'none'}`)
console.log(`absolute origins present: ${[...absoluteOrigins].sort().join(', ') || 'none'}`)

if (failures.length > 0) {
  console.error(`\ncheck:bundle FAILED — ${failures.length} problem(s):`)
  for (const message of failures) console.error(`  ${message}`)
  process.exit(1)
}
console.log('\ncheck:bundle passed — the artefact is static and carries no hostname')
