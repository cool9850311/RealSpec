import { ApiError } from '~/composables/useApi'
import { SESSION_HINT_COOKIE, type DashboardUser } from '~/types/api'
import { deleteCookie, readCookie } from '~/utils/cookie'

/**
 * Deduplicates the `GET /dashboard/me` probe within one document load. Module
 * scope is safe here: this app never runs on a server (`ssr: false`), so this
 * module only ever exists once, in the browser tab that loaded it.
 */
let inflight: Promise<DashboardUser | null> | null = null

/** Whether this browser is holding the readable flag that login sets. */
function hasSessionHint(): boolean {
  return readCookie(SESSION_HINT_COOKIE) !== null
}

/**
 * Who is signed in to the dashboard, as far as this tab knows.
 *
 * The session itself is the HttpOnly `session` cookie, which script cannot
 * read, so the profile can only come from the API: `GET /dashboard/me` answers
 * 200 with it, or 401 when there is no usable cookie.
 *
 * WHY THE PROBE IS CONDITIONAL
 * ----------------------------
 * An anonymous visitor has no session to fetch, and asking anyway has a cost
 * that is easy to miss: the browser records every failed request in its own
 * console at error level ("Failed to load resource: the server responded with
 * a status of 401"), which would make an ordinary anonymous visit to
 * `/reports` fail `console has no errors`. So `POST /dashboard/login` sets a
 * second, non-HttpOnly cookie beside the token (`session_hint`,
 * spec/openapi/openapi.yaml, `dashboardLogin`): it grants nothing — the API
 * never reads it — and answers exactly one question, "is there any point in
 * asking?", which is the one question script cannot answer about an HttpOnly
 * cookie. No hint means no probe, no 401 and no console error.
 */
export function useSession() {
  const user = useState<DashboardUser | null>('session.user', () => null)
  const resolved = useState<boolean>('session.resolved', () => false)
  const api = useApi()

  function adopt(me: DashboardUser | null): void {
    user.value = me
    resolved.value = true
  }

  /**
   * Asks the API who the cookie belongs to, once per document load.
   *
   * Unconditional: the caller is stating that a session may have just come
   * into existence (the login page has one, one line after the credentials
   * were accepted), which is precisely the moment the hint cannot be trusted
   * to be visible yet.
   */
  async function refresh(): Promise<DashboardUser | null> {
    if (!inflight) {
      inflight = api
        .request<DashboardUser>('/dashboard/me')
        .then((me) => {
          adopt(me)
          return me
        })
        .catch((error: unknown) => {
          if (error instanceof ApiError && error.status === 401) {
            clear()
            return null
          }
          throw error
        })
        .finally(() => {
          inflight = null
        })
    }
    return inflight
  }

  /**
   * The dashboard user signed in, or null — without a request when this
   * browser was never given a session.
   */
  async function load(): Promise<DashboardUser | null> {
    if (resolved.value) return user.value
    if (!hasSessionHint()) {
      adopt(null)
      return null
    }
    return refresh()
  }

  /**
   * Forgets the session: both the profile this tab holds and the hint that
   * says asking for it is worthwhile. Called when the API has refused a
   * request with 401, and by anything that signs the visitor out.
   */
  function clear(): void {
    deleteCookie(SESSION_HINT_COOKIE)
    adopt(null)
  }

  /**
   * Ends the session everywhere (`POST /dashboard/logout`) and returns to the
   * sign-in page. The cookie is expired by the API's response either way, so
   * the local state is cleared unconditionally rather than only on success —
   * a network failure here must not leave the visitor believing they are still
   * signed in.
   */
  async function signOut(): Promise<void> {
    try {
      await api.request<void>('/dashboard/logout', { method: 'POST' })
    } finally {
      clear()
      await navigateTo('/login')
    }
  }

  return {
    user,
    merchant: computed(() => user.value?.merchant ?? null),
    resolved,
    load,
    refresh,
    adopt,
    clear,
    signOut,
  }
}
