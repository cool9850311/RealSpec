import { createApiClient } from '~/composables/useApi'
import type { CreateShopOrderRequest, CreateShopOrderResponse, ShopOrder, ShopPaymentForm } from '~/types/shop'

/**
 * The demo merchant's own API, under `/demo-merchant/api` — NOT paygate's.
 * These are the only three calls this browser ever makes to it (spec.md, "One
 * order, three numbers" / "The integration model"): start a purchase, read the
 * form paygate signed, and read the merchant's own belief about the order.
 * paygate itself has no endpoint a browser calls (NFR-SEC-4).
 */
export function useShopApi() {
  const { public: { SHOP_API_URL } } = useRuntimeConfig()
  const client = createApiClient(SHOP_API_URL)

  function createOrder(amount: string): Promise<CreateShopOrderResponse> {
    return client.request<CreateShopOrderResponse, CreateShopOrderRequest>('/orders', {
      method: 'POST',
      body: { amount },
    })
  }

  function getPaymentForm(merchantTradeNo: string): Promise<ShopPaymentForm> {
    return client.request<ShopPaymentForm>(`/orders/${encodeURIComponent(merchantTradeNo)}/form`)
  }

  function getOrder(merchantTradeNo: string): Promise<ShopOrder> {
    return client.request<ShopOrder>(`/orders/${encodeURIComponent(merchantTradeNo)}`)
  }

  return { createOrder, getPaymentForm, getOrder }
}
