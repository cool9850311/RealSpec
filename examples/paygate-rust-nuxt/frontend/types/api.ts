// paygate's own API, transcribed from spec/openapi/openapi.yaml.
//
// Field names are the wire names (snake_case) on purpose: renaming them in the
// front end would hide a mismatch between this app and the API behind a
// mapping layer, and the whole point of the example is that a mismatch fails
// loudly.

export type LoginRequest = {
  email: string
  password: string
}

export interface Merchant {
  id: number
  name: string
  currency: string
  timezone: string
}

export interface DashboardUser {
  email: string
  merchant: Merchant
}

export interface ReportFigures {
  succeeded_count: number
  failed_count: number
  refunded_count: number
  gross_amount: number
  refunded_amount: number
  net_amount: number
  success_rate_bps: number | null
}

export interface ReportDay extends ReportFigures {
  date: string
}

export interface ReportTotals extends ReportFigures {
  declines: Record<string, number>
}

export interface DailyReport {
  currency: string
  timezone: string
  from: string
  to: string
  days: ReportDay[]
  totals: ReportTotals
}

/**
 * The readable cookie `POST /dashboard/login` sets beside the HttpOnly
 * `session` (spec/openapi/openapi.yaml, `dashboardLogin`; spec.md,
 * "Authentication"). It is a flag, not a credential: the API never reads it.
 * Its only purpose is to let the SPA tell "signed out" from "signed in"
 * without calling a protected endpoint — see composables/useSession.ts.
 */
export const SESSION_HINT_COOKIE = 'session_hint'
