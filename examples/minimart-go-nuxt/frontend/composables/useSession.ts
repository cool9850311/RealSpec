import { ApiError } from '~/composables/useApi'
import { SESSION_HINT_COOKIE, type Me } from '~/types/api'
import { deleteCookie, readCookie } from '~/utils/cookie'

/**
 * Deduplicates the `GET /api/v1/me` probe within one document load. Module
 * scope is safe here because `load()` returns early during prerendering: in the
 * browser this module is per tab, and on the server it is never written.
 */
let inflight: Promise<Me | null> | null = null

/** Whether this browser is holding the readable flag that login sets. */
function hasSessionHint(): boolean {
  return readCookie(SESSION_HINT_COOKIE) !== null
}

/**
 * Who is signed in, as far as this tab knows.
 *
 * The session itself is the HttpOnly `token` cookie, which script cannot read,
 * so the profile can only come from the API: `GET /api/v1/me` answers 200 with
 * it, or 401 when there is no usable cookie.
 *
 * WHY THE PROBE IS CONDITIONAL
 * ----------------------------
 * An anonymous visitor has no session to fetch, and asking anyway has a cost
 * that is easy to miss: the browser records every failed request in its own
 * console at error level ("Failed to load resource: the server responded with a
 * status of 401"), so an ordinary anonymous page load would leave an error
 * behind. That makes a developer's console untrustworthy and it makes the
 * `console has no errors` step of spec/bdd/format.yml unimplementable without
 * weakening it — the assertion would have to excuse a class of message that
 * also carries real failures.
 *
 * So the server sets a second, non-HttpOnly cookie beside the token
 * (`session_hint`, see middleware.SetSessionCookies). It grants nothing: the
 * API never reads it, and a forged one buys a 401 and nothing else. It answers
 * exactly one question — "is there any point in asking?" — which is the one
 * question script cannot answer about an HttpOnly cookie. No hint means no
 * probe, no 401 and no console error. A stale hint costs one 401, which expires
 * it on both sides: the server clears it with the response, and `clear()` below
 * removes it here.
 */
export function useSession() {
  const user = useState<Me | null>('session.user', () => null)
  const resolved = useState<boolean>('session.resolved', () => false)
  const api = useApi()

  function adopt(me: Me | null): void {
    user.value = me
    resolved.value = true
  }

  /**
   * Asks the API who the cookie belongs to, once per document load.
   *
   * Unconditional: the caller is stating that a session may have just come into
   * existence (the login page has one, one line after the credentials were
   * accepted), which is precisely the moment the hint cannot be trusted to be
   * visible yet.
   */
  async function refresh(): Promise<Me | null> {
    if (!inflight) {
      inflight = api
        .request<Me>('/me')
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
   * The profile of whoever is signed in, or null — without a request when this
   * browser was never given a session.
   */
  async function load(): Promise<Me | null> {
    // Prerendering has no cookies and no API; the browser resolves the session.
    if (import.meta.server) return null
    if (resolved.value) return user.value
    if (!hasSessionHint()) {
      adopt(null)
      return null
    }
    return refresh()
  }

  /** Records the balance the API reported after a redemption committed. */
  function setPoints(points: number): void {
    if (user.value) {
      user.value = { ...user.value, points }
    }
  }

  /**
   * Forgets the session: both the profile this tab holds and the hint that says
   * asking for it is worthwhile. Called when the API has refused a request with
   * 401, and by anything that signs the visitor out.
   */
  function clear(): void {
    deleteCookie(SESSION_HINT_COOKIE)
    adopt(null)
  }

  return { user, resolved, load, refresh, adopt, setPoints, clear }
}
