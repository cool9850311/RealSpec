<script setup lang="ts">
const { t } = useI18n()
const { user } = useSession()
</script>

<template>
  <div class="app">
    <header class="app__header">
      <span class="app__brand">{{ t('app.name') }}</span>

      <nav class="app__nav" :aria-label="t('nav.primary')">
        <NuxtLink class="app__nav-link" to="/products">{{ t('nav.products') }}</NuxtLink>
        <NuxtLink class="app__nav-link" to="/orders">{{ t('nav.orders') }}</NuxtLink>
      </nav>

      <!--
        The balance is rendered only for a signed-in visitor, which is also what
        makes it a usable assertion: `"testid:points-balance" is hidden` is the
        e2e suite's way of saying "nobody is signed in". The element holds the
        number and nothing else, because the assertion compares its whole text.
      -->
      <p v-if="user" class="app__balance">
        <span class="app__balance-label">{{ t('session.points') }}</span>
        <span class="app__balance-value" data-testid="points-balance">{{ user.points }}</span>
      </p>
    </header>

    <main class="app__main">
      <slot />
    </main>
  </div>
</template>
