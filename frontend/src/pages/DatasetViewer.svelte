<script>
  // Dataset geo data explorer. The map is the canvas: zoomed out every located
  // element is a dot; zooming in, elements with a 3D model become to-scale
  // model markers. Clicking a feature (or a list row) opens the element modal —
  // properties, BOT/IFC substructure (all navigable) and an interactive 3D
  // viewer. Datasets without any located element fall back to a pure 3D
  // explorer over their models. Light/dark follows the app theme.
  import { t as i18nT } from 'svelte-i18n';
  import { Link } from '../lib/router/index.js';
  import { getViewerFeed } from '../lib/api.js';
  import { shortenIRI } from '../lib/rdf-utils.js';
  import { ChevronLeft, Search, Boxes, MapPin } from 'lucide-svelte';
  import { modelRefOf } from '../lib/viewer/detect';
  import { modelRefs } from '../lib/viewer/geometry';
  import ViewerMap from '../components/viewer/ViewerMap.svelte';
  import Model3D from '../components/viewer/Model3D.svelte';
  import ElementModal from '../components/viewer/ElementModal.svelte';

  export let id = '';

  let elements = [];
  let loading = true;
  let error = '';
  let selected = '';
  let modalElement = null;
  let query = '';
  let mapComponent;

  $: filtered = query
    ? elements.filter((e) =>
        (e.label || e.id).toLowerCase().includes(query.toLowerCase())
      )
    : elements;
  $: located = filtered.filter((e) => e.wkt4326);
  $: unlocated = filtered.filter((e) => !e.wkt4326);
  $: hasGeo = elements.some((e) => e.wkt4326);
  $: fallbackRefs = modelRefs(elements);

  const hasModel = (el) => !!modelRefOf(el);

  async function load() {
    loading = true;
    error = '';
    try {
      const data = await getViewerFeed(id);
      elements = data?.elements || [];
    } catch (e) {
      error = e?.message || 'failed';
    } finally {
      loading = false;
    }
  }

  function open(elId, { fly = true } = {}) {
    selected = elId;
    modalElement = elements.find((e) => e.id === elId) || null;
    if (fly && modalElement?.wkt4326) mapComponent?.focusElement(elId);
  }

  function onMapSelect(event) {
    open(event.detail.id, { fly: false });
  }

  load();
</script>

<div class="page explorer-page">
  <div class="page-head">
    <Link to={`/datasets/${id}`} class="btn btn-sm">
      <ChevronLeft size={16} />
      {$i18nT('pages.datasetViewer.back')}
    </Link>
    <h1>{$i18nT('pages.datasetViewer.title')}</h1>
    {#if !loading && elements.length}
      <span class="count-chip">{elements.length} {$i18nT('pages.datasetViewer.elements').toLowerCase()}</span>
    {/if}
  </div>

  {#if loading}
    <p class="hint">…</p>
  {:else if error}
    <p class="hint error">{error}</p>
  {:else if elements.length === 0}
    <p class="hint">{$i18nT('pages.datasetViewer.empty')}</p>
  {:else}
    <div class="explorer">
      <aside class="side card-flat">
        <label class="search">
          <Search size={14} />
          <input
            type="search"
            placeholder={$i18nT('viewer.search')}
            bind:value={query}
            aria-label={$i18nT('viewer.search')}
          />
        </label>
        <div class="list-scroll">
          {#if located.length}
            <div class="group-label"><MapPin size={12} /> {$i18nT('viewer.located')}</div>
            <ul>
              {#each located as el}
                <li>
                  <button class:active={el.id === selected} on:click={() => open(el.id)} title={el.id}>
                    <span class="label">{el.label || shortenIRI(el.id)}</span>
                    {#if hasModel(el)}<span class="badge">3D</span>{/if}
                  </button>
                </li>
              {/each}
            </ul>
          {/if}
          {#if unlocated.length}
            <div class="group-label"><Boxes size={12} /> {$i18nT('viewer.noLocation')}</div>
            <ul>
              {#each unlocated as el}
                <li>
                  <button class:active={el.id === selected} on:click={() => open(el.id, { fly: false })} title={el.id}>
                    <span class="label">{el.label || shortenIRI(el.id)}</span>
                    {#if hasModel(el)}<span class="badge">3D</span>{/if}
                  </button>
                </li>
              {/each}
            </ul>
          {/if}
        </div>
        <p class="side-hint">{$i18nT('viewer.zoomHint')}</p>
      </aside>

      <section class="canvas card-flat">
        {#if hasGeo}
          <ViewerMap
            bind:this={mapComponent}
            {elements}
            {selected}
            on:select={onMapSelect}
            height="100%"
          />
        {:else}
          <Model3D refs={fallbackRefs} {selected} on:select={onMapSelect} height="100%" />
        {/if}
      </section>
    </div>
  {/if}
</div>

<ElementModal
  element={modalElement}
  {elements}
  datasetId={id}
  on:close={() => (modalElement = null)}
  on:navigate={(e) => open(e.detail.id)}
/>

<style>
  .explorer-page {
    display: flex;
    flex-direction: column;
    height: calc(100vh - 90px);
  }
  .page-head {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    margin-bottom: 0.6rem;
  }
  .page-head h1 {
    margin: 0;
    font-size: 1.2rem;
    color: var(--ink-900, #0f172a);
  }
  .count-chip {
    font-size: 0.74rem;
    padding: 2px 10px;
    border-radius: 99px;
    background: var(--bg-soft, #f1f5f9);
    color: var(--muted, #64748b);
  }
  .explorer {
    flex: 1;
    min-height: 0;
    display: grid;
    grid-template-columns: 290px 1fr;
    gap: 0.6rem;
  }
  .side,
  .canvas {
    display: flex;
    flex-direction: column;
    min-height: 0;
    border: 1px solid var(--border, #e2e8f0);
    border-radius: var(--radius-lg, 12px);
    background: var(--bg-elevated, #fff);
    overflow: hidden;
  }
  .search {
    display: flex;
    align-items: center;
    gap: 6px;
    margin: 10px;
    padding: 6px 10px;
    border: 1px solid var(--line-soft, #e6eaef);
    border-radius: var(--radius-md, 9px);
    background: var(--bg, #fff);
    color: var(--muted, #64748b);
  }
  .search input {
    flex: 1;
    border: 0;
    outline: 0;
    background: transparent;
    font-size: 0.85rem;
    color: var(--ink-900, #0f172a);
  }
  .list-scroll {
    flex: 1;
    overflow: auto;
    padding: 0 8px 8px;
  }
  .group-label {
    display: flex;
    align-items: center;
    gap: 5px;
    font-size: 0.68rem;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--muted, #64748b);
    padding: 8px 6px 4px;
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  li > button {
    width: 100%;
    text-align: left;
    border: 0;
    background: transparent;
    padding: 6px 8px;
    border-radius: var(--radius-sm, 7px);
    cursor: pointer;
    font-size: 0.86rem;
    display: flex;
    align-items: center;
    gap: 0.45rem;
    color: var(--ink-900, #0f172a);
  }
  li > button:hover {
    background: var(--bg-hover, rgba(0, 0, 0, 0.04));
  }
  li > button.active {
    background: var(--bg-accent-soft, #e7f0fb);
    font-weight: 600;
  }
  li .label {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .badge {
    font-size: 0.62rem;
    padding: 0 6px;
    border-radius: 99px;
    background: rgba(232, 89, 12, 0.13);
    color: #e8590c;
  }
  .side-hint {
    margin: 0;
    padding: 8px 12px;
    border-top: 1px solid var(--line-soft, #eef1f4);
    font-size: 0.72rem;
    color: var(--muted, #64748b);
  }
  .hint {
    color: var(--muted, #64748b);
  }
  .hint.error {
    color: var(--danger-500, #c0392b);
  }
  @media (max-width: 900px) {
    .explorer {
      grid-template-columns: 1fr;
      grid-template-rows: 220px 1fr;
    }
  }
</style>
