import type { NextConfig } from 'next'

// minimart front end — a fully static Next.js export.
//
// Two properties of this configuration are load-bearing for the e2e suite and
// must survive any future edit:
//
//   1. `output: 'export'`. There is no runtime Next server; the container
//      serves out/ with `serve`, so every route that can be visited has to exist
//      as a file on disk. `trailingSlash: false` makes those files
//      login.html, products.html, orders.html — which is what `serve`'s
//      default clean-URL handling maps /login, /products and /orders onto.
//
//   2. The bundle contains NO hostname. The API base is the literal relative
//      path `/api` in lib/api.ts, and there is deliberately no `env` entry and
//      no NEXT_PUBLIC_* variable here: a build-time environment variable is
//      exactly how an absolute origin would get baked in, and one prebuilt
//      frontend container is shared by every scenario, each on its own
//      published port. `scripts/check-static-bundle.mjs` runs as part of
//      `npm run build` and fails the build if an absolute API address appears.
//
// The pages render their data client-side: the API is unreachable at build
// time, so the export produces the page chrome — the heading, the navigation,
// the loading line — and the browser fills in the rows.
const nextConfig: NextConfig = {
  output: 'export',
  trailingSlash: false,
  // The image optimiser is a server feature; a static export has no server.
  images: { unoptimized: true },
  poweredByHeader: false,
}

export default nextConfig
