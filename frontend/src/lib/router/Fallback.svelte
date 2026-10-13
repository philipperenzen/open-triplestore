<script lang="ts">
  // Renders its slot when no <Route> under the same <Router> matches the
  // current path: the catch-all for the not-found page. Place it after the
  // Routes, so they have registered their patterns by the time it renders.
  import { getContext } from 'svelte';
  import type { Readable } from 'svelte/store';
  import { matchPath } from './match';

  interface Location { pathname: string }
  const loc = getContext<Readable<Location>>('router_location');
  const patterns = getContext<Readable<Map<string, number>>>('router_patterns');

  $: unmatched =
    !!$loc && !!$patterns && ![...$patterns.keys()].some((p) => matchPath(p, $loc.pathname) !== null);
</script>

{#if unmatched}
  <slot />
{/if}
