<script setup lang="ts">
const { t } = useI18n()
const { merchant, signOut } = useSession()
</script>

<template>
  <div class="app">
    <header class="app__header">
      <span class="app__brand">{{ t('app.name') }}</span>

      <!--
        Rendered only once the merchant is known — which is also what makes it
        a usable "signed in" assertion (login.feature: `text of
        "testid:merchant-name" is "Acme Coffee"`). The element holds the name
        and nothing else, because the assertion compares its whole text.
      -->
      <span v-if="merchant" class="app__merchant" data-testid="merchant-name">{{ merchant.name }}</span>

      <button
        v-if="merchant"
        class="button app__sign-out"
        type="button"
        @click="signOut"
      >
        {{ t('nav.signOut') }}
      </button>
    </header>

    <main class="app__main">
      <slot />
    </main>
  </div>
</template>
