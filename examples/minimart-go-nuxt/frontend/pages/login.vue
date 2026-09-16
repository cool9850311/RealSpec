<script setup lang="ts">
import { ApiError } from '~/composables/useApi'
import type { LoginRequest, LoginResponse } from '~/types/api'

definePageMeta({ layout: 'bare' })

const { t } = useI18n()
const api = useApi()
const session = useSession()

const username = ref('')
const password = ref('')
const errorKey = ref<string | null>(null)
const submitting = ref(false)

// The form is a real <form> with a real submit button, so the browser's own
// "Enter submits the form" behaviour does the work — which is exactly what
// login.feature exercises with `press "Enter"` from the password field. There
// is no keydown handler here; adding one would test a reimplementation of the
// accessible path instead of the accessible path.
async function submit(): Promise<void> {
  if (submitting.value) return
  submitting.value = true
  errorKey.value = null
  try {
    await api.request<LoginResponse, LoginRequest>('/auth/login', {
      method: 'POST',
      body: { username: username.value, password: password.value },
    })
  } catch (error) {
    errorKey.value =
      error instanceof ApiError && error.status === 401
        ? 'auth.invalidCredentials'
        : 'auth.unexpectedError'
    return
  } finally {
    submitting.value = false
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
  await navigateTo('/products')
}
</script>

<template>
  <section class="card">
    <h1 class="card__title">{{ t('auth.title') }}</h1>

    <form class="form" novalidate @submit.prevent="submit">
      <div class="form__field">
        <label class="form__label" for="username">{{ t('auth.username') }}</label>
        <input
          id="username"
          v-model="username"
          class="form__input"
          type="text"
          name="username"
          autocomplete="username"
          autocapitalize="none"
          spellcheck="false"
        >
      </div>

      <div class="form__field">
        <label class="form__label" for="password">{{ t('auth.password') }}</label>
        <input
          id="password"
          v-model="password"
          class="form__input"
          type="password"
          name="password"
          autocomplete="current-password"
        >
      </div>

      <button class="button button--primary" type="submit" :aria-busy="submitting">
        {{ t('auth.signIn') }}
      </button>

      <p v-if="errorKey" class="alert" role="alert" data-testid="login-error">{{ t(errorKey) }}</p>
    </form>
  </section>
</template>
