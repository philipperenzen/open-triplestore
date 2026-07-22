<script>
  import { onDestroy } from 'svelte';
  // Not `leaflet` directly: this wrapper re-exports the library with its default
  // marker icons resolved by the bundler. Importing plain `leaflet` here is what
  // made every point marker render as a broken image in production builds — see
  // lib/viewer/leafletIcons.ts for the full explanation.
  import L from '../lib/viewer/leafletIcons';
  import 'leaflet/dist/leaflet.css';
  import { t as i18nT } from 'svelte-i18n';
  import { isDark } from '../lib/theme.js';
  import { geometryCoords } from '../lib/ontology/valueType.js';
  import { parseWktAsWgs84 } from '../lib/viewer/crs';
  import { leafletTiles } from '../lib/viewer/basemaps';

  export let wkts = [];
  export let height = '220px';
  /** When set (metres), a "to scale" toggle draws point markers as real-size
   *  circles (L.circle, radius in metres) instead of fixed-pixel markers. */
  export let scaleMeters = 0;

  let mapEl;
  let map = null;
  let tiles = null;
  let failed = false;
  let geometries = [];
  let toScale = false;
  let drawnLayers = [];
  let ro = null;
  // Set when a draw was skipped because the container had no layout yet; the
  // ResizeObserver replays it the moment the element gains a size.
  let pendingDraw = false;
  let drawFrame = 0;
  let drawRetries = 0;
  // Enough frames (~0.5s at 60fps) for a container that is merely a layout pass
  // behind, but bounded so a permanently zero-sized host (a collapsed accordion,
  // a hidden tab) cannot leave us spinning a requestAnimationFrame loop forever.
  const MAX_DRAW_RETRY_FRAMES = 30;

  // CRS-aware: projected WKT (e.g. the Waalbrug demo's EPSG:28992) is
  // reprojected to WGS84 before plotting.
  $: geometries = (wkts || [])
    .map(w => parseWktAsWgs84(w))
    .filter(Boolean);

  // Tiles follow the app theme (light OSM / dark Carto) and swap live.
  const unsubTheme = isDark.subscribe((dark) => {
    if (!map) return;
    if (tiles) tiles.remove();
    const t = leafletTiles(dark);
    tiles = L.tileLayer(t.url, { maxZoom: 19, attribution: t.attribution }).addTo(map);
  });

  const toLatLng = (coords) => coords.map(([lng, lat]) => [lat, lng]);
  const add = (layer) => {
    layer.addTo(map);
    drawnLayers.push(layer);
  };
  const point = ([lng, lat]) =>
    toScale && scaleMeters > 0
      ? add(L.circle([lat, lng], { radius: scaleMeters / 2, color: '#e8590c', weight: 2, fillOpacity: 0.18 }))
      : add(L.marker([lat, lng]));
  function drawGeometry(g) {
    if (!g) return;
    switch (g.kind) {
      case 'point':
        point(g.coord);
        break;
      case 'multipoint':
        for (const c of g.coords) point(c);
        break;
      case 'linestring':
        add(L.polyline(toLatLng(g.coords), { color: '#4a90d9' }));
        break;
      case 'multilinestring':
        for (const line of g.lines) add(L.polyline(toLatLng(line), { color: '#4a90d9' }));
        break;
      case 'polygon':
        add(L.polygon(g.rings.map(toLatLng), { color: '#6a5acd', weight: 2, fillOpacity: 0.15 }));
        break;
      case 'multipolygon':
        for (const poly of g.polygons)
          add(L.polygon(poly.map(toLatLng), { color: '#6a5acd', weight: 2, fillOpacity: 0.15 }));
        break;
      case 'geometrycollection':
        for (const sub of g.geometries) drawGeometry(sub);
        break;
    }
  }

  function drawAll() {
    if (!map || !mapEl) return;
    // Drawing into a container that has not been laid out yet is worse than not
    // drawing: `fitBounds` subtracts its padding from the container size, so on a
    // 0×0 container `getBoundsZoom` goes negative and the map parks at a NaN zoom
    // — permanently blank, with nothing that would ever recover it. Defer instead.
    //
    // Measure the *element*, never `map.getSize()`. Leaflet memoises the size and
    // only re-measures when something marks it dirty, and `invalidateSize()` —
    // the only public way to do that — returns early while the map has no view
    // yet, which is precisely the state of a map that has not drawn. Asking
    // `getSize()` here would therefore cache a 0×0 forever and wedge the map shut
    // (verified against leaflet 1.9.4: `_size` is undefined until the first
    // `getSize()`, so leaving it untouched lets the first real draw measure
    // correctly). The ResizeObserver in initMap() is what wakes us up; the
    // bounded frame retry only covers hosts that have no ResizeObserver.
    if (!mapEl.clientWidth || !mapEl.clientHeight) {
      pendingDraw = true;
      if (!drawFrame && drawRetries < MAX_DRAW_RETRY_FRAMES && typeof requestAnimationFrame === 'function') {
        drawRetries += 1;
        drawFrame = requestAnimationFrame(() => {
          drawFrame = 0;
          if (map && pendingDraw) drawAll();
        });
      }
      return;
    }
    pendingDraw = false;
    drawRetries = 0;
    for (const l of drawnLayers) l.remove();
    drawnLayers = [];
    for (const g of geometries) drawGeometry(g);
    // Refit so a *changed* geometry set (the singleton preview overlay reuses
    // one mounted instance) is centred, not left at the previous extent.
    const all = [];
    for (const g of geometries) for (const [lng, lat] of geometryCoords(g)) all.push([lat, lng]);
    if (all.length === 1) map.setView(all[0], 12);
    else if (all.length > 1) map.fitBounds(all, { padding: [20, 20] });
    else map.setView([0, 0], 1);
  }

  // Leaflet is a bundled npm dependency (was: CDN-loaded at runtime), so the
  // map works offline and under a strict CSP; only the OSM tiles need network.
  function initMap() {
    try {
      map = L.map(mapEl, { scrollWheelZoom: false, attributionControl: true });
      const t = leafletTiles($isDark);
      tiles = L.tileLayer(t.url, { maxZoom: 19, attribution: t.attribution }).addTo(map);
      observeSize();
    } catch (_) {
      failed = true;
      map = null;
    }
  }

  // The container changes size without any window resize: the preview overlay's
  // maximise/restore toggle, a dragged or resized inspector panel, a tab that
  // reveals the map later. Leaflet 1.x caches the container size and re-measures
  // only on an explicit invalidateSize() or a *window* resize event — it has no
  // ResizeObserver of its own — so without this the map keeps painting tiles and
  // markers into the old rectangle and the rest of the panel stays empty.
  // Feature-guarded because jsdom (vitest) provides no ResizeObserver.
  function observeSize() {
    if (typeof ResizeObserver === 'undefined' || !mapEl) return;
    ro = new ResizeObserver(() => {
      if (!map) return;
      // `animate: false` because this is a layout correction, not a user gesture.
      // `pan` stays at its default (true) on purpose: it re-anchors the view on
      // the same geographic centre, which is what a maximise/restore should do.
      map.invalidateSize({ animate: false });
      // invalidateSize() is a no-op until the map has a view, so a first draw
      // that was deferred for want of layout has to be replayed by hand.
      if (pendingDraw) drawAll();
    });
    ro.observe(mapEl);
  }

  function destroyMap() {
    if (ro) { try { ro.disconnect(); } catch {} ro = null; }
    if (drawFrame && typeof cancelAnimationFrame === 'function') cancelAnimationFrame(drawFrame);
    drawFrame = 0;
    drawRetries = 0;
    pendingDraw = false;
    if (map) { try { map.remove(); } catch {} }
    map = null;
    tiles = null;
    drawnLayers = [];
  }

  onDestroy(() => {
    unsubTheme();
    destroyMap();
  });

  // Lazy init: a reused instance (the singleton preview overlay keeps one
  // mounted) may start with zero geometries, so create the map only once the
  // map div is bound *and* there is something to draw.
  $: if (!map && !failed && mapEl && geometries.length) initMap();
  // The template drops the map div when geometries empty out — tear down so a
  // later non-empty set re-initialises against the freshly bound element.
  $: if (map && geometries.length === 0) destroyMap();
  // Redraw on geometry changes too — without this, the reused preview-overlay
  // instance keeps showing the previous term's geometry.
  $: if (map && (geometries || toScale !== undefined)) drawAll();
</script>

{#if geometries.length === 0}
  <!-- nothing to show -->
{:else if failed}
  <!-- Only reached when Leaflet itself refuses to initialise (it is bundled, so
       this is not an offline/tile problem — missing tiles just leave grey
       squares). The coordinates stay readable either way. -->
  <div class="geo-fallback">
    <span class="muted">{$i18nT('viewer.mapUnavailable')}</span>
    <ul>
      {#each geometries as g}
        {#if g.kind === 'point'}
          <li>📍 {g.coord[1].toFixed(5)}, {g.coord[0].toFixed(5)}</li>
        {:else}
          <li>{$i18nT('viewer.geometryPointCount', { values: { kind: g.kind, count: geometryCoords(g).length } })}</li>
        {/if}
      {/each}
    </ul>
  </div>
{:else}
  <div class="geo-wrap" style="height: {height}">
    <div bind:this={mapEl} class="map"></div>
    {#if scaleMeters > 0}
      <button class="scale-toggle" class:on={toScale} on:click={() => (toScale = !toScale)}>
        {$i18nT('viewer.toScale')}
      </button>
    {/if}
  </div>
{/if}

<style>
  .geo-wrap { position: relative; }
  .map { width: 100%; height: 100%; border-radius: 10px; border: 1px solid var(--line-soft, #e5e7eb); overflow: hidden; }
  .scale-toggle {
    position: absolute; right: 8px; top: 8px; z-index: 500;
    font-size: 0.7rem; padding: 3px 10px; border-radius: 99px; cursor: pointer;
    border: 1px solid var(--border, #d6dde4);
    background: var(--bg-elevated, #fff); color: var(--muted, #64748b);
    box-shadow: var(--shadow-sm, 0 1px 3px rgba(0,0,0,0.12));
  }
  .scale-toggle.on { color: #e8590c; border-color: #e8590c; font-weight: 600; }
  .scale-toggle:focus-visible {
    outline: none;
    box-shadow: 0 0 0 2px var(--bg-elevated, #fff), 0 0 0 4px var(--brand-400, #5aa9e0);
  }
  /* Themed like the rest of the app: the literal light-grey values this used to
     hard-code were unreadable in dark mode. */
  .geo-fallback {
    padding: 0.75rem;
    border: 1px dashed var(--line-soft, #e5e7eb);
    border-radius: 10px;
    background: var(--bg-subtle, #fafafa);
    color: var(--ink-900, #0f172a);
    font-size: 0.85rem;
  }
  .geo-fallback ul { margin: 0.4rem 0 0; padding-left: 1rem; }
  .muted { color: var(--muted, #64748b); }
</style>
