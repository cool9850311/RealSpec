<script setup lang="ts">
import { DEFAULT_LOCALE, isAppLocale } from '~/i18n'
import type { ShopOrder, ShopPaymentForm } from '~/types/shop'
import { formatCurrency } from '~/utils/currency'
import { SHOP_CURRENCY, rememberOrder } from '~/utils/shop-session'

definePageMeta({ layout: 'bare' })

const { t, locale } = useI18n()
const route = useRoute()
const shop = useShopApi()

const merchantTradeNo = computed(() => String(route.params.merchantTradeNo))
const activeLocale = computed(() => (isAppLocale(locale.value) ? locale.value : DEFAULT_LOCALE))

const order = ref<ShopOrder | null>(null)
const form = ref<ShopPaymentForm | null>(null)
const errorKey = ref<string | null>(null)

/**
 * Reads the merchant's own record (for the amount to show) and, separately,
 * the form paygate signed for this order — asking again mints a NEW provider
 * order every time (demo-merchant.yaml, `shopGetPaymentForm`: "Asking again
 * mints a new one"), which is exactly what a fresh render of this page must
 * do, on purpose, every time it mounts (`shop.feature`, "Rendering the payment
 * page twice", "Pressing Back … is how a customer ends up able to pay twice").
 */
async function load(): Promise<void> {
  rememberOrder(merchantTradeNo.value)
  errorKey.value = null
  try {
    order.value = await shop.getOrder(merchantTradeNo.value)
  } catch {
    order.value = null
  }

  // A paid order has no payment page any more (`shop.feature`): there is
  // nothing left to render a form for.
  if (order.value && order.value.status !== 'awaiting_payment') {
    await navigateTo('/shop/result', { replace: true })
    return
  }

  try {
    form.value = await shop.getPaymentForm(merchantTradeNo.value)
  } catch {
    form.value = null
    errorKey.value = 'shop.payFormError'
  }
}

onMounted(load)
</script>

<template>
  <section class="card">
    <h1 class="card__title">{{ t('shop.payTitle') }}</h1>

    <dl class="summary" data-testid="shop-pay-summary">
      <div class="summary__row">
        <dt>{{ t('shop.merchant') }}</dt>
        <dd data-testid="shop-pay-merchant">{{ t('shop.brand') }}</dd>
      </div>
      <div class="summary__row">
        <dt>{{ t('shop.orderNumber') }}</dt>
        <dd data-testid="shop-pay-order-no">{{ merchantTradeNo }}</dd>
      </div>
      <div v-if="order" class="summary__row">
        <dt>{{ t('shop.amountLabel') }}</dt>
        <dd data-testid="shop-pay-amount">{{ formatCurrency(order.amount, SHOP_CURRENCY, activeLocale) }}</dd>
      </div>
    </dl>

    <p v-if="errorKey" class="alert" role="alert" data-testid="shop-pay-error">{{ t(errorKey) }}</p>

    <!--
      Everything but the button is a hidden field: this IS the hidden form
      paygate's hand-off asked the merchant's page to submit
      (spec/openapi/demo-merchant.yaml, `shopGetPaymentForm`) — only the button
      is visible, because the customer, not a script, decides when to leave for
      the provider (`shop.feature` clicks "button:@shop.pay" itself; nothing
      here auto-submits).
    -->
    <form v-if="form" class="form" :action="form.action" method="post">
      <input v-for="(value, name) in form.fields" :key="name" type="hidden" :name="name" :value="value">
      <button class="button button--primary" type="submit">{{ t('shop.pay') }}</button>
    </form>
  </section>
</template>
