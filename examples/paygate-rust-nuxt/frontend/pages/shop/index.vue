<script setup lang="ts">
definePageMeta({ layout: 'bare' })

const { t } = useI18n()
const shop = useShopApi()

const amount = ref('')
const errorKey = ref<string | null>(null)
const submitting = ref(false)

/**
 * `POST /demo-merchant/api/orders`, then straight to this merchant's OWN
 * payment page — never paygate's, which has none (spec.md, "paygate is not on
 * the payment path"). The merchant allocated its order number and its form
 * server to server; this browser has touched only the merchant so far.
 */
async function checkout(): Promise<void> {
  if (submitting.value) return
  submitting.value = true
  errorKey.value = null
  try {
    const order = await shop.createOrder(amount.value)
    await navigateTo(order.pay_url)
  } catch {
    errorKey.value = 'shop.checkoutError'
  } finally {
    submitting.value = false
  }
}
</script>

<template>
  <section class="card">
    <h1 class="card__title">{{ t('shop.brand') }}</h1>

    <form class="form" novalidate @submit.prevent="checkout">
      <div class="form__field">
        <label class="form__label" for="amount">{{ t('shop.amount') }}</label>
        <input
          id="amount"
          v-model="amount"
          class="form__input"
          type="text"
          name="amount"
          inputmode="decimal"
          autocomplete="off"
          placeholder="0.00"
        >
      </div>

      <button class="button button--primary" type="submit" :aria-busy="submitting">
        {{ t('shop.checkout') }}
      </button>

      <p v-if="errorKey" class="alert" role="alert" data-testid="shop-checkout-error">{{ t(errorKey) }}</p>
    </form>
  </section>
</template>
