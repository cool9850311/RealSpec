'use client'

import { useRouter } from 'next/navigation'
import { useTranslations } from 'next-intl'
import { useEffect } from 'react'

// `/`. It lives in the (shop) group so that it renders inside the same chrome
// as the catalogue it hands over to, as the Nuxt front end's index page does.
//
// There is no content at the root: the catalogue is the front door. The
// redirect happens after mount so that the exported file on disk stays a plain
// document — the static server has no rewrite rules and no redirect support,
// and every route the app can reach has to be a file.
export default function IndexPage() {
  const t = useTranslations()
  const router = useRouter()

  useEffect(() => {
    router.replace('/products')
  }, [router])

  return <p className="page__status">{t('app.redirecting')}</p>
}
