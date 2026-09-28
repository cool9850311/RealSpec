<script setup lang="ts">
import { ApiError } from '~/composables/useApi'
import type { DashboardUser, LoginRequest } from '~/types/api'

definePageMeta({ layout: 'bare' })

const { t } = useI18n()
const api = useApi()
const session = useSession()

const email = ref('')
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
    const me = await api.request<DashboardUser, LoginRequest>('/dashboard/login', {
      method: 'POST',
      body: { email: email.value, password: password.value },
    })
    // The response body already IS the signed-in user (spec/openapi/openapi.yaml,
    // dashboardLogin's 200), so the reports page can render the merchant's name
    // on its first frame without a second round trip to /dashboard/me.
    session.adopt(me)
  } catch (error) {
    errorKey.value =
      error instanceof ApiError && error.status === 401
        ? 'auth.invalidCredentials'
        : 'auth.unexpectedError'
    return
  } finally {
    submitting.value = false
  }

  await navigateTo('/reports')
}
</script>

<template>
  <section class="card">
    <h1 class="card__title">{{ t('auth.signIn') }}</h1>

    <form class="form" novalidate @submit.prevent="submit">
      <div class="form__field">
        <label class="form__label" for="email">{{ t('auth.email') }}</label>
        <input
          id="email"
          v-model="email"
          class="form__input"
          type="email"
          name="email"
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
