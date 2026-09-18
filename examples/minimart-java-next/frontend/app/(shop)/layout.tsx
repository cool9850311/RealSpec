'use client'

import Link from 'next/link'
import { usePathname } from 'next/navigation'
import { useTranslations } from 'next-intl'
import type { ReactNode } from 'react'
import { useSession } from '@/lib/session'

const NAV = [
  { href: '/products', key: 'nav.products' },
  { href: '/orders', key: 'nav.orders' },
] as const

// The layout with the site chrome: every route except /login. A route group
// rather than a nested segment, so the URLs stay /products and /orders.
export default function ShopLayout({ children }: { children: ReactNode }) {
  const t = useTranslations()
  const { user } = useSession()
  const pathname = usePathname()

  return (
    <div className="app">
      <header className="app__header">
        <span className="app__brand">{t('app.name')}</span>

        <nav className="app__nav" aria-label={t('nav.primary')}>
          {NAV.map(({ href, key }) => (
            <Link
              key={href}
              className="app__nav-link"
              href={href}
              aria-current={pathname === href ? 'page' : undefined}
            >
              {t(key)}
            </Link>
          ))}
        </nav>

        {/*
          The balance is rendered only for a signed-in visitor, which is also what
          makes it a usable assertion: `"testid:points-balance" is hidden` is the
          e2e suite's way of saying "nobody is signed in". The element holds the
          number and nothing else, because the assertion compares its whole text.
        */}
        {user && (
          <p className="app__balance">
            <span className="app__balance-label">{t('session.points')}</span>
            <span className="app__balance-value" data-testid="points-balance">
              {user.points}
            </span>
          </p>
        )}
      </header>

      <main className="app__main">{children}</main>
    </div>
  )
}
