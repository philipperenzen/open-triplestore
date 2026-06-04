<script>
  import { Link } from '../lib/router/index.js';

  /**
   * breadcrumbs — array of { label: string, href?: string }
   * The last item is always rendered as bc-current (no link).
   * If the array has only one item the <nav> is not rendered.
   */
  export let breadcrumbs = [];

  /** Page title shown in the h2. If empty and no actions slot, the row is omitted. */
  export let title = '';

  /** Lucide icon component class (optional). Pass the component, e.g. icon={Database} */
  /** @type {any} */
  export let icon = null;

  /** Optional count shown as a muted badge after the title */
  export let count = null;
</script>

<div class="page-header">
  {#if breadcrumbs.length > 1}
    <nav class="breadcrumb">
      {#each breadcrumbs as crumb, i}
        {#if i > 0}<span class="bc-sep">›</span>{/if}
        {#if crumb.href && i < breadcrumbs.length - 1}
          <Link to={crumb.href}>{crumb.label}</Link>
        {:else}
          <span class="bc-current">{crumb.label}</span>
        {/if}
      {/each}
    </nav>
  {/if}

  {#if title || $$slots.actions}
    <div class="page-header-row">
      <div class="page-header-left">
        {#if icon}
          <svelte:component this={icon} size={22} class="page-header-icon" />
        {/if}
        {#if title}<h2 class="page-title">{title}</h2>{/if}
        {#if count !== null && count !== undefined}
          <span class="page-count">{count}</span>
        {/if}
      </div>
      <div class="page-header-actions">
        <slot name="actions" />
      </div>
    </div>
  {/if}
</div>
