<script setup lang="ts">
import { ApiError } from '~/composables/useApi'
import { DEFAULT_LOCALE, isAppLocale } from '~/i18n'
import type { DailyReport } from '~/types/api'
import { formatCurrency, formatSuccessRate } from '~/utils/currency'

const { t, locale } = useI18n()
const api = useApi()
const session = useSession()

const activeLocale = computed(() => (isAppLocale(locale.value) ? locale.value : DEFAULT_LOCALE))

const report = ref<DailyReport | null>(null)
const fromInput = ref('')
const toInput = ref('')
const loadErrorKey = ref<string | null>(null)
const loading = ref(true)

/**
 * `GET /dashboard/reports/daily`, with whatever range the two fields hold.
 * Empty fields send no `from`/`to` at all, which is how the API's own default
 * (the last 7 days) is reached on first load — `reports.feature`, "A merchant
 * with no activity sees zeros" relies on exactly this.
 */
async function fetchReport(): Promise<void> {
  loading.value = true
  loadErrorKey.value = null
  try {
    const query: Record<string, string | undefined> = {
      from: fromInput.value || undefined,
      to: toInput.value || undefined,
    }
    const result = await api.request<DailyReport>('/dashboard/reports/daily', { query })
    report.value = result
    // Reflects the range the API actually answered for — its own default the
    // first time, or the range just applied — so the fields never disagree
    // with the table beneath them.
    fromInput.value = result.from
    toInput.value = result.to
  } catch (error) {
    report.value = null
    if (error instanceof ApiError && error.status === 401) {
      session.clear()
      await navigateTo('/login', { replace: true })
      return
    }
    // 422 (RANGE_TOO_LARGE / VALIDATION_FAILED) and 503 (REPORTING_UNAVAILABLE)
    // both land here: the one thing this page promises is that it never shows
    // a blank table for a request the API refused (`reports.feature`, "A
    // merchant with no activity sees zeros, not an error" — the error state and
    // the zero state are deliberately different elements).
    loadErrorKey.value = 'reports.unavailable'
  } finally {
    loading.value = false
  }
}

onMounted(async () => {
  // The gate runs after mount, not in route middleware: this app is a plain
  // SPA (`ssr: false`) with no server request to attach middleware to, and the
  // session cookie is only ever readable from the browser.
  const me = await session.load().catch(() => null)
  if (!me) {
    await navigateTo('/login', { replace: true })
    return
  }
  await fetchReport()
})
</script>

<template>
  <section class="page">
    <h1 class="page__title">{{ t('reports.title') }}</h1>

    <form class="filter" novalidate @submit.prevent="fetchReport">
      <div class="form__field">
        <label class="form__label" for="report-from">{{ t('reports.from') }}</label>
        <input
          id="report-from"
          v-model="fromInput"
          class="form__input"
          type="text"
          name="from"
          placeholder="YYYY-MM-DD"
          autocomplete="off"
        >
      </div>
      <div class="form__field">
        <label class="form__label" for="report-to">{{ t('reports.to') }}</label>
        <input
          id="report-to"
          v-model="toInput"
          class="form__input"
          type="text"
          name="to"
          placeholder="YYYY-MM-DD"
          autocomplete="off"
        >
      </div>
      <button class="button button--primary" type="submit">{{ t('reports.apply') }}</button>
    </form>

    <p v-if="loadErrorKey" class="alert" role="alert" data-testid="report-unavailable">
      {{ t(loadErrorKey) }}
    </p>

    <p v-else-if="loading" class="page__status">{{ t('reports.loading') }}</p>

    <table v-else-if="report" class="table">
      <thead>
        <tr>
          <th scope="col">{{ t('reports.columnDate') }}</th>
          <th scope="col">{{ t('reports.columnSucceeded') }}</th>
          <th scope="col">{{ t('reports.columnFailed') }}</th>
          <th scope="col">{{ t('reports.columnGross') }}</th>
          <th scope="col">{{ t('reports.columnRefunded') }}</th>
          <th scope="col">{{ t('reports.columnNet') }}</th>
          <th scope="col">{{ t('reports.columnSuccessRate') }}</th>
        </tr>
      </thead>
      <tbody>
        <tr v-for="day in report.days" :key="day.date" :data-testid="`report-day-${day.date}`">
          <td data-testid="day-date">{{ day.date }}</td>
          <td class="table__number" data-testid="day-succeeded">{{ day.succeeded_count }}</td>
          <td class="table__number" data-testid="day-failed">{{ day.failed_count }}</td>
          <td class="table__number" data-testid="day-gross">
            {{ formatCurrency(day.gross_amount, report.currency, activeLocale) }}
          </td>
          <td class="table__number" data-testid="day-refunded">
            {{ formatCurrency(day.refunded_amount, report.currency, activeLocale) }}
          </td>
          <td class="table__number" data-testid="day-net">
            {{ formatCurrency(day.net_amount, report.currency, activeLocale) }}
          </td>
          <td class="table__number" data-testid="day-success-rate">
            {{ formatSuccessRate(day.success_rate_bps, activeLocale) }}
          </td>
        </tr>
      </tbody>
      <tfoot>
        <tr data-testid="report-totals">
          <th scope="row">{{ t('reports.totalsLabel') }}</th>
          <td class="table__number" data-testid="total-succeeded">{{ report.totals.succeeded_count }}</td>
          <td class="table__number" data-testid="total-failed">{{ report.totals.failed_count }}</td>
          <td class="table__number" data-testid="total-gross">
            {{ formatCurrency(report.totals.gross_amount, report.currency, activeLocale) }}
          </td>
          <td class="table__number" data-testid="total-refunded">
            {{ formatCurrency(report.totals.refunded_amount, report.currency, activeLocale) }}
          </td>
          <td class="table__number" data-testid="total-net">
            {{ formatCurrency(report.totals.net_amount, report.currency, activeLocale) }}
          </td>
          <td class="table__number" data-testid="total-success-rate">
            {{ formatSuccessRate(report.totals.success_rate_bps, activeLocale) }}
          </td>
        </tr>
      </tfoot>
    </table>
  </section>
</template>
