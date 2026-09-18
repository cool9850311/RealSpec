'use client'

import { useRouter } from 'next/navigation'
import { useTranslations } from 'next-intl'
import { useEffect, useState } from 'react'
import type { MessageKey } from '@/i18n'
import { ApiError, apiRequest } from '@/lib/api'
import { useSession } from '@/lib/session'
import {
  UNLIMITED_STOCK,
  type ApiErrorCode,
  type CreateOrderRequest,
  type CreateOrderResponse,
  type Product,
  type ProductList,
} from '@/lib/types'

// One code, one catalogue key. The API sends stable identifiers and the front
// end owns the words (spec/openapi/openapi.yaml, schema Error); anything
// unmapped falls back to a generic apology rather than leaking the identifier.
const REDEEM_MESSAGE: Record<ApiErrorCode, MessageKey> = {
  INVALID_REQUEST: 'shop.unexpectedError',
  INVALID_CREDENTIALS: 'shop.unexpectedError',
  UNAUTHENTICATED: 'shop.unexpectedError',
  PRODUCT_NOT_FOUND: 'shop.productNotFound',
  INSUFFICIENT_POINTS: 'shop.insufficientPoints',
  OUT_OF_STOCK: 'shop.outOfStock',
}

export default function ProductsPage() {
  const t = useTranslations()
  const router = useRouter()
  const { load: loadSession, setPoints, clear: clearSession } = useSession()

  const [products, setProducts] = useState<Product[]>([])
  const [loading, setLoading] = useState(true)
  const [listErrorKey, setListErrorKey] = useState<MessageKey | null>(null)
  const [redeemErrorKey, setRedeemErrorKey] = useState<MessageKey | null>(null)

  function stockLabel(stock: number): string {
    return stock === UNLIMITED_STOCK ? t('shop.unlimited') : String(stock)
  }

  useEffect(() => {
    async function loadProducts(): Promise<void> {
      setLoading(true)
      setListErrorKey(null)
      try {
        // The catalogue is public and already ordered by id; rendering it in the
        // order the API returned is the only way a wrong order can fail a test.
        const page = await apiRequest<ProductList>('/products')
        setProducts(page.items)
      } catch {
        setProducts([])
        setListErrorKey('shop.loadFailed')
      } finally {
        setLoading(false)
      }
    }

    // Both on mount, never during the export's prerender: the API does not
    // exist at build time. The catalogue is public, so the session is resolved
    // alongside it rather than before it — and for a visitor with no session
    // cookie that costs no request at all (lib/session.tsx). A transport failure
    // while asking is not a reason to hide the catalogue: they simply have no
    // balance to show.
    void loadProducts()
    void loadSession().catch(() => undefined)
  }, [loadSession])

  async function redeem(product: Product): Promise<void> {
    setRedeemErrorKey(null)
    try {
      const result = await apiRequest<CreateOrderResponse, CreateOrderRequest>('/orders', {
        method: 'POST',
        body: { product_id: product.id },
      })
      // In place, without a reload: the response says what the balance is now,
      // and the row's stock follows the same rule the transaction used — only a
      // positive stock is decremented, the -1 sentinel never is. The row is
      // replaced rather than mutated, because React re-renders on identity.
      setPoints(result.balance_after)
      setProducts((current) =>
        current.map((row) =>
          row.id === product.id && row.stock > 0 ? { ...row, stock: row.stock - 1 } : row,
        ),
      )
    } catch (error) {
      if (!(error instanceof ApiError)) {
        setRedeemErrorKey('shop.unexpectedError')
        return
      }
      if (error.status === 401) {
        // The cookie expired between page load and click.
        clearSession()
        router.push('/login')
        return
      }
      setRedeemErrorKey(error.code ? REDEEM_MESSAGE[error.code] : 'shop.unexpectedError')
    }
  }

  return (
    <section className="page">
      <h1 className="page__title">{t('shop.title')}</h1>

      {redeemErrorKey && (
        <p className="alert" role="alert" data-testid="redeem-error">
          {t(redeemErrorKey)}
        </p>
      )}

      {loading ? (
        <p className="page__status">{t('shop.loading')}</p>
      ) : listErrorKey ? (
        <p className="alert" role="alert">
          {t(listErrorKey)}
        </p>
      ) : products.length === 0 ? (
        <p className="page__status">{t('shop.empty')}</p>
      ) : (
        <table className="table">
          <thead>
            <tr>
              <th scope="col">{t('shop.columnName')}</th>
              <th scope="col">{t('shop.columnCost')}</th>
              <th scope="col">{t('shop.columnStock')}</th>
              <th scope="col">{t('shop.columnRedeem')}</th>
            </tr>
          </thead>
          <tbody>
            {products.map((product) => (
              <tr key={product.id} data-testid={`product-row-${product.id}`}>
                <td data-testid="product-name">{product.name}</td>
                <td className="table__number" data-testid="product-cost">
                  {product.cost_points}
                </td>
                <td className="table__number" data-testid="product-stock">
                  {stockLabel(product.stock)}
                </td>
                <td>
                  <button
                    className="button button--primary"
                    type="button"
                    data-testid={`redeem-${product.id}`}
                    onClick={() => void redeem(product)}
                  >
                    {t('shop.redeem')}
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </section>
  )
}
