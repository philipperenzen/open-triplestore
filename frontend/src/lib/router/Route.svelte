<script lang="ts">
  import { getContext } from 'svelte';
  import type { Readable } from 'svelte/store';

  export let path: string = '/';
  export let component: any = undefined;

  interface Location { pathname: string }
  const loc = getContext<Readable<Location>>('router_location');

  function matchPath(pattern: string, pathname: string): Record<string, string> | null {
    const paramNames: string[] = [];
    const regexStr =
      '^' +
      pattern
        .replace(/[.+*?^${}()|[\]\\]/g, '\\$&')
        .replace(/:([a-zA-Z_][a-zA-Z0-9_]*)/g, (_, name: string) => {
          paramNames.push(name);
          return '([^/]+)';
        }) +
      '$';
    const match = pathname.match(new RegExp(regexStr));
    if (!match) return null;
    const params: Record<string, string> = {};
    paramNames.forEach((name, i) => { params[name] = match[i + 1]; });
    return params;
  }

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
