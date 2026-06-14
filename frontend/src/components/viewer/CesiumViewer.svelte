<script>
  // CesiumJS 3D-Tiles viewer for a dataset. Loads the dataset's tileset
  // (/api/datasets/:id/3dtiles/tileset.json), and on click resolves the picked
  // feature's stable per-feature IRI (the EXT_structural_metadata `iri`
  // property — the same key that is the RDF subject and the viewer lookup key)
  // and runs a SPARQL DESCRIBE-style query for it, showing predicate/object
  // pairs in a side panel.
  //
  // Cesium owns depth, terrain and globe occlusion natively, so this path does
  // NOT suffer the MapLibre+three.js satellite/tilt/z-fight failure modes the
  // 2D viewer works around — switching imagery here is a plain layer swap with
  // no custom WebGL layer to collapse.
  import { onMount, onDestroy, createEventDispatcher } from 'svelte';
  import { Boxes, X, MapPin, Satellite, ExternalLink, Sparkles, Loader2 } from 'lucide-svelte';
  import { shortenIRI } from '../../lib/rdf-utils.ts';
  import { navigate } from '../../lib/router/index.js';
  import { openSparkExplain } from '../../lib/sparkHelp.js';

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
  // `select` to the parent (which opens the inspector, with the IFC/BOT
  // decomposition + sub-element tree) instead of showing this component's own
  // lightweight SPARQL panel — so switching to 3D Tiles sacrifices no features.
  export let embedded = false;
  /** Currently-selected element id (from the parent) to highlight in the scene. */
  export let selected = '';

  const dispatch = createEventDispatcher();

  // Cesium fetches its workers / glsl / Assets relative to CESIUM_BASE_URL. We
  // point it at the matching CDN build so the static assets resolve without any
  // bundler/copy-plugin configuration.
  const CESIUM_VERSION = '1.123.0';
  const CESIUM_BASE_URL = `https://cdn.jsdelivr.net/npm/cesium@${CESIUM_VERSION}/Build/Cesium/`;

  let containerEl;
  let viewer = null;
  let tileset = null;
  let Cesium = null;
  let handler = null;

  let loading = true;
  let error = '';
  let baseLayer = 'satellite'; // 'satellite' | 'streets'

  // Side panel state for the picked feature.
  let selectedIri = '';
  let selectedLabel = '';
  let rows = []; // { p, o, isIri }
  let queryLoading = false;
  let queryError = '';

  // The currently-highlighted feature (so we can clear its colour on the next pick).
  let highlighted = null;
  const HIGHLIGHT_COLOR_CSS = '#e8590c';

  async function init() {
    try {
      // Set the base URL before any Cesium module touches it.
      window.CESIUM_BASE_URL = CESIUM_BASE_URL;
      Cesium = await import('cesium');
      await import('cesium/Build/Cesium/Widgets/widgets.css');

      viewer = new Cesium.Viewer(containerEl, {
        // World imagery is the default base layer; terrain stays off (flat
        // ellipsoid) so a tileset georeferenced to ground sits correctly without
        // a terrain-provider round-trip.
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
      });
      // Hide the default Cesium credit overlay clutter; keep the logo.
      viewer.scene.globe.depthTestAgainstTerrain = false;
      // Soften the lighting. Cesium's default sun (intensity 2.0) blows the flat,
      // sun-facing roofs of our short tiles out to pure white when viewed near
      // top-down. A dimmer sun + the GLB material's small emissive floor make the
      // tiles read as shaded grey buildings instead of white squares.
      viewer.scene.light = new Cesium.SunLight({ intensity: 0.5 });

      await loadTileset();
      attachPicking();
    } catch (e) {
      error = e?.message || 'Failed to initialise the 3D viewer.';
    } finally {
      // Clear the overlay once the tileset is in the scene — the camera flight
      // below must NOT gate the UI (zoomTo resolves only after the flight, which
      // can stall on a degenerate bounding volume).
      loading = false;
    }
  }

  async function loadTileset() {
    if (tileset) {
      viewer.scene.primitives.remove(tileset);
      tileset = null;
    }
    tileset = await Cesium.Cesium3DTileset.fromUrl(
      `/api/datasets/${encodeURIComponent(datasetId)}/3dtiles/tileset.json`,
    );
    viewer.scene.primitives.add(tileset);
    // Fly to the tileset without blocking init: zoomTo awaits the camera flight,
    // which can hang. Prefer the ready tileset's boundingSphere; fall back to
    // zoomTo. Either way init() proceeds and the overlay clears.
    try {
      const bs = tileset.boundingSphere;
      if (bs && Number.isFinite(bs.radius) && bs.radius > 0) {
        viewer.camera.flyToBoundingSphere(bs, { duration: 0 });
      } else {
        viewer.zoomTo(tileset).catch(() => {});
      }
    } catch {
      viewer.zoomTo(tileset).catch(() => {});
    }
  }

  function attachPicking() {
    handler = new Cesium.ScreenSpaceEventHandler(viewer.scene.canvas);
    handler.setInputAction((movement) => {
      const picked = viewer.scene.pick(movement.position);
      // A 3D-Tiles feature exposes per-feature metadata via getProperty(); the
      // binding key is the `iri` property written by the tiling pipeline (P5).
      if (picked instanceof Cesium.Cesium3DTileFeature) {
        const iri = picked.getProperty('iri');
        const label =
          picked.getProperty('label') ||
          picked.getProperty('name') ||
          (iri ? shortenIRI(iri) : '');
        highlightFeature(picked);
        if (iri) {
          // Embedded: hand the pick to the parent's rich inspector. Standalone:
          // show this component's own SPARQL property panel.
          if (embedded) dispatch('select', { id: iri });
          else selectFeature(iri, label);
        }
        return;
      }
      // Clicking empty space clears the selection + highlight.
      clearHighlight();
      if (!embedded) closePanel();
    }, Cesium.ScreenSpaceEventType.LEFT_CLICK);
  }

  function highlightFeature(feature) {
    clearHighlight();
    try {
      highlighted = feature;
      feature.color = Cesium.Color.fromCssColorString(HIGHLIGHT_COLOR_CSS);
    } catch {
      highlighted = null;
    }
  }

  function clearHighlight() {
    if (highlighted) {
      try {
        highlighted.color = Cesium.Color.WHITE;
      } catch {
        /* feature may be unloaded after a tile evicts */
      }
      highlighted = null;
    }
  }

  // Reverse binding stub: colour every feature whose `iri` matches via a 3D
  // Tiles Style, so an external selection (e.g. a SPARQL result row) can be
  // reflected back into the scene.
  // TODO(P7): drive this from the data-explorer selection store.
  export function styleByIri(iri, cssColor = HIGHLIGHT_COLOR_CSS) {
    if (!tileset || !Cesium || !iri) return;
    const esc = String(iri).replace(/'/g, "\\'");
    tileset.style = new Cesium.Cesium3DTileStyle({
      color: {
        conditions: [
          [`\${iri} === '${esc}'`, `color('${cssColor}')`],
          ['true', "color('#ffffff')"],
        ],
      },
    });
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

  // Imagery toggle: world satellite imagery vs a simple OSM raster basemap.
  // Cesium composites raster layers natively — no custom layer to rebuild — so
  // this is the depth-correct replacement for the old MapLibre satellite toggle.
  function setBaseLayer(kind) {
    if (kind === baseLayer || !viewer || !Cesium) return;
    baseLayer = kind;
    const layers = viewer.imageryLayers;
    layers.removeAll();
    if (kind === 'streets') {
      layers.addImageryProvider(
        new Cesium.OpenStreetMapImageryProvider({ url: 'https://tile.openstreetmap.org/' }),
      );
    } else {
      // Default Cesium ion world imagery.
      layers.addImageryProvider(Cesium.ImageryLayer.fromWorldImagery({}).imageryProvider);
    }
  }

  onMount(() => {
    init();
  });

  // Embedded mode: reflect the parent explorer's selection into the scene by
  // colour-styling the matching feature (reverse binding §7.3).
  $: if (embedded && tileset && Cesium) styleByIri(selected || '');

  onDestroy(() => {
    handler?.destroy?.();
    handler = null;
    if (viewer && !viewer.isDestroyed?.()) viewer.destroy();
    viewer = null;
    tileset = null;
  });
</script>

<div class="cesium-wrap" style:height>
  <div bind:this={containerEl} class="cesium-canvas" role="application" aria-label="3D tiles viewer"></div>

  {#if loading}
    <div class="cesium-overlay">
      <Loader2 size={22} class="spin" />
      <span>Loading 3D tiles…</span>
    </div>
  {:else if error}
    <div class="cesium-overlay error">
      <span>{error}</span>
    </div>
  {/if}

  <!-- Imagery toggle (satellite vs streets). Depth/terrain are native, so this
       swap can't collapse the scene the way the 2D viewer's custom layer could. -->
  <div class="base-toggle" role="group" aria-label="Base imagery">
    <button
      class:active={baseLayer === 'satellite'}
      title="Satellite imagery"
      aria-label="Satellite imagery"
      on:click={() => setBaseLayer('satellite')}
    ><Satellite size={14} /></button>
    <button
      class:active={baseLayer === 'streets'}
      title="Street map"
      aria-label="Street map"
      on:click={() => setBaseLayer('streets')}
    ><MapPin size={14} /></button>
  </div>

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
          <p class="info-hint"><Loader2 size={14} class="spin" /> …</p>
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

  .cesium-overlay {
    position: absolute;
    inset: 0;
    z-index: 6;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 0.5rem;
    color: var(--ink-100, #e2e8f0);
    background: rgba(11, 17, 24, 0.55);
    font-size: 0.85rem;
  }
  .cesium-overlay.error {
    color: #ffd7c9;
    background: rgba(40, 12, 8, 0.6);
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

  .base-toggle {
    position: absolute;
    top: 10px;
    left: 10px;
    z-index: 5;
    display: flex;
    border-radius: 8px;
    overflow: hidden;
    box-shadow: 0 1px 4px rgba(0, 0, 0, 0.35);
  }
  .base-toggle button {
    border: 0;
    padding: 7px 9px;
    background: var(--bg-elevated, #fff);
    color: var(--muted, #64748b);
    cursor: pointer;
    display: flex;
    align-items: center;
  }
  .base-toggle button + button {
    border-left: 1px solid var(--line-soft, #e6eaef);
  }
  .base-toggle button:hover {
    color: var(--ink-900, #0f172a);
  }
  .base-toggle button.active {
    background: var(--bg-accent-soft, #e7f0fb);
    color: var(--brand-600, #2563a8);
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
    border-radius: 12px;
    background: var(--bg-elevated, rgba(255, 255, 255, 0.97));
    border: 1px solid var(--line-soft, #e6eaef);
    box-shadow: 0 4px 18px rgba(0, 0, 0, 0.28);
    backdrop-filter: blur(8px);
    overflow: hidden;
    color: var(--ink-900, #0f172a);
  }
  .info-head {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 8px 10px;
    border-bottom: 1px solid var(--line-soft, #e6eaef);
    color: var(--ink-900, #0f172a);
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
    color: var(--muted, #64748b);
    cursor: pointer;
    padding: 2px;
    border-radius: 6px;
    text-decoration: none;
  }
  .info-link:hover,
  .info-close:hover {
    color: var(--ink-900, #0f172a);
    background: var(--bg-hover, rgba(0, 0, 0, 0.05));
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
    color: var(--muted, #64748b);
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
    border-bottom: 1px solid var(--line-soft, #eef1f4);
    overflow-wrap: anywhere;
  }
  .info-table th {
    width: 42%;
    font-weight: 600;
    color: var(--ink-700, #334155);
  }
  .info-table td {
    color: var(--ink-900, #0f172a);
  }
  .info-table a {
    color: var(--brand-600, #1d6fb8);
    text-decoration: none;
  }
  .info-table a:hover {
    text-decoration: underline;
  }
  /* Predicate cell is a button that opens its term's resource page. Styled to
     read as a quiet link, not a chrome button. */
  .pred-link {
    border: 0;
    background: none;
    padding: 0;
    margin: 0;
    font: inherit;
    text-align: left;
    color: var(--ink-700, #334155);
    cursor: pointer;
    overflow-wrap: anywhere;
  }
  .pred-link:hover {
    color: var(--brand-600, #1d6fb8);
    text-decoration: underline;
  }
  /* Blank-node object: shown for context but never linked (not dereferenceable). */
  .bnode {
    color: var(--muted, #64748b);
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
