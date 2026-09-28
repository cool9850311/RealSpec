import { HTML_LANG, createAppI18n } from '~/i18n'
import { resolveLocale } from '~/i18n/browser-locale'

// Registers vue-i18n and, once the page has mounted, applies the visitor's own
// locale and sets the e2e suite's hydration marker.
//
// `app:suspense:resolve` is the hook minimart-go-nuxt/frontend uses (its own
// plugins/i18n.ts) for the same purpose, kept here unchanged: Nuxt calls it
// once the root `<Suspense>` has resolved and the page is mounted, which is
// late enough that the page's own `onMounted` data-fetching is already
// underway rather than still queued behind the app shell.
//
// `document.documentElement.dataset.appReady` is the hydration marker the e2e
// suite waits for (format.yml, step `visit`: "network idle plus the app's
// hydration marker"; tests/e2e/assertions.ts `waitForRender`). It is set after
// the locale is settled, so a test that waits for it can trust the strings on
// screen to be the ones its catalogue resolves.
export default defineNuxtPlugin({
  name: 'paygate:i18n',
  setup(nuxtApp) {
    const i18n = createAppI18n()
    nuxtApp.vueApp.use(i18n)

    if (import.meta.client) {
      nuxtApp.hook('app:suspense:resolve', () => {
        const locale = resolveLocale()
        if (i18n.global.locale.value !== locale) {
          i18n.global.locale.value = locale
        }
        document.documentElement.lang = HTML_LANG[locale]
        document.documentElement.dataset.appReady = 'true'
      })
    }
  },
})
