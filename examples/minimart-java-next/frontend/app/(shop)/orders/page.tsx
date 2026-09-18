'use client'

import { useRouter } from 'next/navigation'
import { useLocale, useTranslations } from 'next-intl'
import { useEffect, useMemo, useState } from 'react'
import type { MessageKey } from '@/i18n'
import { apiRequest } from '@/lib/api'
import { useSession } from '@/lib/session'
import type { Order, OrderList } from '@/lib/types'

export default function OrdersPage() {
  const t = useTranslations()
  // Already a BCP 47 tag (HTML_LANG): components/Providers.tsx hands next-intl
  // the tag, not the catalogue id.
  const localeTag = useLocale()
  const router = useRouter()
  const { load: loadSession } = useSession()

  const [orders, setOrders] = useState<Order[]>([])
  const [loading, setLoading] = useState(true)
  const [listErrorKey, setListErrorKey] = useState<MessageKey | null>(null)

  const dateTimeFormat = useMemo(
    () => new Intl.DateTimeFormat(localeTag, { dateStyle: 'medium', timeStyle: 'short' }),
    [localeTag],
  )

  function formatCreatedAt(iso: string): string {
    const at = new Date(iso)
    // An unparseable timestamp is shown verbatim rather than as "Invalid Date":
    // the cell must never be empty, and the raw value is the useful thing to see.
    return Number.isNaN(at.getTime()) ? iso : dateTimeFormat.format(at)
  }

  useEffect(() => {
    async function loadOrders(): Promise<void> {
      setListErrorKey(null)
      try {
        // Newest first is the API's promise (openapi.yaml, listMyOrders); the page
        // renders the order it was given so that a regression there is visible here
        // instead of being papered over by a client-side sort.
        const page = await apiRequest<OrderList>('/orders')
        setOrders(page.items)
      } catch {
        setOrders([])
        setListErrorKey('orders.loadFailed')
      }
    }

    // The gate runs after mount rather than before rendering because the page
    // is exported as a static file: a check at build time would see no API and
    // no cookie, and a redirect decided during hydration would replace the
    // markup React is in the middle of hydrating.
    async function gate(): Promise<void> {
      const me = await loadSession().catch(() => null)
      if (!me) {
        router.replace('/login')
        return
      }
      await loadOrders()
      setLoading(false)
    }

    void gate()
  }, [loadSession, router])

  return (
    <section className="page">
      <h1 className="page__title">{t('orders.title')}</h1>

      {loading ? (
        <p className="page__status">{t('orders.loading')}</p>
      ) : listErrorKey ? (
        <p className="alert" role="alert">
          {t(listErrorKey)}
        </p>
      ) : orders.length === 0 ? (
        <p className="page__status" data-testid="orders-empty">
          {t('orders.empty')}
        </p>
      ) : (
        <table className="table">
          <thead>
            <tr>
              <th scope="col">{t('orders.columnId')}</th>
              <th scope="col">{t('orders.columnProduct')}</th>
              <th scope="col">{t('orders.columnCost')}</th>
              <th scope="col">{t('orders.columnCreatedAt')}</th>
            </tr>
          </thead>
          <tbody>
            {orders.map((order) => (
              <tr key={order.id} data-testid={`order-row-${order.id}`}>
                <td className="table__number" data-testid="order-id">
                  {order.id}
                </td>
                <td data-testid="order-product">{order.product_name}</td>
                <td className="table__number" data-testid="order-cost">
                  {order.cost_points}
                </td>
                <td data-testid="order-created-at">{formatCreatedAt(order.created_at)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </section>
  )
}
