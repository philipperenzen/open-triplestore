<script>
  // CesiumJS 3D-Tiles viewer for a dataset. Loads the dataset's tileset
  // (/api/datasets/:id/3dtiles/tileset.json), and on click resolves the picked
  // feature's stable per-feature IRI (the EXT_structural_metadata `iri`
  // property — the same key that is the RDF subject and the viewer lookup key)
  // and runs a SPARQL query for it, showing predicate/object pairs in a panel.
  //
  // Cesium owns depth, terrain and globe occlusion natively, so this path does
  // NOT suffer the MapLibre+three.js satellite/tilt/z-fight failure modes the
  // 2D viewer works around — switching imagery here is a plain layer swap.
  //
  // Robustness: imagery is token-free (OpenStreetMap streets + Esri World
  // Imagery satellite — the SAME source as the 2D map), so the base layer never
  // depends on a Cesium Ion token (the bundled demo token expires). The globe
  // gets a neutral base colour so even a failed imagery layer reads as a surface,
  // not black. The viewer renders on-demand (requestRenderMode) so it idles at
  // ~0 fps over a near-static scene; every mutation requests a frame.
  import { onMount, onDestroy, createEventDispatcher } from 'svelte';
  import {
    Boxes, X, MapPin, Satellite, ExternalLink, Sparkles, Loader2,
    Home, Plus, Minus, Maximize2, MousePointerClick,
  } from 'lucide-svelte';
  import { shortenIRI } from '../../lib/rdf-utils.ts';
  import { navigate } from '../../lib/router/index.js';
  import { openSparkExplain } from '../../lib/sparkHelp.js';
  import { isDark } from '../../lib/theme.js';

  // Open the resource page for a predicate/object IRI (in-app navigation so it
  // shares the SPA session) — bnodes (_:…) are not dereferenceable, so callers
  // must gate on isBnode before invoking this.
  function openResource(iri) {
    navigate(`/resource?iri=${encodeURIComponent(iri)}`);
  }

  /** @type {string} dataset id whose 3D Tiles tileset is loaded. */
  export let datasetId = '';
  export let height = '100%';
  // Embedded mode: hosted inside the dataset explorer's canvas next to the
  // element list + the rich ElementModal inspector. A pick then dispatches
  // `select` to the parent (which opens the inspector) instead of showing this
  // component's own lightweight SPARQL panel — so switching to 3D Tiles
  // sacrifices no features. A "full screen" button opens the standalone page.
  export let embedded = false;
  /** Show the "open full screen" in-app navigation button (embedded mode).
   *  Iframe embeds turn this off — there is no SPA router to navigate. */
  export let expand = true;
  /** Currently-selected element id (from the parent) to highlight in the scene. */
  export let selected = '';

  const dispatch = createEventDispatcher();

  // Cesium fetches its workers / glsl / Assets relative to CESIUM_BASE_URL; we
  // point it at the matching CDN build so static assets resolve with no bundler
  // configuration.
  const CESIUM_VERSION = '1.123.0';
  const CESIUM_BASE_URL = `https://cdn.jsdelivr.net/npm/cesium@${CESIUM_VERSION}/Build/Cesium/`;
  // Token-free Esri World Imagery — the same source the 2D map viewer uses — so
  // the satellite base never depends on a Cesium Ion token.
  const ESRI_IMAGERY =
    'https://server.arcgisonline.com/ArcGIS/rest/services/World_Imagery/MapServer/tile/{z}/{y}/{x}';
  const ESRI_CREDIT = 'Esri, Maxar, Earthstar Geographics, and the GIS User Community';

  let containerEl;
  let viewer = null;
  let tileset = null;
  let Cesium = null;
  let handler = null;

  let loading = true;
  let error = '';
  let empty = false; // tileset has no renderable geometry
  let baseLayer = 'streets'; // 'streets' | 'satellite' — token-free either way
  let showHint = true; // "click a building to inspect", cleared on first pick
  let homeView = null; // captured bounding sphere for the Home button

  // Side panel state for the picked feature (standalone mode only).
  let selectedIri = '';
  let selectedLabel = '';
  let rows = []; // { p, o, isIri }
  let queryLoading = false;
  let queryError = '';

  // Last IRI applied as the tileset selection style — so an identical selection
  // doesn't rebuild the style (which would force a render under requestRenderMode).
  let lastStyledIri = null;
  const HIGHLIGHT_COLOR_CSS = '#e8590c';

  let dark = false;
  const unsubTheme = isDark.subscribe((v) => {
    dark = v;
  });

  async function init() {
    try {
      window.CESIUM_BASE_URL = CESIUM_BASE_URL;
      Cesium = await import('cesium');
      await import('cesium/Build/Cesium/Widgets/widgets.css');

      viewer = new Cesium.Viewer(containerEl, {
        // We add our own token-free imagery (below), so disable the default Ion
        // world-imagery base layer entirely.
        baseLayer: false,
        baseLayerPicker: false,
        geocoder: false,
        homeButton: false,
        sceneModePicker: false,
        navigationHelpButton: false,
        timeline: false,
        animation: false,
        fullscreenButton: false,
        infoBox: false,
        selectionIndicator: false,
        // Render on demand: idle at ~0 fps over a near-static scene; every
        // mutation below calls scene.requestRender(). Cesium auto-requests during
        // camera moves and tile streaming, so interaction stays live.
        requestRenderMode: true,
        maximumRenderTimeChange: Infinity,
      });
      if (import.meta.env.DEV) window.__otsCesium = viewer; // dev console handle

      const scene = viewer.scene;
      scene.globe.depthTestAgainstTerrain = false;
      // Neutral globe so a fully-failed imagery layer still reads as a surface
      // rather than the black void the old Ion-dependent default fell back to.
      scene.globe.baseColor = Cesium.Color.fromCssColorString('#1a2330');
      // Soften the lone sun so flat, sun-facing roofs read as shaded grey instead
      // of blowing out to white near top-down.
      scene.light = new Cesium.SunLight({ intensity: 0.55 });

      applyBaseLayer();
      await loadTileset();
      attachPicking();
      scene.requestRender();
    } catch (e) {
      error = e?.message || 'Failed to initialise the 3D viewer.';
    } finally {
      // Clear the overlay once the tileset is in the scene — the camera flight
      // must NOT gate the UI.
      loading = false;
    }
  }

  /** (Re)apply the current base imagery. Token-free in both modes. */
  function applyBaseLayer() {
    if (!viewer || !Cesium) return;
    const layers = viewer.imageryLayers;
    layers.removeAll();
    try {
      const provider =
        baseLayer === 'satellite'
          ? new Cesium.UrlTemplateImageryProvider({
              url: ESRI_IMAGERY,
              maximumLevel: 19,
              credit: ESRI_CREDIT,
            })
          : new Cesium.OpenStreetMapImageryProvider({ url: 'https://tile.openstreetmap.org/' });
      layers.addImageryProvider(provider);
      // If tiles fail to load (network/provider outage), drop back to OSM streets
      // once so the demo never silently shows a blank base.
      if (provider.errorEvent) {
        provider.errorEvent.addEventListener(() => {
          if (baseLayer !== 'streets') {
            baseLayer = 'streets';
            applyBaseLayer();
          }
        });
      }
    } catch {
      /* leave the neutral globe surface */
    }
    viewer.scene.requestRender();
  }

  async function loadTileset() {
    if (tileset) {
      viewer.scene.primitives.remove(tileset);
      tileset = null;
    }
    tileset = await Cesium.Cesium3DTileset.fromUrl(
      `/api/datasets/${encodeURIComponent(datasetId)}/3dtiles/tileset.json`,
    );
    // Load the (single) tile a touch more eagerly so the block fills in quickly.
    tileset.maximumScreenSpaceError = 16;
    // Keep the default HIGHLIGHT (multiply) blend mode: a non-selected
    // color('#ffffff') is the multiply identity, so every building KEEPS its
    // per-feature COLOR_0 tone while only the matched feature tints orange. This
    // matters in embedded mode, where the parent's `selected` is often an IFC
    // element that ISN'T a tileset feature — under REPLACE that would dim the
    // whole block to grey on every selection, defeating the COLOR_0 polish.
    viewer.scene.primitives.add(tileset);
    // A fresh tileset has no style; drop the idempotency cache so the current
    // selection re-applies onto it.
    lastStyledIri = null;

    const bs = tileset.boundingSphere;
    const hasContent = bs && Number.isFinite(bs.radius) && bs.radius > 0;
    empty = !hasContent;
    if (hasContent) {
      homeView = bs.clone ? bs.clone() : bs;
      // flyToBoundingSphere with duration 0 resolves immediately (zoomTo can hang
      // on a degenerate volume).
      viewer.camera.flyToBoundingSphere(bs, { duration: 0 });
    } else {
      flyHomeFallback();
    }
    applyHighlight(embedded ? selected : selectedIri);
    viewer.scene.requestRender();
  }

  /** Fixed Nijmegen pose so the camera always frames *something*, even when the
   *  tileset is empty or its bounding volume is degenerate. */
  function flyHomeFallback() {
    if (!viewer || !Cesium) return;
    viewer.camera.flyTo({
      destination: Cesium.Cartesian3.fromDegrees(5.8337, 51.8408, 1400),
      duration: 0,
    });
  }

  function flyHome() {
    if (!viewer) return;
    if (homeView) viewer.camera.flyToBoundingSphere(homeView, { duration: 0.8 });
    else flyHomeFallback();
    viewer.scene.requestRender();
  }

  function zoomBy(dir) {
    if (!viewer) return;
    const h = viewer.camera.positionCartographic.height || 1000;
    if (dir < 0) viewer.camera.zoomIn(h * 0.35);
    else viewer.camera.zoomOut(h * 0.35);
    viewer.scene.requestRender();
  }

  function openFullScreen() {
    navigate(`/datasets/${encodeURIComponent(datasetId)}/cesium`);
  }

  function attachPicking() {
    handler = new Cesium.ScreenSpaceEventHandler(viewer.scene.canvas);
    handler.setInputAction((movement) => {
      const picked = viewer.scene.pick(movement.position);
      // A 3D-Tiles feature exposes per-feature metadata via getProperty(); the
      // binding key is the `iri` property written by the tiling pipeline.
      if (picked instanceof Cesium.Cesium3DTileFeature) {
        const iri = picked.getProperty('iri');
        const label =
          picked.getProperty('label') ||
          picked.getProperty('name') ||
          (iri ? shortenIRI(iri) : '');
        showHint = false;
        if (iri) {
          // Embedded: hand the pick to the parent's rich inspector (it sets
          // `selected`, which restyles the scene reactively). Standalone: show
          // this component's own SPARQL panel; selectedIri drives the highlight.
          if (embedded) dispatch('select', { id: iri });
          else selectFeature(iri, label);
        }
        return;
      }
      // Clicking empty space clears the selection + highlight.
      if (!embedded) closePanel();
    }, Cesium.ScreenSpaceEventType.LEFT_CLICK);

    // Double-click empty space re-frames the block — the only recovery once you
    // orbit away from a small tileset.
    handler.setInputAction((m) => {
      const picked = viewer.scene.pick(m.position);
      if (!(picked instanceof Cesium.Cesium3DTileFeature)) flyHome();
    }, Cesium.ScreenSpaceEventType.LEFT_DOUBLE_CLICK);
  }

  /**
   * Highlight the feature whose `iri` matches by colour-styling the tileset —
   * the SINGLE source of truth for selection (no per-feature `feature.color`,
   * which a later tile pass would overwrite, causing the old flicker-then-revert).
   * Passing a falsy iri clears the style back to the per-feature COLOR_0 tones.
   */
  function applyHighlight(iri) {
    if (!tileset || !Cesium) return;
    const key = iri || '';
    if (key === lastStyledIri) return; // idempotent — don't force a re-render
    lastStyledIri = key;
    if (key) {
      const esc = String(key).replace(/'/g, "\\'");
      // HIGHLIGHT (multiply) blend mode: white is the identity, so non-matched
      // buildings keep their COLOR_0 tone while the matched feature multiplies to
      // a clear orange. (The COLOR_0 tones are light pastels, so orange × tone
      // still reads unmistakably orange against the pale neighbours.)
      tileset.style = new Cesium.Cesium3DTileStyle({
        color: {
          conditions: [
            [`\${iri} === '${esc}'`, `color('${HIGHLIGHT_COLOR_CSS}')`],
            ['true', "color('#ffffff')"],
          ],
        },
      });
    } else {
      tileset.style = undefined;
    }
    viewer?.scene.requestRender();
  }

  async function selectFeature(iri, label) {
    selectedIri = iri;
    selectedLabel = label || shortenIRI(iri);
    rows = [];
    queryError = '';
    queryLoading = true;
    try {
      const query =
        'SELECT ?p ?o WHERE { <' + iri.replace(/>/g, '%3E') + '> ?p ?o } LIMIT 500';
      const res = await fetch(`/sparql?query=${encodeURIComponent(query)}`, {
        method: 'GET',
        headers: { Accept: 'application/sparql-results+json' },
        credentials: 'include',
      });
      if (!res.ok) throw new Error(`SPARQL ${res.status}`);
      const json = await res.json();
      const bindings = json?.results?.bindings || [];
      rows = bindings.map((b) => {
        const o = b.o?.value ?? '';
        // Blank nodes ('_:b0' / bnode-typed bindings) are not dereferenceable, so
        // they render as plain text rather than a /resource link.
        const isBnode = b.o?.type === 'bnode' || o.startsWith('_:');
        return {
          p: b.p?.value ?? '',
          o,
          isIri: b.o?.type === 'uri' && !isBnode,
          isBnode,
        };
      });
    } catch (e) {
      queryError = e?.message || 'Query failed.';
    } finally {
      queryLoading = false;
    }
  }

  function closePanel() {
    selectedIri = '';
    selectedLabel = '';
    rows = [];
    queryError = '';
  }

  // Imagery toggle: streets (OSM) vs satellite (Esri). Cesium composites raster
  // layers natively — no custom layer to rebuild.
  function setBaseLayer(kind) {
    if (kind === baseLayer || !viewer || !Cesium) return;
    baseLayer = kind;
    applyBaseLayer();
  }

  function onKeydown(e) {
    // Escape closes the standalone feature panel. (No global 'r' shortcut — it
    // would hijack the key app-wide while the embedded viewer is mounted; the
    // Home button + double-click-empty-space already reset the view.)
    if (e.key === 'Escape' && !embedded && selectedIri) {
      closePanel();
      e.stopPropagation();
    }
  }

  onMount(() => {
    init();
  });

  // Reflect the active selection into the scene: the parent's `selected` (embedded)
  // or the local `selectedIri` (standalone). applyHighlight is idempotent.
  $: if (tileset && Cesium) applyHighlight(embedded ? selected || '' : selectedIri);

  onDestroy(() => {
    unsubTheme();
    handler?.destroy?.();
    handler = null;
    if (viewer && !viewer.isDestroyed?.()) viewer.destroy();
    viewer = null;
    tileset = null;
  });
</script>

<svelte:window on:keydown={onKeydown} />

<div class="cesium-wrap" class:dark style:height>
  <div bind:this={containerEl} class="cesium-canvas" role="application" aria-label="3D tiles viewer"></div>

  <div class="cesium-overlay-zone" aria-live="polite" aria-busy={loading}>
    {#if loading}
      <div class="cesium-overlay">
        <Loader2 size={22} class="spin" />
        <span>Loading 3D tiles…</span>
      </div>
    {:else if error}
      <div class="cesium-overlay error">
        <Boxes size={26} />
        <span>{error}</span>
      </div>
    {:else if empty}
      <div class="cesium-overlay empty">
        <Boxes size={28} />
        <p class="empty-title">No 3D-Tiles geometry yet</p>
        <p class="empty-sub">This dataset has no streamable 3D-Tiles content — try the Map view for its other geometry.</p>
      </div>
    {/if}
  </div>

  {#if !loading && !error && !empty && showHint}
    <div class="cesium-hint" role="status">
      <MousePointerClick size={13} /> Click a building to inspect its linked data
    </div>
  {/if}

  <!-- Imagery toggle (streets vs satellite). Both token-free. -->
  <div class="seg-toggle base-toggle" role="group" aria-label="Base imagery">
    <button
      class:active={baseLayer === 'streets'}
      title="Street map"
      aria-label="Street map"
      on:click={() => setBaseLayer('streets')}
    ><MapPin size={14} /></button>
    <button
      class:active={baseLayer === 'satellite'}
      title="Satellite imagery"
      aria-label="Satellite imagery"
      on:click={() => setBaseLayer('satellite')}
    ><Satellite size={14} /></button>
  </div>

  <!-- Camera controls: home / zoom (and full-screen when embedded). There is no
       recovery otherwise once the user orbits away from a small tileset. -->
  {#if !loading && !error}
    <div class="cam-controls">
      {#if embedded && expand}
        <button class="cam-btn" title="Open full screen" aria-label="Open full screen" on:click={openFullScreen}>
          <Maximize2 size={15} />
        </button>
      {/if}
      <button class="cam-btn" title="Reset view" aria-label="Reset view" on:click={flyHome}>
        <Home size={15} />
      </button>
      <div class="cam-zoom">
        <button class="cam-btn" title="Zoom in" aria-label="Zoom in" on:click={() => zoomBy(-1)}><Plus size={15} /></button>
        <button class="cam-btn" title="Zoom out" aria-label="Zoom out" on:click={() => zoomBy(1)}><Minus size={15} /></button>
      </div>
    </div>
  {/if}

  {#if selectedIri && !embedded}
    <aside class="info-panel" aria-label="Feature properties">
      <header class="info-head">
        <Boxes size={14} />
        <span class="info-title" title={selectedIri}>{selectedLabel}</span>
        <button
          class="info-link"
          on:click={() => openSparkExplain({ iri: selectedIri, label: selectedLabel })}
          title="Ask Spark to explain"
          aria-label="Ask Spark to explain"
        >
          <Sparkles size={13} />
        </button>
        <a class="info-link" href={`/resource?iri=${encodeURIComponent(selectedIri)}`} target="_blank" rel="noopener" title="Open resource">
          <ExternalLink size={13} />
        </a>
        <button class="info-close" on:click={closePanel} aria-label="Close">
          <X size={14} />
        </button>
      </header>
      <div class="info-body">
        {#if queryLoading}
          <p class="info-hint"><Loader2 size={14} class="spin" /> Loading…</p>
        {:else if queryError}
          <p class="info-hint err">{queryError}</p>
        {:else if rows.length === 0}
          <p class="info-hint">No properties found.</p>
        {:else}
          <table class="info-table">
            <tbody>
              {#each rows as r}
                <tr>
                  <th>
                    <button class="pred-link" on:click={() => openResource(r.p)} title={r.p}>{shortenIRI(r.p)}</button>
                  </th>
                  <td title={r.o}>
                    {#if r.isIri}
                      <a href={`/resource?iri=${encodeURIComponent(r.o)}`} target="_blank" rel="noopener">{shortenIRI(r.o)}</a>
                    {:else if r.isBnode}
                      <span class="bnode" title="Blank node (not dereferenceable)">{r.o}</span>
                    {:else}
                      {r.o}
                    {/if}
                  </td>
                </tr>
              {/each}
            </tbody>
          </table>
        {/if}
      </div>
    </aside>
  {/if}
</div>

<style>
  .cesium-wrap {
    position: relative;
    width: 100%;
    min-height: 280px;
    border-radius: var(--radius-lg, 12px);
    overflow: hidden;
    background: var(--bg-soft, #0b1118);
    --hud-surface: rgba(255, 255, 255, 0.96);
    --hud-ink: var(--ink-900, #0f172a);
    --hud-muted: var(--muted, #64748b);
    --hud-line: var(--line-soft, #e6eaef);
    --hud-active-bg: var(--bg-accent-soft, #e7f0fb);
    --hud-active-ink: var(--brand-600, #2563a8);
  }
  /* Dark HUD so the floating controls/panel don't float bright-white over the
     globe in dark mode. */
  .cesium-wrap.dark {
    --hud-surface: rgba(16, 23, 33, 0.94);
    --hud-ink: #e2e8f0;
    --hud-muted: #94a3b8;
    --hud-line: rgba(255, 255, 255, 0.12);
    --hud-active-bg: rgba(47, 136, 216, 0.22);
    --hud-active-ink: #7ec3e8;
  }
  .cesium-canvas {
    position: absolute;
    inset: 0;
  }
  /* Tame Cesium's bottom credit bar so it blends with the app surface. */
  :global(.cesium-canvas .cesium-widget-credits) {
    font-size: 0.62rem;
    opacity: 0.7;
  }

  .cesium-overlay-zone {
    position: absolute;
    inset: 0;
    z-index: 6;
    pointer-events: none;
    display: flex;
  }
  .cesium-overlay {
    margin: auto;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 0.5rem;
    padding: 1.4rem 1.8rem;
    color: var(--ink-100, #e2e8f0);
    background: rgba(11, 17, 24, 0.62);
    border-radius: 14px;
    font-size: 0.85rem;
    text-align: center;
    max-width: min(340px, 80%);
  }
  .cesium-overlay.error {
    color: #ffd7c9;
    background: rgba(40, 12, 8, 0.66);
  }
  .cesium-overlay.empty {
    color: #cdd9e6;
  }
  .empty-title {
    margin: 0.2rem 0 0;
    font-weight: 600;
    font-size: 0.95rem;
  }
  .empty-sub {
    margin: 0;
    font-size: 0.78rem;
    opacity: 0.8;
    line-height: 1.4;
  }
  :global(.cesium-wrap .spin) {
    animation: cv-spin 1s linear infinite;
  }
  @keyframes cv-spin {
    to {
      transform: rotate(360deg);
    }
  }
  @media (prefers-reduced-motion: reduce) {
    :global(.cesium-wrap .spin) {
      animation: none;
    }
  }

  /* One-time "click a building" affordance. */
  .cesium-hint {
    position: absolute;
    left: 50%;
    bottom: 16px;
    transform: translateX(-50%);
    z-index: 5;
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 6px 13px;
    border-radius: 999px;
    background: var(--hud-surface);
    color: var(--hud-ink);
    border: 1px solid var(--hud-line);
    box-shadow: var(--shadow-sm, 0 2px 8px rgba(0, 0, 0, 0.18));
    font-size: 0.76rem;
    backdrop-filter: blur(8px);
    animation: hintIn 0.3s ease both;
  }
  @keyframes hintIn {
    from { opacity: 0; transform: translate(-50%, 6px); }
    to { opacity: 1; transform: translate(-50%, 0); }
  }
  @media (prefers-reduced-motion: reduce) {
    .cesium-hint { animation: none; }
  }

  /* Shared segmented-toggle look (also used by the map basemap toggle). */
  .seg-toggle {
    position: absolute;
    top: 10px;
    left: 10px;
    z-index: 5;
    display: flex;
    border-radius: var(--radius-sm, 8px);
    overflow: hidden;
    box-shadow: var(--shadow-sm, 0 2px 8px rgba(0, 0, 0, 0.22));
    border: 1px solid var(--hud-line);
  }
  .seg-toggle button {
    border: 0;
    padding: 7px 10px;
    background: var(--hud-surface);
    color: var(--hud-muted);
    cursor: pointer;
    display: flex;
    align-items: center;
  }
  .seg-toggle button + button {
    border-left: 1px solid var(--hud-line);
  }
  .seg-toggle button:hover {
    color: var(--hud-ink);
  }
  .seg-toggle button.active {
    background: var(--hud-active-bg);
    color: var(--hud-active-ink);
  }
  .seg-toggle button:focus-visible,
  .cam-btn:focus-visible {
    outline: none;
    box-shadow: 0 0 0 2px #0c121c, 0 0 0 4px var(--brand-400, #5aa9e0);
  }

  /* Camera control cluster (bottom-right). */
  .cam-controls {
    position: absolute;
    right: 10px;
    bottom: 26px;
    z-index: 5;
    display: flex;
    flex-direction: column;
    gap: 6px;
    align-items: stretch;
  }
  .cam-zoom {
    display: flex;
    flex-direction: column;
    border-radius: var(--radius-sm, 8px);
    overflow: hidden;
    box-shadow: var(--shadow-sm, 0 2px 8px rgba(0, 0, 0, 0.22));
    border: 1px solid var(--hud-line);
  }
  .cam-zoom .cam-btn {
    border-radius: 0;
    box-shadow: none;
    border: 0;
  }
  .cam-zoom .cam-btn + .cam-btn {
    border-top: 1px solid var(--hud-line);
  }
  .cam-btn {
    width: 34px;
    height: 32px;
    display: grid;
    place-items: center;
    border: 1px solid var(--hud-line);
    border-radius: var(--radius-sm, 8px);
    background: var(--hud-surface);
    color: var(--hud-muted);
    cursor: pointer;
    box-shadow: var(--shadow-sm, 0 2px 8px rgba(0, 0, 0, 0.22));
  }
  .cam-btn:hover {
    color: var(--hud-active-ink);
  }

  /* Feature-properties side panel (predicate/object table). */
  .info-panel {
    position: absolute;
    top: 10px;
    right: 10px;
    bottom: 10px;
    z-index: 6;
    width: min(340px, 42%);
    display: flex;
    flex-direction: column;
    border-radius: var(--radius-md, 12px);
    background: var(--hud-surface);
    border: 1px solid var(--hud-line);
    box-shadow: var(--shadow-md, 0 4px 18px rgba(0, 0, 0, 0.28));
    backdrop-filter: blur(8px);
    overflow: hidden;
    color: var(--hud-ink);
  }
  .info-head {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 8px 10px;
    border-bottom: 1px solid var(--hud-line);
    color: var(--hud-ink);
  }
  .info-title {
    flex: 1;
    min-width: 0;
    font-weight: 600;
    font-size: 0.85rem;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .info-link,
  .info-close {
    display: grid;
    place-items: center;
    border: 0;
    background: transparent;
    color: var(--hud-muted);
    cursor: pointer;
    padding: 2px;
    border-radius: 6px;
    text-decoration: none;
  }
  .info-link:hover,
  .info-close:hover {
    color: var(--hud-ink);
    background: var(--bg-hover, rgba(0, 0, 0, 0.05));
  }
  .info-link:focus-visible,
  .info-close:focus-visible,
  .pred-link:focus-visible {
    outline: none;
    box-shadow: 0 0 0 2px var(--brand-400, #5aa9e0);
    border-radius: 6px;
  }
  .info-body {
    flex: 1;
    min-height: 0;
    overflow: auto;
    padding: 4px 0;
  }
  .info-hint {
    margin: 0;
    padding: 10px 12px;
    color: var(--hud-muted);
    font-size: 0.8rem;
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .info-hint.err {
    color: var(--danger-500, #c0392b);
  }
  .info-table {
    width: 100%;
    border-collapse: collapse;
    font-size: 0.78rem;
  }
  .info-table th,
  .info-table td {
    text-align: left;
    vertical-align: top;
    padding: 5px 10px;
    border-bottom: 1px solid var(--hud-line);
    overflow-wrap: anywhere;
  }
  .info-table th {
    width: 42%;
    font-weight: 600;
    color: var(--hud-ink);
    opacity: 0.85;
  }
  .info-table td {
    color: var(--hud-ink);
  }
  .info-table a {
    color: var(--hud-active-ink);
    text-decoration: none;
  }
  .info-table a:hover {
    text-decoration: underline;
  }
  /* Predicate cell is a button that opens its term's resource page. */
  .pred-link {
    border: 0;
    background: none;
    padding: 0;
    margin: 0;
    font: inherit;
    text-align: left;
    color: var(--hud-ink);
    opacity: 0.85;
    cursor: pointer;
    overflow-wrap: anywhere;
  }
  .pred-link:hover {
    color: var(--hud-active-ink);
    opacity: 1;
    text-decoration: underline;
  }
  .bnode {
    color: var(--hud-muted);
    font-style: italic;
  }

  @media (max-width: 640px) {
    .info-panel {
      width: auto;
      left: 10px;
      top: auto;
      height: 45%;
    }
  }
</style>
