import { DEFAULT_LOCALE, LOCALE_COOKIE, isAppLocale, matchLocale, type AppLocale } from '~/i18n'
import { readCookie } from '~/utils/cookie'

// Browser-only: resolveLocale reads `document.cookie` (through readCookie) and
// `navigator`. It is called from plugins/i18n.ts behind an `import.meta.client`
// guard, which is also what keeps it out of the prerender bundle.

/**
 * The locale the visitor asked for: their explicit choice (the `locale`
 * cookie) first, then the languages the browser sent in Accept-Language,
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
