<template>
  <!--
    The chrome-free layout, used by /login. It carries no navigation and no
    balance on purpose: an anonymous visitor has nowhere to navigate to and no
    balance to show, and login.feature asserts the absence of the latter.
  -->
  <div class="app app--bare">
    <main class="app__main app__main--centred">
      <slot />
    </main>
  </div>
</template>
