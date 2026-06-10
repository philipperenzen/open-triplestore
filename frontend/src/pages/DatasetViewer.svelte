<script>
  // Dataset 3D & map viewer: map (Leaflet) + 3D (three.js) views over the
  // dataset's viewer feed, with a shared selection driving a linked-data panel.
  // Selecting a part on the map, in the 3D scene, or in the element list keeps
  // all three in sync; the panel shows that element's RDF.
  import { t as i18nT } from 'svelte-i18n';
  import { Link } from '../lib/router/index.js';
  import { getViewerFeed } from '../lib/api.js';
  import { shortenIRI } from '../lib/rdf-utils.js';
  import { ChevronLeft, Map as MapIcon, Boxes } from 'lucide-svelte';
  import ViewerMap from '../components/viewer/ViewerMap.svelte';
  import Viewer3D from '../components/viewer/Viewer3D.svelte';
  import ElementPanel from '../components/viewer/ElementPanel.svelte';

  export let id = '';

  let elements = [];
  let loading = true;
  let error = '';
  let selected = '';

  $: selectedElement = elements.find((e) => e.id === selected) || null;

  async function load() {
    loading = true;
    error = '';
    try {
      const data = await getViewerFeed(id);
      elements = data?.elements || [];
      // Pre-select the first element that has a 3D model or geometry.
      const first =
        elements.find((e) => e.gltf_url || (e.files || []).length) ||
        elements.find((e) => e.wkt4326);
      if (first) selected = first.id;
    } catch (e) {
      error = e?.message || 'failed';
    } finally {
      loading = false;
    }
  }

  function onSelect(event) {
    selected = event.detail.id;
  }

  load();
</script>

<div class="page viewer-page">
  <div class="page-head">
    <Link to={`/datasets/${id}`} class="btn btn-sm">
      <ChevronLeft size={16} />
      {$i18nT('pages.datasetViewer.back')}
    </Link>
    <h1>{$i18nT('pages.datasetViewer.title')}</h1>
  </div>

  {#if loading}
    <p class="hint">…</p>
  {:else if error}
    <p class="hint error">{error}</p>
  {:else if elements.length === 0}
    <p class="hint">{$i18nT('pages.datasetViewer.empty')}</p>
  {:else}
    <div class="viewer-grid">
      <section class="pane map-pane">
        <header><MapIcon size={15} /> {$i18nT('pages.datasetViewer.map')}</header>
        <ViewerMap {elements} {selected} on:select={onSelect} height="calc(100% - 30px)" />
      </section>
      <section class="pane three-pane">
        <header><Boxes size={15} /> {$i18nT('pages.datasetViewer.threeD')}</header>
        <Viewer3D {elements} {selected} on:select={onSelect} height="calc(100% - 30px)" />
      </section>
      <aside class="pane side-pane">
        <header>{$i18nT('pages.datasetViewer.elements')} ({elements.length})</header>
        <ul class="element-list">
          {#each elements as el}
            <li>
              <button
                class:active={el.id === selected}
                on:click={() => (selected = el.id)}
                title={el.id}
              >
                {el.label || shortenIRI(el.id)}
                {#if el.gltf_url || (el.files || []).length}<span class="badge">3D</span>{/if}
                {#if el.wkt4326}<span class="badge geo">geo</span>{/if}
              </button>
            </li>
          {/each}
        </ul>
        <div class="panel-wrap">
          <ElementPanel iri={selected} datasetId={id} element={selectedElement} />
        </div>
      </aside>
    </div>
  {/if}
</div>

<style>
  .viewer-page {
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
  }
  .viewer-grid {
    flex: 1;
    min-height: 0;
    display: grid;
    grid-template-columns: 1fr 1fr 340px;
    grid-template-rows: 1fr;
    gap: 0.6rem;
  }
  .pane {
    display: flex;
    flex-direction: column;
    min-height: 0;
    border: 1px solid var(--border, #e3e7ea);
    border-radius: 10px;
    background: var(--surface-1, #fff);
    overflow: hidden;
  }
  .pane > header {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    padding: 5px 10px;
    font-size: 0.8rem;
    font-weight: 600;
    color: var(--text-2, #555);
    border-bottom: 1px solid var(--border, #e3e7ea);
  }
  .map-pane,
  .three-pane {
    min-height: 0;
  }
  .side-pane {
    display: grid;
    grid-template-rows: auto minmax(80px, 32%) 1fr;
  }
  .element-list {
    list-style: none;
    margin: 0;
    padding: 0.3rem;
    overflow: auto;
    border-bottom: 1px solid var(--border, #e3e7ea);
  }
  .element-list button {
    width: 100%;
    text-align: left;
    border: 0;
    background: transparent;
    padding: 4px 8px;
    border-radius: 6px;
    cursor: pointer;
    font-size: 0.85rem;
    display: flex;
    align-items: center;
    gap: 0.4rem;
  }
  .element-list button:hover {
    background: var(--surface-2, #f1f3f5);
  }
  .element-list button.active {
    background: var(--accent-soft, #e7f0fb);
    font-weight: 600;
  }
  .badge {
    font-size: 0.62rem;
    padding: 0 6px;
    border-radius: 99px;
    background: #e8590c22;
    color: #e8590c;
  }
  .badge.geo {
    background: #4a90d922;
    color: #2f6fb3;
  }
  .panel-wrap {
    min-height: 0;
    overflow: auto;
  }
  .hint {
    color: var(--text-2, #777);
  }
  .hint.error {
    color: #c0392b;
  }
  @media (max-width: 1100px) {
    .viewer-grid {
      grid-template-columns: 1fr;
      grid-template-rows: 320px 320px auto;
    }
  }
</style>
