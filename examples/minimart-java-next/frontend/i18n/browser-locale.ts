import { DEFAULT_LOCALE, LOCALE_COOKIE, isAppLocale, matchLocale, type AppLocale } from '@/i18n'
import { readCookie } from '@/lib/cookie'

// Browser-only: resolveLocale reads `document.cookie` (through readCookie) and
// `navigator`. It is called from an effect in components/Providers.tsx, and
// effects never run during the static export's prerender.

/**
 * The locale the visitor asked for: their explicit choice (the `locale`
 * cookie) first, then the languages the browser sends as Accept-Language,
 * then the default.
 */
export function resolveLocale(): AppLocale {
  const fromCookie = readCookie(LOCALE_COOKIE)
  if (isAppLocale(fromCookie)) return fromCookie
  for (const tag of navigator.languages ?? [navigator.language]) {
    const matched = matchLocale(tag)
    if (matched) return matched
  }
  return DEFAULT_LOCALE
}
