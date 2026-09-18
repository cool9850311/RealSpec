'use client'

import { NextIntlClientProvider } from 'next-intl'
import { useEffect, useState, type ReactNode } from 'react'
import { DEFAULT_LOCALE, HTML_LANG, MESSAGES, type AppLocale } from '@/i18n'
import { resolveLocale } from '@/i18n/browser-locale'
import { SessionProvider } from '@/lib/session'

// A missing key must render as the raw key and stay silent: the e2e suite
// addresses elements by the string a key resolves to, so a raw key is a red
// test rather than a plausible-looking fallback. next-intl's default handler
// would write the miss to the console, and `console has no errors` is an
// assertion in these features — so the handler is silent, and a wrong key is
// caught where it can be, by `npm run check:types` (see global.d.ts).
function ignoreIntlError(): void {}

function keyAsFallback({ namespace, key }: { namespace?: string; key: string }): string {
  return namespace ? `${namespace}.${key}` : key
}

/**
 * The client-side context every route renders inside: the i18n catalogue and
 * the session.
 *
 * WHY THE LOCALE IS APPLIED THAT LATE
 * -----------------------------------
 * Every HTML file in out/ was rendered in DEFAULT_LOCALE. If the client picked a
 * different locale before or during its first render, that render would
 * disagree with the markup it is hydrating and React would report a hydration
 * mismatch — an error `console has no errors` would rightly fail on. So the
 * first render is always DEFAULT_LOCALE, whatever the visitor asked for, and the
 * switch happens in an effect: effects run only after the tree they belong to
 * has been hydrated and committed, and this component's effects run after its
 * children's.
 *
 * `document.documentElement.dataset.appReady` is the hydration marker the e2e
 * suite waits for (format.yml, step `visit`: "network idle plus the app's
 * hydration marker"). It is set in a SECOND effect, keyed on the resolved
 * locale, which therefore runs only once the re-render in that locale has been
 * committed to the DOM — so a test that waits for it can trust the strings on
 * screen to be the ones its catalogue resolves. The resolved locale is `null`
 * until the first effect has run, which is what makes the second effect fire
 * even for a visitor whose locale is the default one.
 *
 * The cost is one frame of DEFAULT_LOCALE for a zh_TW visitor. The alternative
 * is a console error on every page load.
 */
export function Providers({ children }: { children: ReactNode }) {
  const [resolvedLocale, setResolvedLocale] = useState<AppLocale | null>(null)

  useEffect(() => {
    setResolvedLocale(resolveLocale())
  }, [])

  useEffect(() => {
    if (resolvedLocale === null) return
    document.documentElement.lang = HTML_LANG[resolvedLocale]
    document.documentElement.dataset.appReady = 'true'
  }, [resolvedLocale])

  const locale = resolvedLocale ?? DEFAULT_LOCALE
  return (
    <NextIntlClientProvider
      // The BCP 47 tag, not the catalogue id: next-intl hands this to Intl,
      // which rejects `zh_TW`. Pages read it back with useLocale().
      locale={HTML_LANG[locale]}
      messages={MESSAGES[locale]}
      onError={ignoreIntlError}
      getMessageFallback={keyAsFallback}
    >
      <SessionProvider>{children}</SessionProvider>
    </NextIntlClientProvider>
  )
}
