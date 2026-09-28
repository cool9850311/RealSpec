<script setup lang="ts">
import type { ShopOrder } from '~/types/shop'
import { recallOrder } from '~/utils/shop-session'

definePageMeta({ layout: 'bare' })

const { t } = useI18n()
const shop = useShopApi()

/**
 * How often to ask the merchant's own record while it is still
 * `awaiting_payment`. This is a CONDITION dressed as an interval, not a wait
 * with a duration in mind: the loop below stops the instant the record settles
 * one way or the other, and the polling window itself is unbounded — the
 * merchant genuinely does not know when, or whether, the provider's callback
 * arrives (spec.md: "if the callback has not arrived yet the shop says so").
 */
const POLL_INTERVAL_MS = 1000

const merchantTradeNo = ref<string | null>(null)
const order = ref<ShopOrder | null>(null)
let timer: ReturnType<typeof setInterval> | undefined

type ResultState = 'awaiting' | 'paid' | 'refunded' | 'failed'

/**
 * The merchant's own record settles the "paid"/"refunded" question, straight
 * from `status`. It never settles "failed" — a declined attempt leaves the
 * order `awaiting_payment` forever and tells the merchant nothing (spec.md,
 * NFR-SETTLE-2) — so "failed" is read from `last_attempt_status` instead, an
 * OPTIONAL field this build's `GET /orders/{no}` may report about the most
 * recent attempt (see types/shop.ts `ShopAttemptStatus` for why this is
 * schema-compatible and this frontend's build report for why it is required:
 * without it, shop.feature's "fails authentication" and "declined card"
 * scenarios cannot be told apart from an attempt still in flight).
 */
const state = computed<ResultState>(() => {
  if (!order.value) return 'awaiting'
  if (order.value.status === 'paid') return 'paid'
  if (order.value.status === 'refunded') return 'refunded'
  if (order.value.last_attempt_status === 'failed') return 'failed'
  return 'awaiting'
})

const STATE_KEY: Record<ResultState, string> = {
  awaiting: 'shop.resultAwaiting',
  paid: 'shop.resultPaid',
  refunded: 'shop.resultRefunded',
  failed: 'shop.resultFailed',
}

const statusText = computed(() => t(STATE_KEY[state.value]))

async function poll(): Promise<void> {
  const no = merchantTradeNo.value
  if (!no) return
  try {
    order.value = await shop.getOrder(no)
  } catch {
    // Transient failure: keep the last known state and try again on the next
    // tick rather than flashing an error over a real, still-payable order.
    return
  }
  if (order.value.status !== 'awaiting_payment' && timer !== undefined) {
    clearInterval(timer)
    timer = undefined
  }
}

onMounted(async () => {
  merchantTradeNo.value = recallOrder()
  if (!merchantTradeNo.value) return
  await poll()
  if (order.value?.status === 'awaiting_payment') {
    timer = setInterval(poll, POLL_INTERVAL_MS)
  }
})

onBeforeUnmount(() => {
  if (timer !== undefined) clearInterval(timer)
})

async function payAgain(): Promise<void> {
  const no = merchantTradeNo.value
  if (!no) return
  await navigateTo(`/shop/pay/${encodeURIComponent(no)}`)
}
</script>

<template>
  <section class="card">
    <h1 class="card__title">{{ t('shop.resultTitle') }}</h1>

    <p v-if="!merchantTradeNo" class="page__status" data-testid="shop-result-not-found">
      {{ t('shop.resultNotFound') }}
    </p>

    <template v-else>
      <dl class="summary">
        <div class="summary__row">
          <dt>{{ t('shop.orderNumber') }}</dt>
          <dd>{{ merchantTradeNo }}</dd>
        </div>
      </dl>

      <!--
        `aria-label` makes the accessible name exactly the translated string
        regardless of how a browser computes a status region's name from
        content, so `"status:@shop.resultPaid"` resolves the same way the text
        the visitor reads does — the same string, in whichever locale is active.
      -->
      <p role="status" :aria-label="statusText" data-testid="shop-result-status">{{ statusText }}</p>

      <button
        v-if="state === 'awaiting' || state === 'failed'"
        class="button button--primary"
        type="button"
        @click="payAgain"
      >
        {{ t('shop.payAgain') }}
      </button>
    </template>
  </section>
</template>
