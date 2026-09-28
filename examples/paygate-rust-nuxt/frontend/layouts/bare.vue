<template>
  <!--
    The chrome-free layout: /login (an anonymous visitor has nowhere to
    navigate to) and every demo-merchant page (/shop, /shop/pay/…,
    /shop/result — a separate storefront, not paygate's dashboard, so it
    carries no dashboard navigation and no merchant-name element).
  -->
  <div class="app app--bare">
    <main class="app__main app__main--centred">
      <slot />
    </main>
  </div>
</template>
