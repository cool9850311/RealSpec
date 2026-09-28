// The demo shop's own, tab-local memory of "which order am I looking at".
//
// WHY THIS EXISTS
// ----------------
// `client_back_url` — the page the PROVIDER sends the customer back to once
// they are done — is a bare path (spec.md, "Being told by the provider": "the
// customer comes back to the shop, the shop reads its own record"), and
// `page URL is "/shop/result"` (shop.feature) asserts it lands there with no
// query string at all. So `/shop/result` cannot learn which order it is
// showing from its own URL, and paygate's `ClientBackURL` redirect (unlike a
// real ECPay `OrderResultURL`, which this example deliberately never registers
// — see spec.md) carries nothing else either.
//
// What it CAN learn from is this tab's own memory: `/shop/pay/<no>` sets it
// the moment it knows which order it is rendering a form for, and every hop
// after that — the provider's cashier, the issuer's 3-D Secure page, the
// redirect back — stays on the SAME origin (one reverse proxy, spec.md,
// "System overview"), so `sessionStorage` survives the whole round trip even
// though each page along the way is a different backend service's own HTML,
// not this SPA's.
const LAST_ORDER_KEY = 'paygate-demo:last-order'

/**
 * The demo shop's own, single currency. `demo-merchant.yaml`'s `Order` schema
 * carries no currency field because this example seeds exactly one demo
 * merchant (`Acme Coffee`, `USD` — shop.feature's Background), so the shop's
 * own page is free to know its own currency the way a real storefront's
 * templates do, without asking an API for it.
 */
export const SHOP_CURRENCY = 'USD'

/** Records which order this tab is currently paying for. */
export function rememberOrder(merchantTradeNo: string): void {
  if (!import.meta.client) return
  try {
    sessionStorage.setItem(LAST_ORDER_KEY, merchantTradeNo)
  } catch {
    // Best effort: a private-browsing quota or a disabled store loses nothing
    // but the /shop/result convenience this enables — see recallOrder.
  }
}

/** The most recent order this tab remembered, or null. */
export function recallOrder(): string | null {
  if (!import.meta.client) return null
  try {
    return sessionStorage.getItem(LAST_ORDER_KEY)
  } catch {
    return null
  }
}
