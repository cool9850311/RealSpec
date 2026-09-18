import type { MessageKeys, Messages, NestedKeyOf } from 'next-intl'
import en_US from './en_US.json'
import zh_TW from './zh_TW.json'

// The i18n catalogue and the rules for choosing between its two locales.
//
// ── The locale cookie (contract with the e2e suite) ──────────────────────────
//
// `spec/bdd/format.yml`, step `locale_is`, says the suite "sets the app's
// locale cookie on the browser context and the matching Accept-Language
// header". This file names that cookie: `locale`, whose value is one of the
// identifiers below (`en_US`, `zh_TW`) — the same spelling the feature files
// and the catalogue filenames use. Accept-Language is honoured as well, as the
// fallback for a visitor who has never chosen a language; in a static export
// the browser's view of it is `navigator.languages`.
//
// ── Why the locale is applied after hydration, not before ────────────────────
//
// The build is a static export, so every HTML file on disk is rendered in
// DEFAULT_LOCALE. If the client picked a different locale *before* its first
// render, that render would disagree with the markup it is hydrating and React
// would report a hydration mismatch — an error the `console has no errors` step
// would (correctly) fail on. So the provider always starts in DEFAULT_LOCALE and
// components/Providers.tsx switches once hydration is over. The cost is one
// frame of English for a zh_TW visitor; the alternative is a console error on
// every page load.

export const SUPPORTED_LOCALES = ['en_US', 'zh_TW'] as const
export type AppLocale = (typeof SUPPORTED_LOCALES)[number]

export const DEFAULT_LOCALE: AppLocale = 'en_US'
export const LOCALE_COOKIE = 'locale'

/**
 * BCP 47 tag for each catalogue locale: the <html lang> attribute, and the
 * locale handed to next-intl and Intl.DateTimeFormat. The catalogue ids use an
 * underscore (`en_US`), which Intl rejects as a language tag, so they never
 * reach an Intl constructor directly.
 */
export const HTML_LANG: Record<AppLocale, string> = {
  en_US: 'en',
  zh_TW: 'zh-Hant-TW',
}

export const MESSAGES: Record<AppLocale, Messages> = { en_US, zh_TW }

/**
 * Every leaf key of the catalogue, as a type. A key held in state — the error
 * a page decided to show — is declared as this rather than as `string`, so a
 * typo in it is a `tsc` error exactly like a typo in a literal `t('…')` call.
 */
export type MessageKey = MessageKeys<Messages, NestedKeyOf<Messages>>

export function isAppLocale(value: unknown): value is AppLocale {
  return typeof value === 'string' && (SUPPORTED_LOCALES as readonly string[]).includes(value)
}

/**
 * Maps one Accept-Language / navigator.language tag onto a catalogue locale.
 * Anything Chinese-Traditional shaped (zh-TW, zh-Hant, zh-HK, zh_TW) selects
 * zh_TW; anything English shaped selects en_US; anything else selects neither.
 */
export function matchLocale(tag: string): AppLocale | null {
  const normalised = tag.trim().toLowerCase().replace(/_/g, '-')
  if (!normalised) return null
  if (normalised === 'zh_tw' || normalised.startsWith('zh-tw') || normalised.startsWith('zh-hant') || normalised.startsWith('zh-hk') || normalised.startsWith('zh-mo')) {
    return 'zh_TW'
  }
  if (normalised === 'en_us' || normalised === 'en' || normalised.startsWith('en-')) {
    return 'en_US'
  }
  return null
}
