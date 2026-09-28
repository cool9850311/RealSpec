import { HTML_LANG, type AppLocale } from '~/i18n'

/**
 * Formats a minor-unit amount (paygate's `Amount`, ECPay's own convention) as
 * the merchant's currency, in the active locale — `reports.feature`:
 * "Amounts are formatted in the merchant's currency for the active locale."
 *
 * `Intl.NumberFormat` disambiguates on its own: `en_US` renders USD as `$`,
 * `zh_TW` renders it as `US$` (its own currency would otherwise read the same
 * way), which is exactly what the report and shop e2e scenarios assert.
 */
export function formatCurrency(amountMinor: number, currency: string, locale: AppLocale): string {
  return new Intl.NumberFormat(HTML_LANG[locale], { style: 'currency', currency }).format(amountMinor / 100)
}

/**
 * Formats `success_rate_bps` (0–10000, or null when there were no attempts to
 * rate) as the two-decimal percentage `reports.feature` asserts — "100.00%",
 * "66.66%" — or the em dash for null.
 */
export function formatSuccessRate(bps: number | null, locale: AppLocale): string {
  if (bps === null) return '—'
  return new Intl.NumberFormat(HTML_LANG[locale], {
    style: 'percent',
    minimumFractionDigits: 2,
    maximumFractionDigits: 2,
  }).format(bps / 10000)
}
