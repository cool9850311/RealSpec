import type en_US from './i18n/en_US.json'

// Binds next-intl's translation functions to the English catalogue's shape, so
// `t('shop.titel')` — or any key the catalogue does not have — is a type error
// under `npm run check:types` instead of a raw key on screen at run time.
// en_US is the reference because it is DEFAULT_LOCALE, the locale every page is
// prerendered in; i18n/index.ts types zh_TW against the same shape, so a key
// missing from the Chinese catalogue fails the same check.
declare module 'next-intl' {
  interface AppConfig {
    Messages: typeof en_US
  }
}
