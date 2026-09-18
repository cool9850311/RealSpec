import { isApiErrorCode, type ApiErrorCode } from '@/lib/types'

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

/**
 * The API base: the literal relative path `/api`.
 *
 * Not configurable, and deliberately not `process.env.NEXT_PUBLIC_API_URL ??
 * '/api'`: a build-time environment variable is exactly how an absolute origin
 * would get baked into the bundle, and one prebuilt frontend container is
 * shared by every e2e scenario, each on its own published port. The reverse
 * proxy in local/Caddyfile routes `/api/*` to the backend, so relative to
 * whatever origin served the page is always right.
 * `scripts/check-static-bundle.mjs` fails the build if that ever stops being
 * true.
 */
const API_BASE = '/api'

interface RequestOptions<B extends Record<string, unknown>> {
  method?: 'GET' | 'POST'
  body?: B
}

/** The `error` field of a JSON error body, or null for any other body. */
async function readErrorCode(response: Response): Promise<ApiErrorCode | null> {
  try {
    const body: unknown = await response.json()
    if (body !== null && typeof body === 'object') {
      const code = (body as { error?: unknown }).error
      return isApiErrorCode(code) ? code : null
    }
  } catch {
    // Not JSON — a proxy's HTML error page, an empty 502. The status alone
    // is still an answer.
  }
  return null
}

/**
 * The one place that knows where the API is.
 *
 * Plain `fetch`, so there is no retry anywhere in the path: the e2e suite
 * asserts on the exact set of requests a click makes ("network request ...
 * responded 201"), and a silent retry would turn one redemption into two.
 */
export async function apiRequest<T, B extends Record<string, unknown> = Record<string, never>>(
  path: string,
  options: RequestOptions<B> = {},
): Promise<T> {
  const headers: Record<string, string> = { accept: 'application/json' }
  let body: string | undefined
  if (options.body !== undefined) {
    headers['content-type'] = 'application/json'
    body = JSON.stringify(options.body)
  }

  let response: Response
  try {
    response = await fetch(`${API_BASE}/v1${path}`, {
      method: options.method ?? 'GET',
      body,
      // Same-origin by construction, but stated rather than assumed: the
      // session is the HttpOnly `token` cookie and nothing works without it.
      credentials: 'same-origin',
      headers,
    })
  } catch {
    // fetch rejects only when no response arrived at all.
    throw new ApiError(0, null)
  }

  if (!response.ok) {
    throw new ApiError(response.status, await readErrorCode(response))
  }
  try {
    return (await response.json()) as T
  } catch {
    // A 2xx whose body is not the JSON the contract promises is not a success
    // the caller can use. The status is kept, and no code is invented.
    throw new ApiError(response.status, null)
  }
}
