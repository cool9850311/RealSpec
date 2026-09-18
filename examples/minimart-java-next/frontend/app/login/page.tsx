'use client'

import { useRouter } from 'next/navigation'
import { useTranslations } from 'next-intl'
import { useRef, useState, type FormEvent } from 'react'
import type { MessageKey } from '@/i18n'
import { ApiError, apiRequest } from '@/lib/api'
import { useSession } from '@/lib/session'
import type { LoginRequest, LoginResponse } from '@/lib/types'

export default function LoginPage() {
  const t = useTranslations()
  const router = useRouter()
  const session = useSession()

  const [username, setUsername] = useState('')
  const [password, setPassword] = useState('')
  const [errorKey, setErrorKey] = useState<MessageKey | null>(null)
  const [submitting, setSubmitting] = useState(false)
  // The guard against a second submit reads a ref, not the state: two submits
  // in one frame (a double click, Enter then click) both see the state value
  // of the render they were dispatched from.
  const submittingRef = useRef(false)

  // The form is a real <form> with a real submit button, so the browser's own
  // "Enter submits the form" behaviour does the work — which is exactly what
  // login.feature exercises with `press "Enter"` from the password field. There
  // is no keydown handler here; adding one would test a reimplementation of the
  // accessible path instead of the accessible path.
  async function submit(event: FormEvent<HTMLFormElement>): Promise<void> {
    event.preventDefault()
    if (submittingRef.current) return
    submittingRef.current = true
    setSubmitting(true)
    setErrorKey(null)
    try {
      await apiRequest<LoginResponse, LoginRequest>('/auth/login', {
        method: 'POST',
        body: { username, password },
      })
    } catch (error) {
      setErrorKey(
        error instanceof ApiError && error.status === 401
          ? 'auth.invalidCredentials'
          : 'auth.unexpectedError',
      )
      return
    } finally {
      submittingRef.current = false
      setSubmitting(false)
    }

    // The cookie is set. Read the profile before leaving, for two reasons: the
    // catalogue can then render the balance on its first frame instead of after a
    // second round trip, and the session state this tab already holds may be a
    // stale "nobody is signed in" — the visitor who was bounced here from /orders
    // resolved exactly that answer a moment ago, and a cached negative must not
    // outlive the login that disproved it.
    //
    // A failure here is not a failed login: the cookie exists either way, so the
    // visitor is sent on and the catalogue asks again.
    await session.refresh().catch(() => undefined)
    router.push('/products')
  }

  return (
    <section className="card">
      <h1 className="card__title">{t('auth.title')}</h1>

      <form className="form" noValidate onSubmit={(event) => void submit(event)}>
        <div className="form__field">
          <label className="form__label" htmlFor="username">
            {t('auth.username')}
          </label>
          <input
            id="username"
            className="form__input"
            type="text"
            name="username"
            autoComplete="username"
            autoCapitalize="none"
            spellCheck={false}
            value={username}
            onChange={(event) => setUsername(event.target.value)}
          />
        </div>

        <div className="form__field">
          <label className="form__label" htmlFor="password">
            {t('auth.password')}
          </label>
          <input
            id="password"
            className="form__input"
            type="password"
            name="password"
            autoComplete="current-password"
            value={password}
            onChange={(event) => setPassword(event.target.value)}
          />
        </div>

        <button className="button button--primary" type="submit" aria-busy={submitting}>
          {t('auth.signIn')}
        </button>

        {errorKey && (
          <p className="alert" role="alert" data-testid="login-error">
            {t(errorKey)}
          </p>
        )}
      </form>
    </section>
  )
}
