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
  import { copyToClipboard } from '../lib/clipboard.ts';
  import { ChevronLeft, ChevronRight, Search, Boxes, MapPin, X, Download, FileDown, Footprints, Code2, Check } from 'lucide-svelte';
  import { modelRefOf, modelRefsOf } from '../lib/viewer/detect';
  import { modelRefs } from '../lib/viewer/geometry';
  import ViewerMap from '../components/viewer/ViewerMap.svelte';
  import Model3D from '../components/viewer/Model3D.svelte';
  import CesiumViewer from '../components/viewer/CesiumViewer.svelte';
  import ElementModal from '../components/viewer/ElementModal.svelte';
  import Walkthrough from '../components/viewer/Walkthrough.svelte';

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
  const nodeLabel = (el) => el?.label || shortenIRI(el?.id || '');

  // ── Structural tree ─────────────────────────────────────────────────────────
  // Arrange the side list by the BOT/IFC spatial structure — Site → Building →
  // Storey → Space → Element — so the parts of one building stay together and a
  // 3000-element model is navigable by drilling in. Datasets with no parent
  // links (a flat set of geo features) keep the located / unlocated split, and
  // an active search always shows a flat result list.
  $: byId = new Map(elements.map((e) => [e.id, e]));
  $: childrenOf = (() => {
    const m = new Map();
    for (const e of elements) {
      if (e.parent && byId.has(e.parent)) {
        if (!m.has(e.parent)) m.set(e.parent, []);
        m.get(e.parent).push(e.id);
      }
    }
    const lbl = (cid) => byId.get(cid)?.label || cid;
    for (const kids of m.values()) kids.sort((a, b) => lbl(a).localeCompare(lbl(b)));
    return m;
  })();
  $: roots = elements
    .filter((e) => !e.parent || !byId.has(e.parent))
    .sort((a, b) => nodeLabel(a).localeCompare(nodeLabel(b)));
  $: hasStructure = childrenOf.size > 0;

  let expanded = new Set();
  let treeInit = false;
  // Open the top two structural levels on first load (Site → Building →
  // Storeys), leaving deeper containers collapsed.
  $: if (elements.length && !treeInit && childrenOf) {
    treeInit = true;
    const next = new Set();
    const seed = (eid, depth) => {
      const kids = childrenOf.get(eid) || [];
      if (kids.length && depth < 2) {
        next.add(eid);
        kids.forEach((k) => seed(k, depth + 1));
      }
    };
    roots.forEach((r) => seed(r.id, 0));
    expanded = next;
  }
  function toggle(eid) {
    const next = new Set(expanded);
    next.has(eid) ? next.delete(eid) : next.add(eid);
    expanded = next;
  }
  function expandAll() {
    expanded = new Set(childrenOf.keys());
  }
  function collapseAll() {
    expanded = new Set();
  }
  // Flatten the tree to the rows currently visible (depth for indentation,
  // child count for the disclosure caret).
  $: treeRows = (() => {
    const rows = [];
    const walk = (eid, depth) => {
      const el = byId.get(eid);
      if (!el) return;
      const kids = childrenOf.get(eid) || [];
      rows.push({ el, depth, count: kids.length, open: expanded.has(eid) });
      if (kids.length && expanded.has(eid)) kids.forEach((k) => walk(k, depth + 1));
    };
    roots.forEach((r) => walk(r.id, 0));
    return rows;
  })();

  // Two-phase load so the map paints fast on big BIM datasets: phase 1 fetches
  // only the located elements (the map's render set — seconds, not the whole
  // multi-thousand-element building); phase 2 fetches the full feed (the
  // structure tree + every sub-element) in the background and swaps it in.
  let fullLoaded = false;
  async function load() {
    loading = true;
    error = '';
    // Phase 1 — fast located subset → the map renders almost immediately.
    try {
      const fast = await getViewerFeed(id, null, { located: true });
      elements = fast?.elements || [];
    } catch {
      /* fall through — the full feed below is the source of truth */
    }
    loading = false; // shell + map are interactive now

    // Graph list (download menu) — cheap, fetch alongside without blocking.
    listDatasetGraphs(id)
      .then((g) => (graphs = g || []))
      .catch(() => (graphs = [])); // downloads simply hide when unavailable

    // Phase 2 — the full feed (tree + sub-elements) in the background.
    try {
      const full = await getViewerFeed(id);
      if (full?.elements) {
        treeInit = false; // re-seed the auto-expansion over the full hierarchy
        elements = full.elements;
      }
    } catch (e) {
      if (!elements.length) error = e?.message || 'failed';
    } finally {
      fullLoaded = true;
    }
  }

  function open(elId, { fly = true } = {}) {
    selected = elId;
    const el = elements.find((e) => e.id === elId);
    // Fly even for an element with no geometry of its own — the map walks up to
    // its located ancestor (the building) and lights this element's mesh.
    if (el && fly && hasGeo) mapComponent?.focusElement(elId);
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
    if (!panels.length) selected = ''; // clear the highlight + x-ray when nothing is open
  }
  function closeAll() {
    panels = [];
    selected = '';
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

  // ── First-person walkthrough ────────────────────────────────────────────────
  // ViewerMap suggests an IFC building to walk through once you're zoomed in on
  // it; clicking the prompt opens the whole-building model in a first-person view.
  let walkSuggest = null; // { id, label } | null
  let walkthrough = null; // { url, format, upAxis, label } | null
  function onWalkSuggest(e) {
    walkSuggest = e.detail || null;
  }
  function openWalkthrough() {
    const el = walkSuggest && elements.find((x) => x.id === walkSuggest.id);
    const ref = el && modelRefOf(el);
    if (!ref) return;
    walkthrough = {
      url: ref.url.split('#')[0], // the whole building, not one element fragment
      format: ref.format,
      upAxis: ref.upAxis,
      label: walkSuggest.label,
    };
  }
  // "Explore inside" from an element inspector — open the walkthrough for the
  // element's whole IFC building (a container: Site / Building / Storey).
  function walkthroughFor(elId) {
    const el = elements.find((x) => x.id === elId);
    const ref = el && modelRefsOf(el).find((r) => r.format === 'ifc');
    if (!ref) return;
    walkthrough = {
      url: ref.url.split('#')[0],
      format: 'ifc',
      upAxis: ref.upAxis,
      label: el.label || shortenIRI(el.id),
    };
  }
  function walkInspect(e) {
    walkthrough = null; // leave the immersive view to show the full RDF panel
    onMapSelect(e);
  }

  // ── Embed snippet ───────────────────────────────────────────────────────────
  // Copyable <iframe> code for the current canvas mode; the /embed/* pages are
  // the chrome-less variants of this explorer (see docs/embedding.md).
  let embedOpen = false;
  let embedCopied = false;
  $: embedUrl = `${window.location.origin}/embed/${canvasMode === 'cesium' ? 'cesium' : 'map'}/${encodeURIComponent(id)}`;
  $: embedSnippet = `<iframe src="${embedUrl}" width="100%" height="480" style="border:0;border-radius:12px" loading="lazy" allowfullscreen></iframe>`;
  async function copyEmbed() {
    embedCopied = await copyToClipboard(embedSnippet);
    if (embedCopied) setTimeout(() => (embedCopied = false), 1600);
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
    {#if !loading && elements.length}
      <div class="dl-wrap embed-wrap">
        <button class="btn btn-sm" on:click={() => (embedOpen = !embedOpen)} aria-expanded={embedOpen}>
          <Code2 size={14} /> {$i18nT('embed.embedTitle')}
        </button>
        {#if embedOpen}
          <div class="dl-menu embed-menu">
            <div class="dl-head">{$i18nT('embed.embedDesc')}</div>
            <code class="embed-code">{embedSnippet}</code>
            <button class="btn btn-sm embed-copy" on:click={copyEmbed}>
              {#if embedCopied}<Check size={13} /> {$i18nT('embed.copied')}{:else}{$i18nT('embed.copy')}{/if}
            </button>
          </div>
        {/if}
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

  {#if error}
    <p class="hint error">{error}</p>
  {:else}
    <!-- The shell + map render immediately; the side list shows a loading state
         while the feed streams in, so the map is interactive in ~1s instead of
         blocking on the whole-building feed. -->
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
        {#if hasStructure && !query}
          <div class="tree-tools">
            <button class="link-btn" on:click={expandAll}>{$i18nT('viewer.expandAll')}</button>
            <span class="dot-sep">·</span>
            <button class="link-btn" on:click={collapseAll}>{$i18nT('viewer.collapseAll')}</button>
          </div>
        {/if}
        <div class="list-scroll">
          {#if loading && elements.length === 0}
            <div class="side-loading" role="status">
              <span class="ls-spin"></span>{$i18nT('viewer.loadingElements')}
            </div>
          {:else if !loading && elements.length === 0}
            <p class="empty-row">{$i18nT('pages.datasetViewer.empty')}</p>
          {:else if query}
            <!-- Search: flat result list across the whole dataset. -->
            <ul>
              {#each filtered as el (el.id)}
                <li>
                  <button class="row" class:active={el.id === selected} on:click={() => open(el.id)} title={el.id}>
                    <span class="label">{nodeLabel(el)}</span>
                    {#if el.wkt4326}<span class="loc-i"><MapPin size={11} /></span>{/if}
                    {#if hasModel(el)}<span class="badge">3D</span>{/if}
                  </button>
                </li>
              {/each}
              {#if !filtered.length}<li class="empty-row">{$i18nT('viewer.noMatches')}</li>{/if}
            </ul>
          {:else if hasStructure}
            <!-- Structural tree: Site → Building → Storey → Space → Element. -->
            <ul class="tree">
              {#each treeRows as r (r.el.id)}
                <li
                  class="tree-row"
                  class:active={r.el.id === selected}
                  style:--depth={r.depth}
                >
                  {#if r.count}
                    <button
                      class="twist"
                      class:open={r.open}
                      on:click={() => toggle(r.el.id)}
                      aria-label={r.open ? $i18nT('viewer.collapse') : $i18nT('viewer.expand')}
                    ><ChevronRight size={13} /></button>
                  {:else}
                    <span class="twist-spacer"></span>
                  {/if}
                  <button class="row row-main" on:click={() => open(r.el.id)} title={r.el.id}>
                    <span class="label">{nodeLabel(r.el)}</span>
                    {#if r.count}<span class="cnt">{r.count}</span>{/if}
                    {#if r.el.wkt4326}<span class="loc-i"><MapPin size={11} /></span>{/if}
                    {#if hasModel(r.el)}<span class="badge">3D</span>{/if}
                  </button>
                </li>
              {/each}
            </ul>
          {:else}
            <!-- Flat dataset (no spatial structure): located / unlocated split. -->
            {#if located.length}
              <div class="group-label"><MapPin size={12} /> {$i18nT('viewer.located')}</div>
              <ul>
                {#each located as el (el.id)}
                  <li>
                    <button class="row" class:active={el.id === selected} on:click={() => open(el.id)} title={el.id}>
                      <span class="label">{nodeLabel(el)}</span>
                      {#if hasModel(el)}<span class="badge">3D</span>{/if}
                    </button>
                  </li>
                {/each}
              </ul>
            {/if}
            {#if unlocated.length}
              <div class="group-label"><Boxes size={12} /> {$i18nT('viewer.noLocation')}</div>
              <ul>
                {#each unlocated as el (el.id)}
                  <li>
                    <button class="row" class:active={el.id === selected} on:click={() => open(el.id, { fly: false })} title={el.id}>
                      <span class="label">{nodeLabel(el)}</span>
                      {#if hasModel(el)}<span class="badge">3D</span>{/if}
                    </button>
                  </li>
                {/each}
              </ul>
            {/if}
          {/if}
        </div>
        {#if !fullLoaded}
          <div class="loading-structure" role="status">
            <span class="ls-spin"></span>{$i18nT('viewer.loadingStructure')}
          </div>
        {/if}
        <p class="side-hint">{$i18nT('viewer.zoomHint')}</p>
      </aside>

      <section class="canvas card-flat">
        {#if canvasMode === 'cesium'}
          <!-- 3D Tiles globe. Embedded: a pick dispatches `select`, opening the
               same ElementModal inspector the map view uses (no lost features). -->
          <CesiumViewer datasetId={id} {selected} embedded on:select={onMapSelect} height="100%" />
        {:else if hasGeo || loading}
          <!-- Mount the map immediately (even before the feed resolves) so the
               basemap is interactive in ~1s; features stream in when the feed lands. -->
          <ViewerMap
            bind:this={mapComponent}
            {elements}
            {selected}
            extraAttribution={mapAttribution}
            on:select={onMapSelect}
            on:walksuggest={onWalkSuggest}
            height="100%"
          />
          {#if walkSuggest && !walkthrough}
            <button class="walk-suggest" on:click={openWalkthrough}>
              <Footprints size={16} /> {$i18nT('viewer.walkThroughBuilding')}
            </button>
          {/if}
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
    on:walkthrough={(e) => walkthroughFor(e.detail.id)}
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

{#if walkthrough}
  <Walkthrough
    url={walkthrough.url}
    format={walkthrough.format}
    upAxis={walkthrough.upAxis}
    label={walkthrough.label}
    {elements}
    on:close={() => (walkthrough = null)}
    on:inspect={walkInspect}
  />
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
  .canvas {
    position: relative; /* anchor the floating "walk through" prompt */
  }
  .walk-suggest {
    position: absolute;
    left: 50%;
    bottom: 18px;
    transform: translateX(-50%);
    z-index: 6;
    display: inline-flex;
    align-items: center;
    gap: 7px;
    padding: 9px 17px;
    border: 0;
    border-radius: 999px;
    background: var(--brand-600, #2563a8);
    color: #fff;
    font-size: 0.84rem;
    font-weight: 600;
    cursor: pointer;
    box-shadow: 0 6px 22px rgba(37, 99, 168, 0.5);
    /* One-shot entrance instead of an endless box-shadow pulse, which read as
       distracting and ignored motion preferences. */
    animation: rowIn var(--dur-base, 220ms) var(--ease-out, ease) both;
  }
  .walk-suggest:hover {
    background: #e8590c;
  }
  @media (prefers-reduced-motion: reduce) {
    .walk-suggest,
    .panel-dock,
    .dl-menu {
      animation: none;
    }
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

  /* Structural tree (Site → Building → Storey → Space → Element) */
  .tree-tools {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 2px 12px 0;
  }
  .link-btn {
    border: 0;
    background: transparent;
    color: var(--brand-600, #2563a8);
    cursor: pointer;
    padding: 2px 0;
    font-size: 0.7rem;
  }
  .link-btn:hover {
    text-decoration: underline;
  }
  .dot-sep {
    color: var(--muted, #94a3b8);
    font-size: 0.7rem;
  }
  .tree-row {
    display: flex;
    align-items: center;
    border-radius: var(--radius-sm, 7px);
    padding-left: calc(var(--depth, 0) * 13px);
  }
  .tree-row:hover {
    background: var(--bg-hover, rgba(0, 0, 0, 0.04));
  }
  .tree-row.active {
    background: var(--bg-accent-soft, #e7f0fb);
  }
  .tree-row.active .label {
    font-weight: 600;
  }
  .tree-row .twist {
    flex: none;
    width: 22px;
    align-self: stretch;
    display: flex;
    align-items: center;
    justify-content: center;
    border: 0;
    background: transparent;
    color: var(--muted, #64748b);
    cursor: pointer;
    padding: 0;
  }
  .tree-row .twist :global(svg) {
    transition: transform 0.12s ease;
  }
  .tree-row .twist.open :global(svg) {
    transform: rotate(90deg);
  }
  .twist-spacer {
    flex: none;
    width: 22px;
  }
  .tree-row .row-main {
    flex: 1;
    min-width: 0;
    width: auto;
    background: transparent;
  }
  .tree-row .row-main:hover {
    background: transparent;
  }
  .cnt {
    flex: none;
    font-size: 0.64rem;
    padding: 0 6px;
    border-radius: 99px;
    background: var(--bg-soft, #eef2f6);
    color: var(--muted, #64748b);
  }
  .loc-i {
    flex: none;
    display: inline-flex;
    color: var(--brand-500, #2f88d8);
  }
  .empty-row {
    padding: 10px 8px;
    color: var(--muted, #64748b);
    font-size: 0.8rem;
  }
  .side-hint {
    margin: 0;
    padding: 8px 12px;
    border-top: 1px solid var(--line-soft, #eef1f4);
    font-size: 0.72rem;
    color: var(--muted, #64748b);
  }
  /* Side-list loading state while the first (located) feed is in flight. */
  .side-loading {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 14px 12px;
    font-size: 0.8rem;
    color: var(--muted, #64748b);
  }
  /* Background full-feed (structure tree) loading cue — the map is already live. */
  .loading-structure {
    display: flex;
    align-items: center;
    gap: 7px;
    padding: 7px 12px;
    border-top: 1px solid var(--line-soft, #eef1f4);
    font-size: 0.72rem;
    color: var(--muted, #64748b);
  }
  .ls-spin {
    width: 11px;
    height: 11px;
    flex: none;
    border: 2px solid color-mix(in srgb, var(--brand-500, #2f88d8) 35%, transparent);
    border-top-color: var(--brand-500, #2f88d8);
    border-radius: 50%;
    animation: ls-spin 0.8s linear infinite;
  }
  @keyframes ls-spin {
    to {
      transform: rotate(360deg);
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .ls-spin {
      animation: none;
    }
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
  /* Visible keyboard focus on the bespoke viewer controls. */
  .canvas-mode button:focus-visible,
  .dock-focus:focus-visible,
  .dock-close:focus-visible,
  .dock-closeall:focus-visible,
  .twist:focus-visible,
  .link-btn:focus-visible,
  .walk-suggest:focus-visible,
  .row:focus-visible {
    outline: none;
    box-shadow: 0 0 0 2px var(--brand-400, #5aa9e0);
    border-radius: var(--radius-sm, 7px);
  }

  /* Download menu (IFC + linked-data formats) */
  .dl-wrap { position: relative; margin-left: auto; }
  /* When the Embed button is present it owns the auto push; Download follows it. */
  .embed-wrap ~ .dl-wrap { margin-left: 0; }
  .embed-menu { min-width: 340px; }
  .embed-code {
    display: block;
    margin: 6px 8px;
    padding: 8px 10px;
    border-radius: 8px;
    background: var(--bg-soft, #f1f5f9);
    color: var(--ink-700, #334155);
    font-size: 0.68rem;
    line-height: 1.5;
    word-break: break-all;
    user-select: all;
  }
  .embed-copy { margin: 0 8px 6px; }
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
