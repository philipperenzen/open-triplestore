<script>
  // Map widget for ```map blocks: WGS84 WKT features on the shared GeoPreview
  // (Leaflet) plus a legend that links labelled features to their resource page.
  import { t } from 'svelte-i18n';
  import GeoPreview from '../GeoPreview.svelte';
  import { navigate } from '../../lib/router/index.js';
  import { parseWktGeometry } from '../../lib/ontology/valueType.js';
  import { MapPin } from 'lucide-svelte';

  /** @type {Array<{wkt: string, label?: string, iri?: string}>} */
  export let features = [];

  $: valid = (features || []).filter((f) => parseWktGeometry(f.wkt));
  $: labelled = valid.filter((f) => f.label || f.iri);

  function open(iri) {
    navigate(`/resource?iri=${encodeURIComponent(iri)}`);
  }
</script>

<div class="map-block">
  {#if valid.length}
    <GeoPreview wkts={valid.map((f) => f.wkt)} height="260px" />
    <div class="legend-row">
      <span class="count"><MapPin size={11} /> {$t('components.chat.mapFeatures', { values: { count: valid.length } })}</span>
      {#if labelled.length}
        <ul class="legend">
          {#each labelled as f}
            <li>
              {#if f.iri}
                <button class="link" title={f.iri} on:click={() => open(f.iri)}>{f.label || f.iri}</button>
              {:else}
                <span>{f.label}</span>
              {/if}
            </li>
          {/each}
        </ul>
      {/if}
    </div>
  {:else}
    <div class="empty">{$t('components.chat.noGeometry')}</div>
  {/if}
</div>

<style>
  .map-block { margin: 0 0 0.55rem; }
  .legend-row { display: flex; align-items: baseline; gap: 0.6rem; margin-top: 0.3rem; flex-wrap: wrap; }
  .count {
    display: inline-flex; align-items: center; gap: 0.25rem;
    font-size: 0.72rem; color: var(--ink-400); white-space: nowrap;
  }
  .legend {
    list-style: none; display: flex; flex-wrap: wrap; gap: 0.2rem 0.7rem;
    margin: 0; padding: 0; font-size: 0.74rem; color: var(--ink-600);
  }
  .link {
    background: none; border: none; cursor: pointer; padding: 0; font-size: 0.74rem;
    color: #4f46e5; text-decoration: underline; text-decoration-color: rgba(79,70,229,0.35);
  }
  .link:hover { text-decoration-color: currentColor; }
  .empty {
    padding: 0.75rem; border: 1px dashed var(--line-strong); border-radius: 10px;
    font-size: 0.78rem; color: var(--ink-400);
  }
  :global(:is([data-theme="dark"], .dark)) .link { color: #a5b4fc; }
</style>
