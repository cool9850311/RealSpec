// The demo merchant's own API, transcribed from spec/openapi/demo-merchant.yaml.
// NOT paygate's — a different service, on its own path prefix, reached only
// server-to-server by this shop's own backend except for the three calls this
// browser makes directly (create order, read the signed form, read the order).

// `type`, not `interface`, on purpose: only a type alias gets TypeScript's
// implicit index signature, which is what makes it assignable to the JSON-body
// parameter of the fetch helper in composables/useApi.ts.
export type CreateShopOrderRequest = {
  /** Decimal string, e.g. "12.50" — the wire shape demo-merchant.yaml requires. */
  amount: string
}

export interface CreateShopOrderResponse {
  merchant_trade_no: string
  /** This merchant's own page — always `/shop/pay/<merchant_trade_no>`. */
  pay_url: string
}

/** What the merchant's page must submit to the provider: `POST /orders/{no}/form`. */
export interface ShopPaymentForm {
  action: string
  fields: Record<string, string>
}

export type ShopOrderStatus = 'awaiting_payment' | 'paid' | 'refunded'

/**
 * `AttemptStatus` (crates/domain), mirrored here as a STRING the demo
 * merchant's `GET /orders/{no}` may optionally report about the most recent
 * attempt.
 *
 * THIS FIELD IS NOT IN spec/openapi/demo-merchant.yaml's DOCUMENTED
 * PROPERTIES, and it is required for `shop.feature`'s "Payment failed" and
 * "try again" scenarios to be tellable at all — see this frontend's build
 * report for the full reasoning. The schema's `Order` object does not declare
 * `additionalProperties: false`, so a field beyond the four documented ones is
 * schema-compatible; it is simply undocumented today. Read defensively:
 * `undefined` (the field absent, e.g. before crates/demo-merchant adds it) is
 * treated exactly like `null` (no attempt, or the merchant does not expose one
 * yet) — never a crash.
 */
export type ShopAttemptStatus = 'redirected' | 'succeeded' | 'failed' | 'abandoned' | null

export interface ShopOrder {
  merchant_trade_no: string
  status: ShopOrderStatus
  /** Minor units — the same convention as paygate's own `Amount`. */
  amount: number
  trade_no: string | null
  /** See {@link ShopAttemptStatus}. Optional until crates/demo-merchant adds it. */
  last_attempt_status?: ShopAttemptStatus
}
