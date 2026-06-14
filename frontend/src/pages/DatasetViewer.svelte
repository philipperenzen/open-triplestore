<script>
  // Dataset geo data explorer. The map is the canvas: zoomed out every located
  // element is a dot; zooming in, elements with a 3D model become to-scale
  // model markers. Clicking a feature (or a list row) opens the element modal —
  // properties, BOT/IFC substructure (all navigable) and an interactive 3D
  // viewer. Datasets without any located element fall back to a pure 3D
  // explorer over their models. Light/dark follows the app theme.
  import { t as i18nT } from 'svelte-i18n';
  import { Link } from '../lib/router/index.js';
  import { getViewerFeed, listDatasetGraphs } from '../lib/api.js';
  import { shortenIRI } from '../lib/rdf-utils.js';
  import { ChevronLeft, Search, Boxes, MapPin, X, Download, FileDown } from 'lucide-svelte';
  import { modelRefOf } from '../lib/viewer/detect';
  import { modelRefs } from '../lib/viewer/geometry';
  import ViewerMap from '../components/viewer/ViewerMap.svelte';
  import Model3D from '../components/viewer/Model3D.svelte';
  import CesiumViewer from '../components/viewer/CesiumViewer.svelte';
  import ElementModal from '../components/viewer/ElementModal.svelte';

  export let id = '';

  let elements = [];
  let graphs = [];
  let loading = true;
  let error = '';
  let selected = '';
  let query = '';
  let mapComponent;
  let dlOpen = false;
  // Central canvas render mode. 'map' = MapLibre + three.js (the full-feature
  // explorer: located dots/lines/areas, to-scale glTF/IFC/CityJSON/STL models,
  // and click → the ElementModal inspector with the BOT/IFC decomposition tree).
  // 'cesium' = the CesiumJS 3D-Tiles globe view (a toggle, not a separate page,
  // so it sacrifices none of those features — a pick still opens the inspector).
  let canvasMode = 'map';

  // Download sources: the original IFC file (fragment-less) and every dataset
  // graph in the user's preferred RDF serialization.
  const LD_FORMATS = [
    { key: 'turtle', label: 'Turtle' },
    { key: 'jsonld', label: 'JSON-LD' },
    { key: 'rdfxml', label: 'RDF/XML' },
    { key: 'ntriples', label: 'N-Triples' },
  ];
  $: ifcUrl = (elements.find((e) => e.ifc_url)?.ifc_url || '').split('#')[0];
  $: mapAttribution = elements.some((e) =>
    (e.files || []).some(([, url]) => /3dbag/i.test(url || ''))
  )
    ? '© 3DBAG by tudelft3d and 3DGI (CC BY 4.0)'
    : '';

  // Several movable inspector panels can be open at once (one per element).
  // Only the first MODEL_CAP panels mount the heavy 3D viewer; the rest open
  // "info-only" with a button to load 3D on demand (which evicts the oldest 3D
  // viewer). A small dock lists the open panels and brings them to front.
  const MAX_PANELS = 5;
  const MODEL_CAP = 2;
  let panels = []; // { id, z, loadModel, seq }
  let zTop = 1100;
  let seqCounter = 0;

  const panelLabel = (pid) => {
    const el = elements.find((e) => e.id === pid);
    return el?.label || shortenIRI(pid);
  };

  $: filtered = query
    ? elements.filter((e) =>
        (e.label || e.id).toLowerCase().includes(query.toLowerCase())
      )
    : elements;
  $: located = filtered.filter((e) => e.wkt4326);
  $: unlocated = filtered.filter((e) => !e.wkt4326);
  $: hasGeo = elements.some((e) => e.wkt4326);
  $: fallbackRefs = modelRefs(elements);
  // 3D-Tiles-able content: any loadable 3D model, or volumetric WKT geometry
  // (POLYHEDRALSURFACE / TIN / SOLID / Z) — gates the Cesium viewer entry point.
  $: has3dContent =
    elements.some((e) => modelRefOf(e)) ||
    elements.some((e) => /POLYHEDRALSURFACE|\bTIN\b|\bSOLID\b| Z[ (]/i.test(e.wkt4326 || ''));

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
    try {
      graphs = (await listDatasetGraphs(id)) || [];
    } catch {
      graphs = []; // downloads simply hide when the graph list is unavailable
    }
  }

  function open(elId, { fly = true } = {}) {
    selected = elId;
    const el = elements.find((e) => e.id === elId);
    if (el && fly && el.wkt4326) mapComponent?.focusElement(elId);
    if (!el) return;
    if (panels.some((p) => p.id === elId)) {
      focusPanel(elId);
      return;
    }
    const wantsModel = hasModel(el);
    const modelCount = panels.filter((p) => p.loadModel).length;
    const panel = { id: elId, z: ++zTop, seq: seqCounter++, loadModel: wantsModel && modelCount < MODEL_CAP };
    panels = [...panels, panel];
    if (panels.length > MAX_PANELS) panels = panels.slice(panels.length - MAX_PANELS);
  }

  function focusPanel(id) {
    selected = id;
    panels = panels.map((p) => (p.id === id ? { ...p, z: ++zTop } : p));
  }
  function closePanel(id) {
    panels = panels.filter((p) => p.id !== id);
  }
  function closeAll() {
    panels = [];
  }
  // Promote an info-only panel to a live 3D viewer, evicting the oldest one when
  // the model cap is reached — so the user can always inspect the 3D they want.
  function loadModelFor(id) {
    let next = panels;
    if (panels.filter((p) => p.loadModel).length >= MODEL_CAP) {
      const oldest = panels.filter((p) => p.loadModel).sort((a, b) => a.z - b.z)[0];
      if (oldest) next = next.map((p) => (p.id === oldest.id ? { ...p, loadModel: false } : p));
    }
    panels = next.map((p) => (p.id === id ? { ...p, loadModel: true, z: ++zTop } : p));
  }

  // Fly the map to a located element (panels stay open; the flown-to element
  // gets the selection highlight so it's findable among the rooftops). When the
  // canvas is in 3D-Tiles (Cesium) mode the MapLibre component isn't mounted, so
  // switch back to the map first and focus once it has initialised.
  function showOnMap(elId) {
    selected = elId;
    if (canvasMode === 'cesium') {
      canvasMode = 'map';
      setTimeout(() => mapComponent?.focusElement(elId), 650);
    } else {
      mapComponent?.focusElement(elId);
    }
  }

  function onMapSelect(event) {
    const { id: elId, guid } = event.detail || {};
    // An IFC mesh pick carries the element's GlobalId — resolve it to the feed
    // element so clicking a beam opens *that beam's* panel.
    if (guid) {
      const byGuid = elements.find((e) => e.ifc_guid === guid);
      if (byGuid) {
        open(byGuid.id, { fly: false });
        return;
      }
    }
    if (elId) open(elId, { fly: false });
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
    {#if !loading && has3dContent}
      <!-- Render-mode toggle: the full MapLibre+three.js explorer (default) vs.
           the CesiumJS 3D-Tiles globe. A toggle, not a separate page — the side
           list + ElementModal inspector stay in both. -->
      <div class="canvas-mode" role="group" aria-label={$i18nT('pages.datasetViewer.cesium3dDesc')}>
        <button class:active={canvasMode === 'map'} on:click={() => (canvasMode = 'map')} title={$i18nT('pages.datasetViewer.mapMode')}>
          <MapPin size={13} /> {$i18nT('pages.datasetViewer.mapMode')}
        </button>
        <button class:active={canvasMode === 'cesium'} on:click={() => (canvasMode = 'cesium')} title={$i18nT('pages.datasetViewer.cesium3dDesc')}>
          <Boxes size={13} /> {$i18nT('pages.datasetViewer.cesium3d')}
        </button>
      </div>
    {/if}
    {#if !loading && (ifcUrl || graphs.length)}
      <div class="dl-wrap">
        <button class="btn btn-sm" on:click={() => (dlOpen = !dlOpen)} aria-expanded={dlOpen}>
          <Download size={14} /> {$i18nT('viewer.download')}
        </button>
        {#if dlOpen}
          <div class="dl-menu">
            {#if ifcUrl}
              <a class="dl-item dl-ifc" href={ifcUrl} download>
                <FileDown size={13} /> {$i18nT('viewer.downloadIfc')}
              </a>
            {/if}
            {#if graphs.length}
              <div class="dl-head">{$i18nT('viewer.downloadLd')}</div>
              {#each graphs as g (g.graph_iri)}
                <div class="dl-graph">
                  <span class="dl-gname" title={g.graph_iri}>{shortenIRI(g.graph_iri)}</span>
                  <span class="dl-fmts">
                    {#each LD_FORMATS as f}
                      <a href={`/store?graph=${encodeURIComponent(g.graph_iri)}&format=${f.key}`}>{f.label}</a>
                    {/each}
                  </span>
                </div>
              {/each}
            {/if}
          </div>
        {/if}
      </div>
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
        {#if canvasMode === 'cesium'}
          <!-- 3D Tiles globe. Embedded: a pick dispatches `select`, opening the
               same ElementModal inspector the map view uses (no lost features). -->
          <CesiumViewer datasetId={id} {selected} embedded on:select={onMapSelect} height="100%" />
        {:else if hasGeo}
          <ViewerMap
            bind:this={mapComponent}
            {elements}
            {selected}
            extraAttribution={mapAttribution}
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

{#each panels as p (p.id)}
  <ElementModal
    element={elements.find((e) => e.id === p.id) || null}
    {elements}
    datasetId={id}
    offset={p.seq % 6}
    z={p.z}
    lite={!p.loadModel}
    hasMap={hasGeo}
    on:focus={() => focusPanel(p.id)}
    on:close={() => closePanel(p.id)}
    on:navigate={(e) => open(e.detail.id)}
    on:loadmodel={() => loadModelFor(p.id)}
    on:showonmap={(e) => showOnMap(e.detail.id)}
  />
{/each}

{#if panels.length}
  <div class="panel-dock">
    <span class="dock-label">{panels.length} {$i18nT('viewer.openPanels')}</span>
    {#each panels as p (p.id)}
      <span class="dock-chip" class:has3d={p.loadModel}>
        <button class="dock-focus" on:click={() => focusPanel(p.id)} title={panelLabel(p.id)}>
          {#if p.loadModel}<Boxes size={11} />{/if}
          <span class="dock-name">{panelLabel(p.id)}</span>
        </button>
        <button class="dock-close" on:click={() => closePanel(p.id)} aria-label={$i18nT('viewer.close')}>
          <X size={12} />
        </button>
      </span>
    {/each}
    {#if panels.length > 1}
      <button class="dock-closeall" on:click={closeAll}>{$i18nT('viewer.closeAll')}</button>
    {/if}
  </div>
{/if}

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

  /* Canvas render-mode toggle (Map ↔ 3D Tiles) */
  .canvas-mode {
    display: inline-flex;
    border: 1px solid var(--line-soft, #e2e8f0);
    border-radius: 999px;
    overflow: hidden;
    background: var(--bg-soft, #f1f5f9);
  }
  .canvas-mode button {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    border: 0;
    background: transparent;
    padding: 4px 11px;
    font-size: 0.76rem;
    color: var(--muted, #64748b);
    cursor: pointer;
  }
  .canvas-mode button + button {
    border-left: 1px solid var(--line-soft, #e2e8f0);
  }
  .canvas-mode button.active {
    background: var(--bg-accent-soft, #e7f0fb);
    color: var(--brand-600, #2563a8);
    font-weight: 600;
  }

  /* Download menu (IFC + linked-data formats) */
  .dl-wrap { position: relative; margin-left: auto; }
  .dl-menu {
    position: absolute;
    right: 0;
    top: calc(100% + 6px);
    z-index: 50;
    min-width: 300px;
    padding: 8px;
    border-radius: 12px;
    background: var(--bg-elevated, #fff);
    border: 1px solid var(--line-soft, #e2e8f0);
    box-shadow: var(--shadow-md, 0 12px 30px rgba(0, 0, 0, 0.16));
    animation: rowIn var(--dur-base, 220ms) var(--ease-out, ease) both;
  }
  .dl-item {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 7px 8px;
    border-radius: 8px;
    font-size: 0.84rem;
    color: var(--ink-900, #0f172a);
    text-decoration: none;
  }
  .dl-item:hover { background: var(--bg-hover, rgba(0, 0, 0, 0.05)); }
  .dl-head {
    font-size: 0.66rem;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--muted, #64748b);
    font-weight: 700;
    padding: 8px 8px 2px;
  }
  .dl-graph {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 10px;
    padding: 5px 8px;
    font-size: 0.8rem;
  }
  .dl-gname {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--ink-700, #334155);
    min-width: 0;
  }
  .dl-fmts { display: inline-flex; gap: 8px; flex: none; }
  .dl-fmts a { font-size: 0.74rem; color: var(--brand-600, #1d6fb8); text-decoration: none; }
  .dl-fmts a:hover { text-decoration: underline; }

  /* Dock — manages the open inspector panels (focus / close). */
  .panel-dock {
    position: fixed;
    left: 50%;
    bottom: 14px;
    transform: translateX(-50%);
    z-index: 1300;
    display: flex;
    align-items: center;
    gap: 6px;
    max-width: min(92vw, 900px);
    flex-wrap: wrap;
    padding: 6px 8px;
    border-radius: 999px;
    background: var(--bg-elevated, rgba(255, 255, 255, 0.92));
    border: 1px solid var(--line-soft, #e2e8f0);
    box-shadow: var(--shadow-md, 0 12px 30px rgba(0, 0, 0, 0.16));
    backdrop-filter: blur(10px);
    animation: rowIn var(--dur-base, 220ms) var(--ease-out, ease) both;
  }
  .dock-label {
    font-size: 0.7rem;
    color: var(--muted, #64748b);
    padding: 0 4px;
    white-space: nowrap;
  }
  .dock-chip {
    display: inline-flex;
    align-items: center;
    gap: 2px;
    border-radius: 999px;
    background: var(--bg-soft, #f1f5f9);
    border: 1px solid transparent;
    max-width: 180px;
  }
  .dock-chip.has3d {
    border-color: rgba(232, 89, 12, 0.4);
  }
  .dock-focus {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    border: 0;
    background: transparent;
    padding: 4px 4px 4px 10px;
    cursor: pointer;
    font-size: 0.76rem;
    color: var(--ink-900, #0f172a);
    min-width: 0;
  }
  .dock-name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .dock-close {
    border: 0;
    background: transparent;
    padding: 4px 8px 4px 2px;
    cursor: pointer;
    color: var(--muted, #64748b);
    display: grid;
    place-items: center;
  }
  .dock-close:hover {
    color: var(--danger-500, #c0392b);
  }
  .dock-closeall {
    border: 0;
    background: transparent;
    cursor: pointer;
    font-size: 0.72rem;
    color: var(--muted, #64748b);
    padding: 0 8px;
    white-space: nowrap;
  }
  .dock-closeall:hover {
    color: var(--ink-900, #0f172a);
  }
  @media (max-width: 900px) {
    .explorer {
      grid-template-columns: 1fr;
      grid-template-rows: 220px 1fr;
    }
  }
</style>
