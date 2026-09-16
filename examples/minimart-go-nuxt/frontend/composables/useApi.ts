import { isApiErrorCode, type ApiErrorCode } from '~/types/api'

/**
 * A response the API refused, or a request that never got one.
 *
 * `status` is the HTTP status, or 0 when the request failed at the transport
 * level. `code` is the stable identifier from the body — `{"error": "<CODE>"}`
 * — and is null when the body was not one of those. Human text for a code lives
 * in the i18n catalogue, never here (spec/openapi/openapi.yaml, schema Error).
 */
export class ApiError extends Error {
  readonly status: number
  readonly code: ApiErrorCode | null

  constructor(status: number, code: ApiErrorCode | null) {
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
      return new ApiError(status, isApiErrorCode(code) ? code : null)
    }
  }
  return new ApiError(0, null)
}

interface RequestOptions<B extends Record<string, unknown>> {
  method?: 'GET' | 'POST'
  body?: B
}

/**
 * The one place that knows where the API is.
 *
 * The base is `runtimeConfig.public.API_URL`, which is the literal relative
 * path `/api` (see nuxt.config.ts): the reverse proxy in local/Caddyfile routes
 * `/api/*` to the backend, so the bundle carries no hostname and every e2e
 * scenario — each on its own host — can share one prebuilt container.
 */
export function useApi() {
  const { public: { API_URL } } = useRuntimeConfig()

  async function request<T, B extends Record<string, unknown> = Record<string, never>>(
    path: string,
    options: RequestOptions<B> = {},
  ): Promise<T> {
    try {
      return await $fetch<T>(`${API_URL}/v1${path}`, {
        method: options.method ?? 'GET',
        body: options.body,
        // Same-origin by construction, but stated rather than assumed: the
        // session is the HttpOnly `token` cookie and nothing works without it.
        credentials: 'same-origin',
        headers: { accept: 'application/json' },
        // The e2e suite asserts on the exact set of requests a click makes
        // ("network request ... responded 201"); a silent retry would turn one
        // redemption into two.
        retry: false,
      })
    } catch (error) {
      throw toApiError(error)
    }
  }

  return { request }
}
