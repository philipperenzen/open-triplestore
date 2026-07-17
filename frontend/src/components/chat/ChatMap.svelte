<script>
  // Map widget for ```map blocks: WGS84 WKT features on the shared GeoPreview
  // (Leaflet) plus a legend that links labelled features to their resource page.
  // When the spec carries validated 3D "models" (see chatRich.parseMapSpec),
  // the georeferenced MapLibre viewer (ViewerMap) renders instead, standing
  // each model on the map at its WKT POINT anchor — ViewerMap pulls maplibre +
  // three, so it is imported on demand and plain feature maps never pay for it.
  import { t } from 'svelte-i18n';
  import GeoPreview from '../GeoPreview.svelte';
  import { navigate } from '../../lib/router/index.js';
  import { parseWktGeometry } from '../../lib/ontology/valueType.js';
  import { MapPin, Boxes } from 'lucide-svelte';

  const viewerMap = () => import('../viewer/ViewerMap.svelte');

  /** @type {Array<{wkt: string, label?: string, iri?: string}>} */
  export let features = [];
  /** @type {Array<{label: string, url: string, format: string, wkt: string}>} */
  export let models = [];

  $: valid = (features || []).filter((f) => parseWktGeometry(f.wkt));
  $: labelled = valid.filter((f) => f.label || f.iri);

  // ViewerMap elements: each model carries a FOG-style files entry
  // ([formatKey, url] — detect.modelRefOf maps the plain format key back) and
  // its POINT anchor; plain features come along as vector-only elements.
  $: elements = (models || []).length
    ? [
        ...models.map((m, i) => ({
          id: `model-${i}`,
          label: m.label || '',
          types: [],
          wkt4326: m.wkt,
          files: [[m.format, m.url]],
        })),
        ...valid.map((f, i) => ({
          id: f.iri || `feature-${i}`,
          label: f.label || '',
          types: [],
          wkt4326: f.wkt,
        })),
      ]
    : [];

  function open(iri) {
    navigate(`/resource?iri=${encodeURIComponent(iri)}`);
  }

  // Selecting a feature element on the 3D map opens its resource page (the
  // element id is the feature iri when one was given).
  function onSelect(e) {
    const id = e.detail?.id || '';
    if (/^https?:\/\//i.test(id)) open(id);
  }
</script>

<div class="map-block">
  {#if elements.length}
    <div class="viewer-stage">
      {#await viewerMap()}
        <div class="placeholder">
          {$t('components.chat.preparingWidget', { values: { label: $t('components.chat.pendingMap') } })}
        </div>
      {:then mod}
        <svelte:component this={mod.default} {elements} height="100%" on:select={onSelect} />
      {:catch}
        <div class="placeholder">{$t('components.chat.model3dFailed')}</div>
      {/await}
    </div>
    <div class="legend-row">
      <span class="count"><Boxes size={11} /> {models.length} {$t('components.chat.mapModels')}</span>
      {#if valid.length}
        <span class="count"><MapPin size={11} /> {$t('components.chat.mapFeatures', { values: { count: valid.length } })}</span>
      {/if}
      <ul class="legend">
        {#each models.filter((m) => m.label) as m}
          <li><span title={m.url}>{m.label}</span></li>
        {/each}
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
    </div>
  {:else if valid.length}
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
  .viewer-stage {
    height: 320px; border-radius: 10px; overflow: hidden;
    border: 1px solid var(--line-soft, #e5e7eb);
  }
  .placeholder {
    height: 100%; display: flex; align-items: center; justify-content: center;
    font-size: 0.78rem; color: var(--ink-400); background: var(--bg-soft, #f1f5f9);
  }
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
