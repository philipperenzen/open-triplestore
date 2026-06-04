<script lang="ts">
  import { onMount, setContext } from 'svelte';
  import { location, ensureRouterListener } from '../locationStore.js';

  setContext('router_location', location);

  onMount(() => {
    ensureRouterListener();
  });
</script>

<slot />
