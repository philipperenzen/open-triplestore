<script lang="ts">
  import { t } from 'svelte-i18n';
  import { facetChips, type ContentFacets } from '../lib/contentFacets';

  /** The `facets` of a registry entry or version; nothing is shown until it is counted. */
  let { facets = null }: { facets?: Partial<ContentFacets> | null } = $props();

  let chips = $derived(facetChips(facets));
  let tooLarge = $derived(!!facets?.capped);
</script>

{#if chips.length || tooLarge}
  <div class="facet-chips" role="list" aria-label={$t('components.facetChips.label')}>
    {#each chips as chip (chip.key)}
      <span class="facet-chip facet-{chip.key}" role="listitem">
        {$t(`components.facetChips.${chip.key}`, { values: { count: chip.count } })}
      </span>
    {/each}
    {#if tooLarge}
      <span class="facet-chip facet-capped" role="listitem">{$t('components.facetChips.tooLarge')}</span>
    {/if}
  </div>
{/if}

<style>
  .facet-chips { display: inline-flex; flex-wrap: wrap; gap: 0.25rem; margin-top: 0.25rem; }
  .facet-chip {
    display: inline-block;
    padding: 0.0625rem 0.4375rem;
    border-radius: 999px;
    font-size: 0.6875rem;
    line-height: 1.25rem;
    background: var(--bg-soft, #f1f5f9);
    color: var(--ink-600, #475569);
    border: 1px solid var(--border, #e2e8f0);
    white-space: nowrap;
  }
  .facet-capped { font-style: italic; }
  :global(:is([data-theme="dark"], .dark)) .facet-chip { background: rgba(148, 163, 184, 0.12); color: var(--ink-300, #cbd5e1); border-color: rgba(148, 163, 184, 0.25); }
</style>
