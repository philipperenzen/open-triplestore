<script>
  // Sticky in-page anchor navigation for long detail pages. Renders nothing
  // until at least three sections are visible — a two-section page doesn't
  // need a nav. Targets are plain `id` attributes on section wrappers (give
  // them `scroll-margin-top` so the sticky bar doesn't cover the heading).
  import { onMount, tick } from 'svelte';

  /** @type {{ id: string, label: string, visible?: boolean }[]} */
  export let sections = [];

  $: shown = sections.filter((s) => s.visible !== false);

  function goTo(id) {
    const el = document.getElementById(id);
    if (!el) return;
    el.scrollIntoView({ behavior: 'smooth', block: 'start' });
    history.replaceState(null, '', `#${id}`);
  }

  // Honor an incoming #hash deep-link once the page's first data batch has
  // rendered the target sections.
  onMount(async () => {
    const hash = location.hash.replace(/^#/, '');
    if (!hash) return;
    await tick();
    const el = document.getElementById(hash);
    if (el) el.scrollIntoView({ block: 'start' });
  });
</script>

{#if shown.length >= 3}
  <nav class="section-nav" aria-label="Page sections">
    {#each shown as s (s.id)}
      <button type="button" class="sn-pill" on:click={() => goTo(s.id)}>{s.label}</button>
    {/each}
  </nav>
{/if}

<style>
  .section-nav {
    position: sticky;
    top: 0;
    z-index: 5;
    display: flex;
    gap: 0.35rem;
    padding: 0.5rem 0.25rem;
    margin: 0 -0.25rem;
    background: var(--bg-elevated);
    backdrop-filter: blur(var(--glass-blur, 8px));
    border-bottom: 1px solid var(--line-soft);
    /* Long pages on narrow screens: the pills scroll sideways instead of wrapping
       into a bar tall enough to eat the viewport. */
    overflow-x: auto;
    scrollbar-width: none;
  }
  .section-nav::-webkit-scrollbar {
    display: none;
  }
  .sn-pill {
    flex-shrink: 0;
    border: 1px solid var(--line-soft);
    background: transparent;
    color: var(--ink-600);
    font: inherit;
    font-size: 0.78rem;
    padding: 0.28rem 0.75rem;
    border-radius: 999px;
    cursor: pointer;
    transition:
      background 0.12s,
      color 0.12s,
      border-color 0.12s;
  }
  .sn-pill:hover {
    background: var(--bg-soft);
    color: var(--ink-800);
    border-color: var(--line-strong);
  }
</style>
