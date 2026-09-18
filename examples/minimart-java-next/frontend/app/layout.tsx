import type { Metadata } from 'next'
import type { ReactNode } from 'react'
import { Providers } from '@/components/Providers'
import './globals.css'

// The document shell every route is exported into.
//
// `lang="en"` matches DEFAULT_LOCALE, the locale every page is prerendered in;
// components/Providers.tsx rewrites it after hydration when the visitor's
// locale differs (see i18n/index.ts for why the switch happens there).
export const metadata: Metadata = {
  title: 'minimart',
  robots: { index: false },
  // A real icon, so that the browser does not request /favicon.ico, fail to
  // find it, and write a console error the e2e suite would have to excuse.
  icons: { icon: { url: '/favicon.svg', type: 'image/svg+xml' } },
}

export default function RootLayout({ children }: { children: ReactNode }) {
  return (
    <html lang="en">
      <body>
        <Providers>{children}</Providers>
      </body>
    </html>
  )
}
