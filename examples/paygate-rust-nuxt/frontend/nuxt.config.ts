import { fileURLToPath } from 'node:url'
import VueI18nVitePlugin from '@intlify/unplugin-vue-i18n/vite'

// paygate front end — a fully static Nuxt SPA. minimart-go-nuxt/frontend is the
// example this holds still (spec.md: "the front end is the axis this example
// holds still"), and this file mirrors its structure and its two load-bearing
// properties:
//
//   1. `npm run generate` is the only build, and it is a CLIENT-ONLY SPA
//      (`ssr: false`): there is no server render at all, so a route can never
//      disagree with markup rendered elsewhere and there is nothing for the
//      browser to hydrate against — it MOUNTS into an empty shell. Every
//      reachable STATIC route is still listed under `nitro.prerender.routes` so
//      that the static file server has a document for it; `/shop/pay/<no>` is
//      the one route this app has that is not enumerable ahead of time (an
//      unbounded merchant order number), and it is served by nuxt's own SPA
//      fallback (`.output/public/200.html`, written automatically because
//      `ssr: false`): local/Caddyfile must `try_files` an unmatched path under
//      /shop/pay/ to that file. See frontend's final report for the exact
//      requirement handed to the ORCHESTRATOR.
//
//   2. The bundle contains NO hostname. Both API bases are literal relative
//      paths — `/api` for paygate's own dashboard API, `/demo-merchant/api` for
//      the demo shop — never `process.env.…`, so one prebuilt frontend
//      container can be shared by every e2e scenario, each on its own
//      published port. `scripts/check-static-bundle.mjs` runs as part of
//      `npm run generate` and fails the build if a hostname appears.
export default defineNuxtConfig({
  compatibilityDate: '2025-01-01',
  srcDir: '.',
  ssr: false,
  devtools: { enabled: false },

  app: {
    head: {
      htmlAttrs: { lang: 'en' },
      title: 'paygate',
      // A real icon, so that the browser does not request /favicon.ico, fail to
      // find it, and write a console error the e2e suite would have to excuse.
      link: [{ rel: 'icon', type: 'image/svg+xml', href: '/favicon.svg' }],
      meta: [
        { charset: 'utf-8' },
        { name: 'viewport', content: 'width=device-width, initial-scale=1' },
        { name: 'robots', content: 'noindex' },
      ],
    },
  },

  css: [fileURLToPath(new URL('./assets/css/app.css', import.meta.url))],

  imports: {
    // `useI18n` is used by every page; auto-importing it keeps the pages free
    // of framework plumbing.
    presets: [{ from: 'vue-i18n', imports: ['useI18n'] }],
  },

  runtimeConfig: {
    public: {
      // Relative on purpose, and not configurable. See the header comment.
      API_URL: '/api',
      SHOP_API_URL: '/demo-merchant/api',
    },
  },

  nitro: {
    // Without this, @intlify/unplugin-vue-i18n emits a specifier for vue-i18n
    // in the server bundle that cannot resolve, and every prerendered route
    // fails with a 500 (same finding as minimart-go-nuxt/frontend).
    externals: {
      inline: ['vue-i18n'],
    },
    prerender: {
      // Hand-written and exhaustive for every STATIC route; the file server has
      // no catch-all rewrite for anything not listed here or produced as the
      // SPA fallback. `/shop/pay/[merchantTradeNo]` is deliberately absent: its
      // id space is unbounded, and with `ssr: false` every prerendered document
      // is the same empty shell anyway, so the SPA fallback document
      // (`200.html`) serves it exactly as well as a per-id file would.
      crawlLinks: false,
      failOnError: true,
      routes: ['/', '/login', '/reports', '/shop', '/shop/result'],
    },
  },

  vite: {
    plugins: [
      VueI18nVitePlugin({
        strictMessage: false,
        include: [fileURLToPath(new URL('./i18n/*.json', import.meta.url))],
      }),
    ],
  },

  typescript: {
    strict: true,
    typeCheck: false,
  },
})
