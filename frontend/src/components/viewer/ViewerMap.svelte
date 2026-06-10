<script>
  // Interactive map for the dataset viewer: every element with WGS84 geometry is a
  // clickable feature (circle markers / polylines / polygons), kept in sync with
  // the shared selection. Leaflet is a bundled npm dependency (no CDN).
  import { onMount, onDestroy, createEventDispatcher } from 'svelte';
  import L from 'leaflet';
  import 'leaflet/dist/leaflet.css';
  import { toMapFeature, featureBounds } from '../../lib/viewer/geometry';

  /** @type {import('../../lib/viewer/geometry').ViewerElement[]} */
  export let elements = [];
  /** Currently selected element IRI (two-way synced with the page). */
  export let selected = '';
  export let height = '100%';

  const dispatch = createEventDispatcher();

  let mapEl;
  let map = null;
  /** @type {Map<string, L.Path[]>} element IRI → its Leaflet layers */
  let layersById = new Map();

  const BASE_STYLE = { color: '#4a90d9', weight: 3, fillOpacity: 0.25 };
  const SELECTED_STYLE = { color: '#e8590c', weight: 4, fillOpacity: 0.45 };

  function styleFor(id) {
    return id === selected ? SELECTED_STYLE : BASE_STYLE;
  }

  function rebuildLayers() {
    if (!map) return;
    for (const layers of layersById.values()) layers.forEach((l) => l.remove());
    layersById = new Map();

    const features = elements.map(toMapFeature).filter(Boolean);
    for (const f of features) {
      const layers = [];
      const add = (layer) => {
        layer.bindTooltip(f.label);
        layer.on('click', () => dispatch('select', { id: f.id }));
        layer.addTo(map);
        layers.push(layer);
      };
      if (f.kind === 'point') {
        for (const ll of f.latlngs) add(L.circleMarker(ll, { radius: 8, ...styleFor(f.id) }));
      } else if (f.kind === 'line') {
        add(L.polyline(f.latlngs, styleFor(f.id)));
      } else {
        add(L.polygon(f.latlngs, styleFor(f.id)));
      }
      layersById.set(f.id, layers);
    }
    const bounds = featureBounds(features);
    if (bounds) map.fitBounds(bounds, { padding: [30, 30], maxZoom: 17 });
  }

  function restyle() {
    for (const [id, layers] of layersById) {
      for (const layer of layers) layer.setStyle(styleFor(id));
    }
  }

  onMount(() => {
    map = L.map(mapEl, { scrollWheelZoom: true, attributionControl: true });
    L.tileLayer('https://tile.openstreetmap.org/{z}/{x}/{y}.png', {
      maxZoom: 19,
      attribution: '&copy; OpenStreetMap',
    }).addTo(map);
    rebuildLayers();
  });

  onDestroy(() => {
    if (map) map.remove();
    map = null;
  });

  $: if (map && elements) rebuildLayers();
  $: if (map && selected !== undefined) restyle();
</script>

<div bind:this={mapEl} class="viewer-map" style:height role="application" aria-label="map"></div>

<style>
  .viewer-map {
    width: 100%;
    border-radius: 8px;
    overflow: hidden;
    min-height: 240px;
    background: var(--surface-2, #f1f3f5);
  }
</style>
