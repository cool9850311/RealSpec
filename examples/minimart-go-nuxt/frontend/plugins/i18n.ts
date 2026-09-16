import { HTML_LANG, createAppI18n } from '~/i18n'
import { resolveLocale } from '~/i18n/browser-locale'

// Registers vue-i18n for both halves of the build — the prerender pass and the
// browser — and, in the browser only, applies the visitor's own locale once the
// page is hydrated.
//
// WHY THE LOCALE IS APPLIED THAT LATE
// -----------------------------------
// Every HTML file on disk was rendered in DEFAULT_LOCALE. If the client picked a
// different locale before or during its first render, that render would disagree
// with the markup it is hydrating and Vue would log four "Hydration text
// mismatch" warnings and one "Hydration completed but contains mismatches."
// error — an error `console has no errors` would rightly fail on. So the
// instance always starts in DEFAULT_LOCALE and the switch happens in
// `app:suspense:resolve`, which Nuxt calls once the root <Suspense> has resolved
// and the layout and page are hydrated.
//
// `app:mounted` is NOT late enough: the page component is an async import, so
// the root mounts while the page subtree is still waiting to hydrate. Measured,
// not guessed.
//
// The cost is one frame of DEFAULT_LOCALE for a zh_TW visitor. The alternative
// is a console error on every page load.
//
// `document.documentElement.dataset.appReady` is the hydration marker the e2e
// suite waits for (format.yml, step `visit`: "network idle plus the app's
// hydration marker"). It is set after the locale is settled, so a test that
// waits for it can trust the strings on screen to be the ones its catalogue
// resolves.
export default defineNuxtPlugin({
  name: 'minimart:i18n',
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
