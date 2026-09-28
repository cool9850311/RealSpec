<script setup lang="ts">
const { t } = useI18n()

// No feature ever visits "/" — the e2e suite always starts at "/login" or
// "/shop" — but a static host still needs a document at the root, and a
// redirect after mount (rather than a prerendered rewrite) keeps that document
// a plain, inert file: this app's static file server has no rewrite rules of
// its own (nuxt.config.ts).
onMounted(() => {
  void navigateTo('/login', { replace: true })
})
</script>

<template>
  <p class="page__status">{{ t('app.name') }}</p>
</template>
