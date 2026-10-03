<script lang="ts">
  import { getContext, onDestroy } from 'svelte';
  import { matchPath } from './match';
  import type { Readable } from 'svelte/store';

  export let path: string = '/';
  export let component: any = undefined;

  interface Location { pathname: string }
  const loc = getContext<Readable<Location>>('router_location');

  // Tell the router which pattern this Route serves, so a <Fallback> can tell
  // when no Route matches the current path (the not-found page).
  type Register = (_pattern: string) => () => void;
  const register = getContext<Register | undefined>('router_register');
  const unregister = register ? register(path) : () => {};
  onDestroy(unregister);

  $: params = $loc ? matchPath(path, $loc.pathname) : null;
  $: active = params !== null;
</script>

{#if active}
  {#if component}
    <svelte:component this={component} />
  {:else}
    <slot params={params ?? {}} />
  {/if}
{/if}
