<script setup lang="ts">
const { t } = useI18n()

// There is no content at the root: the catalogue is the front door. The
// redirect happens after mount so that the prerendered file on disk stays a
// plain document — the static server has no rewrite rules and no redirect
// support, and every route the app can reach has to be a file.
onMounted(() => {
  void navigateTo('/products', { replace: true })
})
</script>

<template>
  <p class="page__status">{{ t('app.redirecting') }}</p>
</template>
