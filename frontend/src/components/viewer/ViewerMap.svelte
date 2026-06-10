<script>
  // Explorable 3D map for the dataset viewer (MapLibre GL). Located elements
  // render as vector features (dots / lines / polygons); elements with a 3D
  // model additionally get the *actual model* standing on the map — loaded with
  // three.js into a custom WebGL layer, georeferenced at its anchor and scaled
  // to real metres, so a 96 m clock tower is 96 m tall next to the OSM building
  // extrusions around it. CityJSON/CityGML models carry their own georeference
  // and place themselves. The basemap follows the app theme — a colourful
  // hosted light style and a custom midnight-with-colours dark style — plus an
  // Esri satellite toggle. Right-drag (or Ctrl-drag) tilts and rotates.
  import { onMount, onDestroy, createEventDispatcher } from 'svelte';
  import maplibregl from 'maplibre-gl';
  import 'maplibre-gl/dist/maplibre-gl.css';
  import * as THREE from 'three';
  import { t as i18nT } from 'svelte-i18n';
  import { Map as MapIcon, Satellite } from 'lucide-svelte';
  import { isDark } from '../../lib/theme.js';
  import { elementsToGeoJSON, featureBounds, toMapFeature, modelAnchor } from '../../lib/viewer/geometry';
  import { modelRefOf } from '../../lib/viewer/detect';
  import { loadModel, realWorldMeters, defaultMaterial, NORMALISED_DIM } from '../../lib/viewer/models';
  import { styleFor, add3dBuildings } from '../../lib/viewer/basemaps';

  /** @type {import('../../lib/viewer/geometry').ViewerElement[]} */
  export let elements = [];
  export let selected = '';
  export let height = '100%';
  /** Fallback footprint (m) for models with untrustworthy units (most STLs). */
  const FALLBACK_FOOTPRINT_M = 90;

  const dispatch = createEventDispatcher();

  const SELECT_COLOR = '#e8590c';

  /** Per-frame scratch matrix (avoids a Matrix4 allocation every render). */
  const SCRATCH_PROJ = new THREE.Matrix4();
  // Scratch objects for raycastModels (runs on every mousemove). Reused across
  // entries — only primitives (id, NDC depth) survive a loop iteration.
  const RAY_PROJ = new THREE.Matrix4();
  const RAY_FWD = new THREE.Matrix4();
  const RAY_INV = new THREE.Matrix4();
  const RAY_A = new THREE.Vector3();
  const RAY_B = new THREE.Vector3();
  const RAY_HIT = new THREE.Vector3();
  const RAY = new THREE.Ray();

  let mapEl;
  let map = null;
  let dark = false;
  let basemap = 'streets'; // 'streets' | 'satellite'
  /** id → { anchor, anchorUsed, scene, modelGroup, box, mercMatrix, meters } */
  let entries = new Map();
  let renderer = null;
  const camera = new THREE.Camera();
  let lastProj = null; // latest map projection matrix (for raycasting)
  let fitted = false;
  let hoverPopup = null;
  // True between a style's 'style.load' and the next setStyle(): addSource /
  // addLayer are safe. (isStyleLoaded() is the wrong guard — it also waits for
  // tiles, so it is still false while 'style.load' fires.)
  let styleReady = false;

  const unsubTheme = isDark.subscribe((v) => {
    if (v === dark) return;
    dark = v;
    if (map) applyStyle();
  });

  // ── three.js custom layer ──────────────────────────────────────────────────
  // One scene per model, rendered with the camera projection set to
  // (map matrix × model-to-mercator matrix). Keeping vertices in local metres
  // and folding the mercator offset into a CPU-side double-precision matrix
  // avoids the float32 jitter that placing geometry at raw mercator
  // coordinates causes at street-level zooms.
  const modelLayer = {
    id: 'ots-3d-models',
    type: 'custom',
    renderingMode: '3d',
    onAdd(m, gl) {
      renderer = new THREE.WebGLRenderer({ canvas: m.getCanvas(), context: gl, antialias: true });
      renderer.autoClear = false;
    },
    render(gl, args) {
      // MapLibre v5 passes {defaultProjectionData}; v4 passed the raw matrix.
      const arr = args?.defaultProjectionData?.mainMatrix ?? args;
      lastProj = arr;
      if (!entries.size || !renderer) return;
      const proj = SCRATCH_PROJ.fromArray(arr); // reused — this runs every frame
      renderer.resetState();
      for (const e of entries.values()) {
        if (!e.scene || !e.mercMatrix) continue;
        camera.projectionMatrix.copy(proj).multiply(e.mercMatrix);
        renderer.render(e.scene, camera);
      }
    },
    onRemove() {
      renderer?.dispose();
      renderer = null;
    },
  };

  /** Model-local (y-up metres, normalised) → mercator placement matrix. */
  function mercMatrixFor(lonLat, meters) {
    const merc = maplibregl.MercatorCoordinate.fromLngLat({ lng: lonLat[0], lat: lonLat[1] }, 0);
    const s = merc.meterInMercatorCoordinateUnits() * (meters / NORMALISED_DIM);
    return new THREE.Matrix4()
      .makeTranslation(merc.x, merc.y, merc.z)
      .multiply(new THREE.Matrix4().makeScale(s, -s, s))
      .multiply(new THREE.Matrix4().makeRotationX(Math.PI / 2));
  }

  let shadowTexture = null;
  /** Soft radial "contact shadow" under a model — a cheap grounding cue. */
  function makeShadowDisc(radius) {
    if (!shadowTexture) {
      const c = document.createElement('canvas');
      c.width = c.height = 128;
      const ctx = c.getContext('2d');
      const g = ctx.createRadialGradient(64, 64, 8, 64, 64, 64);
      g.addColorStop(0, 'rgba(0,0,0,0.42)');
      g.addColorStop(1, 'rgba(0,0,0,0)');
      ctx.fillStyle = g;
      ctx.fillRect(0, 0, 128, 128);
      shadowTexture = new THREE.CanvasTexture(c);
    }
    const mesh = new THREE.Mesh(
      new THREE.PlaneGeometry(radius * 2, radius * 2),
      new THREE.MeshBasicMaterial({ map: shadowTexture, transparent: true, depthWrite: false })
    );
    mesh.rotation.x = -Math.PI / 2;
    mesh.position.y = 0.02;
    return mesh;
  }

  function buildEntryScene(holder) {
    const scene = new THREE.Scene();
    const hemi = new THREE.HemisphereLight(0xffffff, 0x46506b, dark ? 1.15 : 1.5);
    const sun = new THREE.DirectionalLight(0xffffff, dark ? 1.5 : 2.1);
    sun.position.set(0.6, 1, 0.8);
    scene.add(hemi, sun, holder);
    return scene;
  }

  async function attachModel(entry, el) {
    const ref = modelRefOf(el);
    if (!ref) return;
    try {
      const cached = await loadModel(ref.url, ref.format);
      if (entries.get(el.id) !== entry) return; // rebuilt meanwhile
      const model = cached.clone(true);
      // Clone materials so per-entry theming/highlighting never mutates the cache.
      model.traverse((n) => {
        if (n.isMesh && n.material) {
          n.material = Array.isArray(n.material) ? n.material.map((m) => m.clone()) : n.material.clone();
          if (ref.format === 'stl') n.userData.stl = true;
        }
      });
      // CityJSON/CityGML carry their own georeference — trust it over the WKT dot.
      const anchor = cached.userData.geo?.anchorLonLat ?? entry.anchor;
      if (!anchor) return;
      const meters = realWorldMeters(cached, FALLBACK_FOOTPRINT_M);
      const box = new THREE.Box3().setFromObject(model);
      const radius = Math.max(box.max.x - box.min.x, box.max.z - box.min.z) * 0.62;
      const holder = new THREE.Group();
      holder.add(model, makeShadowDisc(radius));
      entry.modelGroup = model;
      entry.box = box;
      entry.scene = buildEntryScene(holder);
      entry.meters = meters;
      entry.anchorUsed = anchor;
      entry.mercMatrix = mercMatrixFor(anchor, meters);
      themeMaterials();
      highlightModels();
      map?.triggerRepaint();
    } catch {
      /* model failed to load — the vector dot remains */
    }
  }

  /** Re-skin theme-dependent materials (STL default material). */
  function themeMaterials() {
    for (const e of entries.values()) {
      e.modelGroup?.traverse((n) => {
        if (n.isMesh && n.userData.stl) n.material = defaultMaterial(dark);
      });
    }
    map?.triggerRepaint();
  }

  function highlightModels() {
    for (const [id, e] of entries) {
      e.modelGroup?.traverse((n) => {
        if (n.isMesh && n.material && 'emissive' in n.material) {
          n.material.emissive.setHex(id === selected ? 0xe8590c : 0x000000);
          n.material.emissiveIntensity = id === selected ? 0.45 : 0;
        }
      });
    }
    map?.triggerRepaint();
  }

  /** Screen point → model id, by casting a ray against each model's box. */
  function raycastModels(point) {
    if (!lastProj || !map) return null;
    const canvas = map.getCanvas();
    const w = canvas.clientWidth || 1;
    const h = canvas.clientHeight || 1;
    const nx = (point.x / w) * 2 - 1;
    const ny = -(point.y / h) * 2 + 1;
    const proj = RAY_PROJ.fromArray(lastProj);
    let best = null;
    for (const [id, e] of entries) {
      if (!e.scene || !e.mercMatrix || !e.box) continue;
      const fwd = RAY_FWD.multiplyMatrices(proj, e.mercMatrix); // local → NDC
      if (Math.abs(fwd.determinant()) < 1e-20) continue;
      const inv = RAY_INV.copy(fwd).invert();
      // Unproject two NDC depths → a ray in model-local space.
      const a = RAY_A.set(nx, ny, -0.99).applyMatrix4(inv);
      const dir = RAY_B.set(nx, ny, 0.999).applyMatrix4(inv).sub(a);
      if (!dir.lengthSq()) continue;
      RAY.origin.copy(a);
      RAY.direction.copy(dir).normalize();
      if (RAY.intersectBox(e.box, RAY_HIT)) {
        // Model-local distances are not comparable across entries (each local
        // space has its own metres scale); compare NDC depth instead.
        const d = RAY_HIT.applyMatrix4(fwd).z;
        if (!best || d < best.d) best = { id, d };
      }
    }
    return best?.id ?? null;
  }

  // ── Vector overlays (re-added after every style swap) ──────────────────────
  const lineBase = () => (dark ? '#7ec3e8' : '#2e6da4');
  const dotPlain = () => (dark ? '#9fb6c9' : '#5a7a96');
  const dotModel = () => (dark ? '#67b5e8' : '#2f86c9');

  const caseSelected = (value, fallback) => ['case', ['==', ['get', 'id'], selected || ''], value, fallback];

  function ensureSource(id, features) {
    const data = { type: 'FeatureCollection', features };
    const src = map.getSource(id);
    if (src) src.setData(data);
    else map.addSource(id, { type: 'geojson', data });
  }

  function ensureOverlays() {
    if (!map || !styleReady) return;
    const gj = elementsToGeoJSON(elements);
    ensureSource('ots-points', gj.points);
    ensureSource('ots-lines', gj.lines);
    ensureSource('ots-fills', gj.polygons);

    if (!map.getLayer('ots-fill')) {
      map.addLayer({ id: 'ots-fill', type: 'fill', source: 'ots-fills',
        paint: { 'fill-color': lineBase(), 'fill-opacity': 0.18 } });
      map.addLayer({ id: 'ots-fill-line', type: 'line', source: 'ots-fills',
        paint: { 'line-color': lineBase(), 'line-width': 2 } });
      map.addLayer({ id: 'ots-line', type: 'line', source: 'ots-lines',
        layout: { 'line-cap': 'round', 'line-join': 'round' },
        paint: { 'line-color': lineBase(), 'line-width': 3, 'line-opacity': 0.9 } });
      map.addLayer({ id: 'ots-point', type: 'circle', source: 'ots-points',
        paint: {
          'circle-radius': ['interpolate', ['linear'], ['zoom'], 4, 3.5, 14, 7],
          'circle-color': dotPlain(),
          'circle-stroke-width': 1.4,
          'circle-stroke-color': dark ? '#0b1118' : '#ffffff',
          'circle-opacity': 0.9,
          'circle-stroke-opacity': 0.9,
        } });
      map.addLayer({ id: 'ots-label', type: 'symbol', source: 'ots-points', minzoom: 12.5,
        layout: {
          'text-field': ['get', 'label'],
          'text-font': ['Noto Sans Regular'],
          'text-size': 12,
          'text-offset': [0, 1.2],
          'text-anchor': 'top',
          'text-optional': true,
        },
        paint: {
          'text-color': dark ? '#dbe6f2' : '#1c2a3a',
          'text-halo-color': dark ? '#0b1118' : '#ffffff',
          'text-halo-width': 1.3,
        } });
    }
    add3dBuildings(map, dark);
    if (!map.getLayer('ots-3d-models')) map.addLayer(modelLayer);
    applySelectedPaint();
  }

  function applySelectedPaint() {
    if (!map?.getLayer('ots-point')) return;
    // The dot of a modelled element fades out as its real model becomes
    // visible. ['zoom'] is only legal as input to a TOP-LEVEL interpolate, so
    // the hasModel branch lives in the interpolate's outputs, not around it.
    const fadeAt = (atZoom) => ['case', ['get', 'hasModel'], atZoom, 0.9];
    const fade = ['interpolate', ['linear'], ['zoom'], 12.5, fadeAt(0.9), 15.5, fadeAt(0)];
    map.setPaintProperty('ots-point', 'circle-color',
      caseSelected(SELECT_COLOR, ['case', ['get', 'hasModel'], dotModel(), dotPlain()]));
    map.setPaintProperty('ots-point', 'circle-opacity', fade);
    map.setPaintProperty('ots-point', 'circle-stroke-opacity', fade);
    map.setPaintProperty('ots-line', 'line-color', caseSelected(SELECT_COLOR, lineBase()));
    map.setPaintProperty('ots-line', 'line-width', caseSelected(5, 3));
    map.setPaintProperty('ots-fill-line', 'line-color', caseSelected(SELECT_COLOR, lineBase()));
    map.setPaintProperty('ots-fill', 'fill-color', caseSelected(SELECT_COLOR, lineBase()));
  }

  // ── Data → entries + camera framing ─────────────────────────────────────────
  // Svelte invalidates object props on every parent render (safe_not_equal),
  // so guard by identity — otherwise any parent state change (opening/closing
  // the modal) re-runs the rebuild and re-kicks model loads.
  let lastElements = null;
  function rebuildData() {
    if (!map || elements === lastElements) return;
    lastElements = elements;
    entries = new Map();
    for (const el of elements) {
      const anchor = modelAnchor(el);
      const entry = { anchor, anchorUsed: null, scene: null, modelGroup: null, box: null, mercMatrix: null };
      entries.set(el.id, entry);
      if (anchor || modelRefOf(el)) attachModel(entry, el);
    }
    ensureOverlays();
    if (!fitted) {
      const features = elements.map(toMapFeature).filter(Boolean);
      const b = featureBounds(features);
      if (b) {
        fitted = true;
        map.fitBounds([[b[0][1], b[0][0]], [b[1][1], b[1][0]]], { padding: 70, maxZoom: 16.2, duration: 0 });
        // Cinematic tilt-in when there is something 3D to look at.
        if (elements.some((el) => modelRefOf(el))) map.easeTo({ pitch: 52, duration: 1100 });
      }
    }
  }

  /** Pan/fly to an element (used when selecting from the list). */
  export function focusElement(id) {
    if (!map) return;
    const el = elements.find((e) => e.id === id);
    if (!el) return;
    const entry = entries.get(id);
    const f = toMapFeature(el);
    if (f && f.kind !== 'point') {
      const b = featureBounds([f]);
      if (b) map.fitBounds([[b[0][1], b[0][0]], [b[1][1], b[1][0]]], { padding: 90, duration: 800 });
      return;
    }
    const anchor = entry?.anchorUsed ?? modelAnchor(el);
    if (anchor) {
      map.flyTo({
        center: anchor,
        zoom: Math.max(map.getZoom(), 16.4),
        pitch: entry?.scene ? 55 : map.getPitch(),
        duration: 900,
      });
    }
  }

  // ── Interaction ─────────────────────────────────────────────────────────────
  const HIT_LAYERS = ['ots-point', 'ots-line', 'ots-fill'];
  const hitLayers = () => HIT_LAYERS.filter((l) => map.getLayer(l));

  function onClick(e) {
    const modelId = raycastModels(e.point);
    if (modelId) {
      dispatch('select', { id: modelId });
      return;
    }
    const pad = 6;
    const box = [[e.point.x - pad, e.point.y - pad], [e.point.x + pad, e.point.y + pad]];
    const fs = map.queryRenderedFeatures(box, { layers: hitLayers() });
    if (fs.length) dispatch('select', { id: fs[0].properties.id });
  }

  function onMouseMove(e) {
    if (!map) return;
    let label = null;
    const modelId = raycastModels(e.point);
    if (modelId) {
      const el = elements.find((x) => x.id === modelId);
      label = el?.label || modelId.split(/[/#]/).pop();
    } else {
      const pad = 4;
      const box = [[e.point.x - pad, e.point.y - pad], [e.point.x + pad, e.point.y + pad]];
      const fs = map.queryRenderedFeatures(box, { layers: hitLayers() });
      if (fs.length) label = fs[0].properties.label;
    }
    map.getCanvas().style.cursor = label ? 'pointer' : '';
    if (label) {
      hoverPopup ??= new maplibregl.Popup({
        closeButton: false, closeOnClick: false, offset: 12, className: 'viewer-popup',
      });
      hoverPopup.setLngLat(e.lngLat).setText(label).addTo(map);
    } else {
      hoverPopup?.remove();
    }
  }

  function applyStyle() {
    if (!map) return;
    hoverPopup?.remove();
    styleReady = false;
    map.setStyle(styleFor(basemap, dark), { diff: false });
    // Overlays, buildings and the model layer re-attach on 'style.load'.
  }

  function setBasemap(kind) {
    if (kind === basemap) return;
    basemap = kind;
    applyStyle();
  }

  onMount(() => {
    map = new maplibregl.Map({
      container: mapEl,
      style: styleFor(basemap, dark),
      attributionControl: { compact: true },
      maxPitch: 70,
    });
    map.addControl(new maplibregl.NavigationControl({ visualizePitch: true }), 'top-right');
    map.addControl(new maplibregl.ScaleControl({ maxWidth: 110 }), 'bottom-left');
    map.on('style.load', () => {
      styleReady = true;
      ensureOverlays();
      themeMaterials();
    });
    map.on('click', onClick);
    map.on('mousemove', onMouseMove);
    map.on('mouseout', () => {
      hoverPopup?.remove();
      hoverPopup = null;
      map.getCanvas().style.cursor = '';
    });
    if (import.meta.env.DEV) window.__otsViewerMap = map; // dev console handle
    rebuildData();
  });

  onDestroy(() => {
    unsubTheme();
    hoverPopup?.remove();
    if (map) map.remove();
    map = null;
    entries = new Map();
  });

  $: if (map && elements) rebuildData();
  $: if (map && selected !== undefined) {
    applySelectedPaint();
    highlightModels();
  }
</script>

<div class="viewer-map-wrap" style:height>
  <div bind:this={mapEl} class="viewer-map" role="application" aria-label="map"></div>
  <div class="basemap-toggle" role="group" aria-label={$i18nT('viewer.basemap')}>
    <button
      class:active={basemap === 'streets'}
      title={$i18nT('viewer.basemapStreets')}
      aria-label={$i18nT('viewer.basemapStreets')}
      on:click={() => setBasemap('streets')}
    ><MapIcon size={14} /></button>
    <button
      class:active={basemap === 'satellite'}
      title={$i18nT('viewer.basemapSatellite')}
      aria-label={$i18nT('viewer.basemapSatellite')}
      on:click={() => setBasemap('satellite')}
    ><Satellite size={14} /></button>
  </div>
</div>

<style>
  .viewer-map-wrap {
    position: relative;
    width: 100%;
    min-height: 240px;
  }
  .viewer-map {
    position: absolute;
    inset: 0;
    background: var(--bg-soft, #f1f5f9);
  }
  .basemap-toggle {
    position: absolute;
    top: 10px;
    left: 10px;
    display: flex;
    border-radius: 8px;
    overflow: hidden;
    box-shadow: 0 1px 4px rgba(0, 0, 0, 0.25);
    z-index: 5;
  }
  .basemap-toggle button {
    border: 0;
    padding: 7px 9px;
    background: var(--bg-elevated, #fff);
    color: var(--muted, #64748b);
    cursor: pointer;
    display: flex;
    align-items: center;
  }
  .basemap-toggle button + button {
    border-left: 1px solid var(--line-soft, #e6eaef);
  }
  .basemap-toggle button:hover {
    color: var(--ink-900, #0f172a);
  }
  .basemap-toggle button.active {
    background: var(--bg-accent-soft, #e7f0fb);
    color: var(--brand-600, #2563a8);
  }
  :global(.viewer-popup .maplibregl-popup-content) {
    padding: 4px 9px;
    font-size: 0.78rem;
    border-radius: 7px;
    background: var(--bg-elevated, #fff);
    color: var(--ink-900, #0f172a);
    box-shadow: 0 2px 8px rgba(0, 0, 0, 0.3);
  }
  :global(.viewer-popup .maplibregl-popup-tip) {
    border-top-color: var(--bg-elevated, #fff);
  }
</style>
