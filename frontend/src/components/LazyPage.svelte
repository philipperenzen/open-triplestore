<script lang="ts">
  /**
   * LazyPage.svelte — W4-20: lazy-load a page component on first render.
   *
   * Usage:
   *   <LazyPage loader={() => import('./pages/SparqlEditor.svelte')} prop1={val} />
   *
   * The page chunk is fetched from the server only when this component is first
   * mounted, keeping the initial bundle small.
   */
  import { onMount } from 'svelte';

  interface Props {
    loader: () => Promise<{ default: any }>;
    [key: string]: unknown;
  }

  // LazyPage forwards arbitrary props to the lazily-loaded page; the rest element
  // is intentional and this component is never compiled as a custom element.
  // eslint-disable-next-line svelte/valid-compile
  let { loader, ...restProps }: Props = $props();

  let Component = $state(null);
  let loadError = $state(null);
  // Only reveal the "Loading…" state once the chunk fetch has run past the motion
  // threshold — a cached/already-fetched chunk resolves first and never flashes it.
  let showLoading = $state(false);

  onMount(async () => {
    const timer = setTimeout(() => { showLoading = true; }, 480);
    try {
      const mod = await loader();
      Component = mod.default;
    } catch (e) {
      loadError = e?.message ?? 'Failed to load page';
    } finally {
      clearTimeout(timer);
    }
  });
</script>

{#if loadError}
  <div class="card" style="color: var(--color-error, #c0392b); padding: 1.5rem;">
    Failed to load page: {loadError}
  </div>
{:else if Component}
  <Component {...restProps} />
{:else if showLoading}
  <div class="page-loading" aria-busy="true" aria-label="Loading page…" style="padding: 3rem; text-align: center; color: var(--ink-500);">
    Loading…
  </div>
{/if}
