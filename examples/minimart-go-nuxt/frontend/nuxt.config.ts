import { fileURLToPath } from 'node:url'
import VueI18nVitePlugin from '@intlify/unplugin-vue-i18n/vite'

// minimart front end — a fully static Nuxt SPA.
//
// Two properties of this configuration are load-bearing for the e2e suite
// (PLAN.md §4) and must survive any future edit:
//
//   1. `npm run generate` is the only build. There is no runtime Nuxt server;
//      the container serves `.output/public` with `serve`, so every route that
//      can be visited has to exist as a file on disk — hence the explicit
//      prerender list below and `failOnError`.
//
//   2. The bundle contains NO hostname. `API_URL` is the literal relative path
//      `/api`, not `process.env.API_URL || '/api'`: a build-time environment
//      variable is exactly how an absolute origin would get baked in, and one
//      prebuilt frontend container is shared by every scenario, each on its own
//      published port. `scripts/check-static-bundle.mjs` runs as part of
//      `npm run generate` and fails the build if a hostname appears.
//
// The pages render their data client-side (see composables/useApi.ts): the API
// is unreachable at build time, so prerendering produces the page chrome — the
// heading, the navigation, the table header — and the browser fills in the rows.
export default defineNuxtConfig({
  compatibilityDate: '2025-01-01',
  srcDir: '.',
  ssr: true,
  devtools: { enabled: false },

  app: {
    head: {
      // The lang attribute of the prerendered document matches DEFAULT_LOCALE;
      // plugins/locale.client.ts rewrites it after mount when the visitor's
      // locale differs. See i18n/index.ts for why the switch happens there.
      htmlAttrs: { lang: 'en' },
      title: 'minimart',
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
    // of framework plumbing, the same way the original front end's shared layer does.
    presets: [{ from: 'vue-i18n', imports: ['useI18n'] }],
  },

  runtimeConfig: {
    public: {
      // Relative on purpose, and not configurable. See the header comment.
      API_URL: '/api',
    },
  },

  nitro: {
    // Without this, @intlify/unplugin-vue-i18n emits a specifier for vue-i18n
    // in the server bundle that cannot resolve, and every prerendered route
    // fails with a 500. Same finding as the original front end's nuxt.config.ts.
    externals: {
      inline: ['vue-i18n'],
    },
    prerender: {
      // Hand-written and exhaustive: the static file server has no catch-all
      // rewrite, so a path with no file on disk answers 404.
      crawlLinks: false,
      failOnError: true,
      routes: ['/', '/login', '/products', '/orders'],
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
