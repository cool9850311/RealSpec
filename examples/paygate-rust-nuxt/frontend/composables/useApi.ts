/**
 * A response the API refused, or a request that never got one.
 *
 * `status` is the HTTP status, or 0 when the request failed at the transport
 * level. `code` is the stable identifier from the body — `{"error": "<CODE>"}`
 * for paygate's dashboard API (spec/openapi/openapi.yaml, schema Error), or the
 * one string demo-merchant's own error body carries (`GATEWAY_UNAVAILABLE`,
 * spec/openapi/demo-merchant.yaml). Human text for a code lives in the i18n
 * catalogue, never here.
 */
export class ApiError extends Error {
  readonly status: number
  readonly code: string | null

  constructor(status: number, code: string | null) {
    super(`API request failed: status=${status} code=${code ?? 'none'}`)
    this.name = 'ApiError'
    this.status = status
    this.code = code
  }
}

interface FetchFailure {
  status?: number
  statusCode?: number
  data?: { error?: unknown }
}

function toApiError(error: unknown): ApiError {
  if (error instanceof ApiError) return error
  if (error !== null && typeof error === 'object') {
    const failure = error as FetchFailure
    const status = failure.status ?? failure.statusCode
    if (typeof status === 'number') {
      const code = failure.data?.error
      return new ApiError(status, typeof code === 'string' ? code : null)
    }
  }
  return new ApiError(0, null)
}

interface RequestOptions<B extends Record<string, unknown>> {
  method?: 'GET' | 'POST' | 'PUT' | 'PATCH' | 'DELETE'
  body?: B
  query?: Record<string, string | undefined>
}

/**
 * A tiny JSON client bound to one relative base path.
 *
 * The base is always a literal relative path (`/api` or `/demo-merchant/api`,
 * see nuxt.config.ts) — never an absolute origin: the reverse proxy in
 * local/Caddyfile routes each prefix to its own backend, so the bundle carries
 * no hostname and every e2e scenario, each on its own host, can share one
 * prebuilt frontend container (scripts/check-static-bundle.mjs enforces this).
 */
export function createApiClient(base: string) {
  async function request<T, B extends Record<string, unknown> = Record<string, never>>(
    path: string,
    options: RequestOptions<B> = {},
  ): Promise<T> {
    try {
      return await $fetch<T>(`${base}${path}`, {
        method: options.method ?? 'GET',
        body: options.body,
        query: options.query,
        // Same-origin by construction, but stated rather than assumed: a
        // dashboard session is the HttpOnly `session` cookie and nothing about
        // that endpoint works without it.
        credentials: 'same-origin',
        headers: { accept: 'application/json' },
        // The e2e suite asserts on the exact set of requests a click makes
        // ("network request ... responded 201"); a silent retry would turn one
        // click into two requests.
        retry: false,
      })
    } catch (error) {
      throw toApiError(error)
    }
  }

  return { request }
}

/** paygate's own dashboard API, under `/api/v1`. */
export function useApi() {
  const { public: { API_URL } } = useRuntimeConfig()
  return createApiClient(`${API_URL}/v1`)
}
