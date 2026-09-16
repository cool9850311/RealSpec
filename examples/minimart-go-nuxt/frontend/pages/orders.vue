<script setup lang="ts">
import { HTML_LANG, isAppLocale, DEFAULT_LOCALE } from '~/i18n'
import type { Order, OrderList } from '~/types/api'

const { t, locale } = useI18n()
const api = useApi()
const session = useSession()

const orders = ref<Order[]>([])
const loading = ref(true)
const listErrorKey = ref<string | null>(null)

const dateTimeFormat = computed(() => {
  const tag = HTML_LANG[isAppLocale(locale.value) ? locale.value : DEFAULT_LOCALE]
  return new Intl.DateTimeFormat(tag, { dateStyle: 'medium', timeStyle: 'short' })
})

function formatCreatedAt(iso: string): string {
  const at = new Date(iso)
  // An unparseable timestamp is shown verbatim rather than as "Invalid Date":
  // the cell must never be empty, and the raw value is the useful thing to see.
  return Number.isNaN(at.getTime()) ? iso : dateTimeFormat.value.format(at)
}

async function loadOrders(): Promise<void> {
  listErrorKey.value = null
  try {
    // Newest first is the API's promise (openapi.yaml, listMyOrders); the page
    // renders the order it was given so that a regression there is visible here
    // instead of being papered over by a client-side sort.
    const page = await api.request<OrderList>('/orders')
    orders.value = page.items
  } catch {
    orders.value = []
    listErrorKey.value = 'orders.loadFailed'
  }
}

onMounted(async () => {
  // The gate runs after mount rather than in route middleware because the page
  // is prerendered: middleware would run at build time, where there is no API
  // and no cookie, and a redirect decided during hydration would rewrite the
  // markup Vue is in the middle of hydrating.
  const me = await session.load().catch(() => null)
  if (!me) {
    await navigateTo('/login', { replace: true })
    return
  }
  await loadOrders()
  loading.value = false
})
</script>

<template>
  <section class="page">
    <h1 class="page__title">{{ t('orders.title') }}</h1>

    <p v-if="loading" class="page__status">{{ t('orders.loading') }}</p>
    <p v-else-if="listErrorKey" class="alert" role="alert">{{ t(listErrorKey) }}</p>
    <p v-else-if="orders.length === 0" class="page__status" data-testid="orders-empty">
      {{ t('orders.empty') }}
    </p>

    <table v-else class="table">
      <thead>
        <tr>
          <th scope="col">{{ t('orders.columnId') }}</th>
          <th scope="col">{{ t('orders.columnProduct') }}</th>
          <th scope="col">{{ t('orders.columnCost') }}</th>
          <th scope="col">{{ t('orders.columnCreatedAt') }}</th>
        </tr>
      </thead>
      <tbody>
        <tr v-for="order in orders" :key="order.id" :data-testid="`order-row-${order.id}`">
          <td class="table__number" data-testid="order-id">{{ order.id }}</td>
          <td data-testid="order-product">{{ order.product_name }}</td>
          <td class="table__number" data-testid="order-cost">{{ order.cost_points }}</td>
          <td data-testid="order-created-at">{{ formatCreatedAt(order.created_at) }}</td>
        </tr>
      </tbody>
    </table>
  </section>
</template>
