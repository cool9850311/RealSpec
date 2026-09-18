'use client'

import { createContext, useCallback, useContext, useMemo, useRef, useState, type ReactNode } from 'react'
import { ApiError, apiRequest } from '@/lib/api'
import { deleteCookie, readCookie } from '@/lib/cookie'
import { SESSION_HINT_COOKIE, type Me } from '@/lib/types'

/**
 * Deduplicates the `GET /api/v1/me` probe within one document load. Module
 * scope is safe here because nothing calls `refresh()` during the static
 * export's prerender — every caller is an effect or an event handler, and
 * neither runs at build time — so in practice this module is per tab.
 */
let inflight: Promise<Me | null> | null = null

/** Whether this browser is holding the readable flag that login sets. */
function hasSessionHint(): boolean {
  return readCookie(SESSION_HINT_COOKIE) !== null
}

export interface Session {
  /** The signed-in profile, or null for "nobody" and for "not asked yet". */
  user: Me | null
  /** Whether `user` is an answer rather than the initial null. */
  resolved: boolean
  load: () => Promise<Me | null>
  refresh: () => Promise<Me | null>
  setPoints: (points: number) => void
  clear: () => void
}

const SessionContext = createContext<Session | null>(null)

/**
 * Who is signed in, as far as this tab knows.
 *
 * The session itself is the HttpOnly `token` cookie, which script cannot read,
 * so the profile can only come from the API: `GET /api/v1/me` answers 200 with
 * it, or 401 when there is no usable cookie.
 *
 * Mounted once, in the root layout, so the answer survives client-side
 * navigation: the login page resolves the profile and the catalogue it pushes
 * to renders the balance from the same state on its first frame.
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
 * (`session_hint`, see spec.md, "Authentication"). It grants nothing: the API
 * never reads it, and a forged one buys a 401 and nothing else. It answers
 * exactly one question — "is there any point in asking?" — which is the one
 * question script cannot answer about an HttpOnly cookie. No hint means no
 * probe, no 401 and no console error. A stale hint costs one 401, which expires
 * it on both sides: the server clears it with the response, and `clear()` below
 * removes it here.
 */
export function SessionProvider({ children }: { children: ReactNode }) {
  const [user, setUser] = useState<Me | null>(null)
  const [resolved, setResolved] = useState(false)
  // Mirrors of the two states for synchronous reads. `load()` can be called by
  // two components in the same commit, before either state update has
  // re-rendered anything; reading the state variables there would see the
  // stale closure values and probe twice.
  const userRef = useRef<Me | null>(null)
  const resolvedRef = useRef(false)

  const adopt = useCallback((me: Me | null): void => {
    userRef.current = me
    resolvedRef.current = true
    setUser(me)
    setResolved(true)
  }, [])

  /**
   * Forgets the session: both the profile this tab holds and the hint that says
   * asking for it is worthwhile. Called when the API has refused a request with
   * 401, and by anything that signs the visitor out.
   */
  const clear = useCallback((): void => {
    deleteCookie(SESSION_HINT_COOKIE)
    adopt(null)
  }, [adopt])

  /**
   * Asks the API who the cookie belongs to, once per document load.
   *
   * Unconditional: the caller is stating that a session may have just come into
   * existence (the login page has one, one line after the credentials were
   * accepted), which is precisely the moment the hint cannot be trusted to be
   * visible yet.
   */
  const refresh = useCallback((): Promise<Me | null> => {
    if (!inflight) {
      inflight = apiRequest<Me>('/me')
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
  }, [adopt, clear])

  /**
   * The profile of whoever is signed in, or null — without a request when this
   * browser was never given a session.
   */
  const load = useCallback(async (): Promise<Me | null> => {
    if (resolvedRef.current) return userRef.current
    if (!hasSessionHint()) {
      adopt(null)
      return null
    }
    return refresh()
  }, [adopt, refresh])

  /** Records the balance the API reported after a redemption committed. */
  const setPoints = useCallback((points: number): void => {
    const current = userRef.current
    if (!current) return
    // A new object rather than a mutation: React re-renders on identity.
    const next = { ...current, points }
    userRef.current = next
    setUser(next)
  }, [])

  const value = useMemo<Session>(
    () => ({ user, resolved, load, refresh, setPoints, clear }),
    [user, resolved, load, refresh, setPoints, clear],
  )

  return <SessionContext.Provider value={value}>{children}</SessionContext.Provider>
}

export function useSession(): Session {
  const session = useContext(SessionContext)
  if (!session) {
    // A programming error, not a runtime condition: the provider wraps every
    // route in app/layout.tsx.
    throw new Error('useSession() was called outside <SessionProvider>')
  }
  return session
}
