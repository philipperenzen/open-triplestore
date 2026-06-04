<script>
  import { onMount, onDestroy } from 'svelte';
  import { parseWktGeometry, geometryCoords } from '../lib/ontology/valueType.js';

  export let wkts = [];
  export let height = '220px';

  let mapEl;
  let map = null;
  let failed = false;
  let geometries = [];

  $: geometries = (wkts || [])
    .map(w => parseWktGeometry(w))
    .filter(Boolean);

  const LEAFLET_JS = 'https://unpkg.com/leaflet@1.9.4/dist/leaflet.js';
  const LEAFLET_CSS = 'https://unpkg.com/leaflet@1.9.4/dist/leaflet.css';

  let leafletPromise;
  function loadLeaflet() {
    if (typeof window === 'undefined') return Promise.reject(new Error('ssr'));
    if (window.L) return Promise.resolve(window.L);
    if (leafletPromise) return leafletPromise;
    leafletPromise = new Promise((resolve, reject) => {
      if (!document.querySelector(`link[href="${LEAFLET_CSS}"]`)) {
        const link = document.createElement('link');
        link.rel = 'stylesheet';
        link.href = LEAFLET_CSS;
        document.head.appendChild(link);
      }
      const script = document.createElement('script');
      script.src = LEAFLET_JS;
      script.async = true;
      script.onload = () => resolve(window.L);
      script.onerror = () => reject(new Error('failed to load leaflet'));
      document.head.appendChild(script);
    });
    return leafletPromise;
  }

  onMount(async () => {
    if (geometries.length === 0) return;
    try {
      const L = await loadLeaflet();
      if (!mapEl) return;
      map = L.map(mapEl, { scrollWheelZoom: false, attributionControl: true });
      L.tileLayer('https://tile.openstreetmap.org/{z}/{x}/{y}.png', {
        maxZoom: 19,
        attribution: '&copy; OpenStreetMap',
      }).addTo(map);

      const toLatLng = (coords) => coords.map(([lng, lat]) => [lat, lng]);
      const drawGeometry = (g) => {
        if (!g) return;
        switch (g.kind) {
          case 'point':
            L.marker([g.coord[1], g.coord[0]]).addTo(map);
            break;
          case 'multipoint':
            for (const [lng, lat] of g.coords) L.marker([lat, lng]).addTo(map);
            break;
          case 'linestring':
            L.polyline(toLatLng(g.coords), { color: '#4a90d9' }).addTo(map);
            break;
          case 'multilinestring':
            for (const line of g.lines) L.polyline(toLatLng(line), { color: '#4a90d9' }).addTo(map);
            break;
          case 'polygon':
            L.polygon(g.rings.map(toLatLng), { color: '#6a5acd', weight: 2, fillOpacity: 0.15 }).addTo(map);
            break;
          case 'multipolygon':
            for (const poly of g.polygons)
              L.polygon(poly.map(toLatLng), { color: '#6a5acd', weight: 2, fillOpacity: 0.15 }).addTo(map);
            break;
          case 'geometrycollection':
            for (const sub of g.geometries) drawGeometry(sub);
            break;
        }
      };

      const allLatLngs = [];
      for (const g of geometries) {
        drawGeometry(g);
        for (const [lng, lat] of geometryCoords(g)) allLatLngs.push([lat, lng]);
      }

      if (allLatLngs.length === 1) {
        map.setView(allLatLngs[0], 12);
      } else if (allLatLngs.length > 1) {
        map.fitBounds(allLatLngs, { padding: [20, 20] });
      } else {
        map.setView([0, 0], 1);
      }
    } catch (_) {
      failed = true;
    }
  });

  onDestroy(() => {
    if (map) { try { map.remove(); } catch {} }
  });
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
  <div bind:this={mapEl} class="map" style="height: {height}"></div>
{/if}

<style>
  .map { width: 100%; border-radius: 10px; border: 1px solid var(--line-soft, #e5e7eb); overflow: hidden; }
  .geo-fallback { padding: 0.75rem; border: 1px dashed #e5e7eb; border-radius: 10px; background: #fafafa; font-size: 0.85rem; }
  .geo-fallback ul { margin: 0.4rem 0 0; padding-left: 1rem; }
  .muted { color: #888; }
</style>
