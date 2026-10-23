<script>
  // Map view of a GeoSPARQL result in a chat answer, on the shared MapLibre
  // viewer (ViewerMap — the SPARQL workspace's Map tab). Elements come from
  // chatViz.mapElementsFromTable: rows with a WGS84 WKT cell (or a lat/lon
  // column pair); projected geometries are skipped rather than reprojected.
  // ViewerMap pulls maplibre + three, so it is imported on demand.
  import { t } from 'svelte-i18n';
  import { navigate } from '../../lib/router/index.js';
  import { MapPin } from 'lucide-svelte';

  /** @type {import('../../lib/viewer/geometry').ViewerElement[]} */
  export let elements = [];
  /** Rows in the result (some may carry no plottable geometry). */
  export let total = 0;

  const viewerMap = () => import('../viewer/ViewerMap.svelte');

  // Row elements without an IRI get a synthetic `row:N` id — nothing to open.
  function onSelect(e) {
    const id = e.detail?.id || '';
    if (/^(https?:\/\/|urn:)/i.test(id)) navigate(`/resource?iri=${encodeURIComponent(id)}`);
  }
</script>

<div class="map-block">
  {#if elements.length}
    <div class="stage" data-testid="chat-result-map">
      {#await viewerMap()}
        <div class="placeholder">
          {$t('components.chat.preparingWidget', { values: { label: $t('components.chat.pendingMap') } })}
        </div>
      {:then mod}
        <svelte:component this={mod.default} {elements} height="100%" on:select={onSelect} />
      {:catch}
        <div class="placeholder">{$t('components.chat.mapFailed')}</div>
      {/await}
    </div>
    <p class="note">
      <MapPin size={11} />
      {$t('components.chat.mapFeatures', { values: { count: elements.length } })}
      {#if total > elements.length}· {$t('components.chat.mapSkipped', { values: { count: total - elements.length } })}{/if}
    </p>
  {:else}
    <div class="empty">{$t('components.chat.noGeometry')}</div>
  {/if}
</div>

<style>
  .map-block { margin: 0 0 0.55rem; }
  .stage {
    height: 320px; border-radius: 10px; overflow: hidden;
    border: 1px solid var(--line-soft, #e5e7eb);
  }
  .placeholder {
    height: 100%; display: flex; align-items: center; justify-content: center;
    font-size: 0.78rem; color: var(--ink-400); background: var(--bg-soft, #f1f5f9);
  }
  .note {
    display: flex; align-items: center; gap: 0.3rem;
    margin: 0.3rem 0 0; font-size: 0.72rem; color: var(--ink-400);
  }
  .empty {
    padding: 0.75rem; border: 1px dashed var(--line-strong); border-radius: 10px;
    font-size: 0.78rem; color: var(--ink-400);
  }
</style>
