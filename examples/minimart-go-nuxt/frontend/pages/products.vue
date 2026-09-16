<script setup lang="ts">
import { ApiError } from '~/composables/useApi'
import {
  UNLIMITED_STOCK,
  type ApiErrorCode,
  type CreateOrderRequest,
  type CreateOrderResponse,
  type Product,
  type ProductList,
} from '~/types/api'

const { t } = useI18n()
const api = useApi()
const session = useSession()

const products = ref<Product[]>([])
const loading = ref(true)
const listErrorKey = ref<string | null>(null)
const redeemErrorKey = ref<string | null>(null)

// One code, one catalogue key. The API sends stable identifiers and the front
// end owns the words (spec/openapi/openapi.yaml, schema Error); anything
// unmapped falls back to a generic apology rather than leaking the identifier.
const REDEEM_MESSAGE: Record<ApiErrorCode, string> = {
  INVALID_REQUEST: 'shop.unexpectedError',
  INVALID_CREDENTIALS: 'shop.unexpectedError',
  UNAUTHENTICATED: 'shop.unexpectedError',
  PRODUCT_NOT_FOUND: 'shop.productNotFound',
  INSUFFICIENT_POINTS: 'shop.insufficientPoints',
  OUT_OF_STOCK: 'shop.outOfStock',
}

function stockLabel(stock: number): string {
  return stock === UNLIMITED_STOCK ? t('shop.unlimited') : String(stock)
}

async function loadProducts(): Promise<void> {
  loading.value = true
  listErrorKey.value = null
  try {
    // The catalogue is public and already ordered by id; rendering it in the
    // order the API returned is the only way a wrong order can fail a test.
    const page = await api.request<ProductList>('/products')
    products.value = page.items
  } catch {
    products.value = []
    listErrorKey.value = 'shop.loadFailed'
  } finally {
    loading.value = false
  }
}

async function redeem(product: Product): Promise<void> {
  redeemErrorKey.value = null
  try {
    const result = await api.request<CreateOrderResponse, CreateOrderRequest>('/orders', {
      method: 'POST',
      body: { product_id: product.id },
    })
    // In place, without a reload: the response says what the balance is now,
    // and the row's stock follows the same rule the transaction used — only a
    // positive stock is decremented, the -1 sentinel never is.
    session.setPoints(result.balance_after)
    if (product.stock > 0) {
      product.stock -= 1
    }
  } catch (error) {
    if (!(error instanceof ApiError)) {
      redeemErrorKey.value = 'shop.unexpectedError'
      return
    }
    if (error.status === 401) {
      // The cookie expired between page load and click.
      session.clear()
      await navigateTo('/login')
      return
    }
    redeemErrorKey.value = error.code ? REDEEM_MESSAGE[error.code] : 'shop.unexpectedError'
  }
}

onMounted(() => {
  // Both on mount, never during prerendering: the API does not exist at build
  // time. The catalogue is public, so the session is resolved alongside it
  // rather than before it — and for a visitor with no session cookie that costs
  // no request at all (composables/useSession.ts). A transport failure while
  // asking is not a reason to hide the catalogue: they simply have no balance
  // to show.
  void loadProducts()
  void session.load().catch(() => undefined)
})
</script>

<template>
  <section class="page">
    <h1 class="page__title">{{ t('shop.title') }}</h1>

    <p v-if="redeemErrorKey" class="alert" role="alert" data-testid="redeem-error">
      {{ t(redeemErrorKey) }}
    </p>

    <p v-if="loading" class="page__status">{{ t('shop.loading') }}</p>
    <p v-else-if="listErrorKey" class="alert" role="alert">{{ t(listErrorKey) }}</p>
    <p v-else-if="products.length === 0" class="page__status">{{ t('shop.empty') }}</p>

    <table v-else class="table">
      <thead>
        <tr>
          <th scope="col">{{ t('shop.columnName') }}</th>
          <th scope="col">{{ t('shop.columnCost') }}</th>
          <th scope="col">{{ t('shop.columnStock') }}</th>
          <th scope="col">{{ t('shop.columnRedeem') }}</th>
        </tr>
      </thead>
      <tbody>
        <tr v-for="product in products" :key="product.id" :data-testid="`product-row-${product.id}`">
          <td data-testid="product-name">{{ product.name }}</td>
          <td class="table__number" data-testid="product-cost">{{ product.cost_points }}</td>
          <td class="table__number" data-testid="product-stock">{{ stockLabel(product.stock) }}</td>
          <td>
            <button
              class="button button--primary"
              type="button"
              :data-testid="`redeem-${product.id}`"
              @click="redeem(product)"
            >
              {{ t('shop.redeem') }}
            </button>
          </td>
        </tr>
      </tbody>
    </table>
  </section>
</template>
