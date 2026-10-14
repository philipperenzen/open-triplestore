<script lang="ts">
  import { onMount, setContext } from 'svelte';
  import { writable } from 'svelte/store';
  import { location, ensureRouterListener } from '../locationStore.js';

  setContext('router_location', location);

  // Every mounted <Route> registers its pattern here (and drops it when it is
  // destroyed); <Fallback> renders only when none of them matches the path.
  // A count per pattern, because two Routes may share one (`/datasets/:id`).
  const patterns = writable<Map<string, number>>(new Map());
  setContext('router_patterns', patterns);
  setContext('router_register', (pattern: string) => {
    patterns.update((m) => new Map(m).set(pattern, (m.get(pattern) ?? 0) + 1));
    return () =>
      patterns.update((m) => {
        const next = new Map(m);
        const n = (next.get(pattern) ?? 1) - 1;
        if (n > 0) next.set(pattern, n);
        else next.delete(pattern);
        return next;
      });
  });

  onMount(() => {
    ensureRouterListener();
  });
</script>

<slot />
