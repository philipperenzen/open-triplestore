<script>
  import { onDestroy } from 'svelte';
  import L from 'leaflet';
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
    } catch (_) {
      failed = true;
      map = null;
    }
  }

  function destroyMap() {
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
  <div class="geo-fallback">
    <span class="muted">Map unavailable (offline?). Showing coordinates:</span>
    <ul>
      {#each geometries as g}
        {#if g.kind === 'point'}
          <li>📍 {g.coord[1].toFixed(5)}, {g.coord[0].toFixed(5)}</li>
        {:else}
          <li>{g.kind} ({geometryCoords(g).length} pts)</li>
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
  .geo-fallback { padding: 0.75rem; border: 1px dashed #e5e7eb; border-radius: 10px; background: #fafafa; font-size: 0.85rem; }
  .geo-fallback ul { margin: 0.4rem 0 0; padding-left: 1rem; }
  .muted { color: #888; }
</style>
