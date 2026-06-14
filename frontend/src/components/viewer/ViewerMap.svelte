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
  import { styleFor, add3dBuildings, BUILDINGS_LAYER_ID } from '../../lib/viewer/basemaps';

  /** @type {import('../../lib/viewer/geometry').ViewerElement[]} */
  export let elements = [];
  export let selected = '';
  export let height = '100%';
  /** Extra map attribution line (e.g. the 3DBAG CC-BY credit). */
  export let extraAttribution = '';
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
  // Precise sub-element picking. A model whose meshes carry IFC GlobalIds (an
  // IFC building standing on its Site anchor) is one entry but many elements;
  // a triangle raycast resolves the exact wall/slab/door under the cursor.
  // Click-only — a per-frame triangle cast over a whole building would jank.
  const RAYCASTER = new THREE.Raycaster();

  let mapEl;
  let map = null;
  let dark = false;
  let basemap = 'streets'; // 'streets' | 'satellite'
  // Layer visibility (doubles as the legend). 3D models are a custom WebGL layer,
  // so they're toggled via each model group's `.visible` rather than a layout prop.
  let layersOn = { points: true, lines: true, areas: true, models: true, labels: true, osm3d: true };
  const LAYER_DEFS = [
    { key: 'points', shape: 'dot', color: '#2f88d8', label: 'viewer.layerPoints' },
    { key: 'lines', shape: 'line', color: '#2f88d8', label: 'viewer.layerLines' },
    { key: 'areas', shape: 'area', color: '#6d5ba8', label: 'viewer.layerAreas' },
    { key: 'models', shape: 'box', color: '#e8590c', label: 'viewer.layerModels' },
    { key: 'osm3d', shape: 'box', color: '#64748b', label: 'viewer.layerOsmBuildings' },
    { key: 'labels', shape: 'text', color: '#64748b', label: 'viewer.layerLabels' },
  ];
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
  // A fresh custom-layer object per add: MapLibre does NOT reliably re-accept the
  // *same* custom-layer instance after setStyle({diff:false}) (the basemap/theme
  // swap), so re-adding the one const object silently no-ops — which is why the
  // 3D models vanished when switching to satellite. A new object each time (its
  // methods still close over the component-scoped renderer/entries) re-adds
  // cleanly on every style, raster included.
  const makeModelLayer = () => ({
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
  });

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
      const cached = await loadModel(ref.url, ref.format, { upAxis: ref.upAxis });
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

  /** GlobalId of an IFC mesh (or its nearest ancestor that owns one). */
  function meshGuid(obj) {
    for (let n = obj; n; n = n.parent) {
      if (n.userData && n.userData.ifcGuid) return n.userData.ifcGuid;
    }
    return null;
  }

  function setEmissive(mat, on) {
    if (!mat || !('emissive' in mat)) return;
    mat.emissive.setHex(on ? 0xe8590c : 0x000000);
    mat.emissiveIntensity = on ? 0.5 : 0;
  }

  function highlightModels() {
    // When the selected element is one element *inside* a multi-element model
    // (an IFC building), light only its meshes — matched by GlobalId. Otherwise
    // light the whole model when its own id is selected.
    const selGuid = elements.find((x) => x.id === selected)?.ifc_guid || null;
    for (const [id, e] of entries) {
      e.modelGroup?.traverse((n) => {
        if (!n.isMesh || !n.material) return;
        const on = selGuid ? n.userData.ifcGuid === selGuid : id === selected;
        if (Array.isArray(n.material)) n.material.forEach((m) => setEmissive(m, on));
        else setEmissive(n.material, on);
      });
    }
    map?.triggerRepaint();
  }

  /**
   * Screen point → `{ id, guid }` by casting a ray against each model. A cheap
   * box test rejects misses and orders entries; when `precise` (a click), a
   * triangle raycast against the actual meshes resolves the exact IFC
   * sub-element (GlobalId) under the cursor. `guid` is null for single-element
   * models or when the ray grazes the box but misses every triangle.
   */
  function raycastModels(point, precise = false) {
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
      if (e.modelGroup && !e.modelGroup.visible) continue; // respect the layer toggle
      const fwd = RAY_FWD.multiplyMatrices(proj, e.mercMatrix); // local → NDC
      if (Math.abs(fwd.determinant()) < 1e-20) continue;
      const inv = RAY_INV.copy(fwd).invert();
      // Unproject two NDC depths → a ray in model-local space.
      const a = RAY_A.set(nx, ny, -0.99).applyMatrix4(inv);
      const dir = RAY_B.set(nx, ny, 0.999).applyMatrix4(inv).sub(a);
      if (!dir.lengthSq()) continue;
      RAY.origin.copy(a);
      RAY.direction.copy(dir).normalize();
      if (!RAY.intersectBox(e.box, RAY_HIT)) continue;
      // Model-local distances are not comparable across entries (each local
      // space has its own metres scale); compare NDC depth instead.
      const d = RAY_HIT.applyMatrix4(fwd).z;
      let guid = null;
      if (precise && e.modelGroup) {
        e.scene.updateMatrixWorld(); // refresh world matrices if a frame hasn't since load
        RAYCASTER.ray.origin.copy(RAY.origin);
        RAYCASTER.ray.direction.copy(RAY.direction);
        const hits = RAYCASTER.intersectObject(e.modelGroup, true);
        if (hits.length) guid = meshGuid(hits[0].object);
      }
      if (!best || d < best.d) best = { id, guid, d };
    }
    return best ? { id: best.id, guid: best.guid } : null;
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
    if (!map.getLayer('ots-3d-models')) map.addLayer(makeModelLayer());
    applySelectedPaint();
    applyLayerVisibility();
    suppressBasemapBuildingsUnderModels();
  }

  // ── Basemap-building suppression ────────────────────────────────────────────
  // Our georeferenced models stand where the OSM extrusion layer also raises a
  // generic grey block — the two overlap and z-fight (Big Ben inside a slab).
  // Fix: query the extrusion features under each model anchor and filter those
  // footprints out of the layer. Feature ids vary per tile/zoom, so re-resolve
  // whenever the map goes idle after movement.
  let suppressedIds = new Set();
  function suppressBasemapBuildingsUnderModels() {
    if (!map || !map.getLayer(BUILDINGS_LAYER_ID)) return;
    let changed = false;
    for (const e of entries.values()) {
      const anchor = e.anchorUsed ?? e.anchor;
      if (!anchor || !e.modelGroup) continue;
      let feats = [];
      try {
        const c = map.project({ lng: anchor[0], lat: anchor[1] });
        // Query the model's whole FOOTPRINT, not just the anchor pixel — a tall
        // model can poke through neighbouring extrusion footprints too. Radius =
        // half the model's real size, converted to screen pixels at this zoom.
        const mpp =
          (156543.03392 * Math.cos((anchor[1] * Math.PI) / 180)) / Math.pow(2, map.getZoom());
        const r = Math.min(140, Math.max(4, (e.meters || 50) / 2 / Math.max(mpp, 1e-6)));
        feats = map.queryRenderedFeatures(
          [
            [c.x - r, c.y - r],
            [c.x + r, c.y + r],
          ],
          { layers: [BUILDINGS_LAYER_ID] }
        );
      } catch {
        continue;
      }
      for (const f of feats) {
        if (f.id !== undefined && !suppressedIds.has(f.id)) {
          suppressedIds.add(f.id);
          changed = true;
        }
      }
    }
    if (changed) {
      map.setFilter(BUILDINGS_LAYER_ID, ['!', ['in', ['id'], ['literal', [...suppressedIds]]]]);
    }
  }

  function setLayerVis(id, on) {
    if (map?.getLayer(id)) map.setLayoutProperty(id, 'visibility', on ? 'visible' : 'none');
  }
  // Apply the layer toggles. Re-run after (re)adding layers so a basemap switch
  // preserves the user's choices.
  function applyLayerVisibility() {
    if (!map) return;
    setLayerVis('ots-point', layersOn.points);
    setLayerVis('ots-line', layersOn.lines);
    setLayerVis('ots-fill', layersOn.areas);
    setLayerVis('ots-fill-line', layersOn.areas);
    setLayerVis('ots-label', layersOn.labels);
    setLayerVis(BUILDINGS_LAYER_ID, layersOn.osm3d);
    for (const e of entries.values()) {
      if (e.modelGroup) e.modelGroup.visible = layersOn.models;
    }
    map.triggerRepaint?.();
  }
  function toggleLayer(key) {
    layersOn = { ...layersOn, [key]: !layersOn[key] };
    applyLayerVisibility();
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
      // Load a model only when it can actually be placed: it has an anchor, or
      // it self-georeferences (CityJSON/CityGML). Anchorless IFC *element* refs
      // (a `#GlobalId` fragment with no WKT) would just parse the whole building
      // and bail — the anchored Site already stands it on the map.
      const ref = modelRefOf(el);
      const selfPlaces = ref && (ref.format === 'cityjson' || ref.format === 'citygml');
      if (anchor || selfPlaces) attachModel(entry, el);
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
    let el = elements.find((e) => e.id === id);
    if (!el) return;
    // An element with no geometry of its own (an IFC sub-element — a wall, a
    // door) flies to its nearest located ancestor (the building's Site anchor).
    // Its own mesh still lights up, driven by `selected`.
    const seen = new Set();
    while (el && !el.wkt4326 && !entries.get(el.id)?.anchorUsed && el.parent && !seen.has(el.id)) {
      seen.add(el.id);
      el = elements.find((e) => e.id === el.parent) || null;
    }
    if (!el) return;
    const entry = entries.get(el.id);
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
    const pick = raycastModels(e.point, true);
    if (pick) {
      // guid → DatasetViewer resolves it to the specific IFC sub-element and
      // opens that element's window; id is the whole-model fallback.
      dispatch('select', { id: pick.id, guid: pick.guid });
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
    const pick = raycastModels(e.point); // box-level only — keep hover cheap
    if (pick) {
      const el = elements.find((x) => x.id === pick.id);
      label = el?.label || pick.id.split(/[/#]/).pop();
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
      attributionControl: extraAttribution
        ? { compact: true, customAttribution: extraAttribution }
        : { compact: true },
      maxPitch: 70,
    });
    map.addControl(new maplibregl.NavigationControl({ visualizePitch: true }), 'top-right');
    map.addControl(new maplibregl.ScaleControl({ maxWidth: 110 }), 'bottom-left');
    map.on('style.load', () => {
      styleReady = true;
      suppressedIds = new Set(); // feature ids are style/source-specific
      ensureOverlays();
      themeMaterials();
    });
    map.on('idle', suppressBasemapBuildingsUnderModels);
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

  <!-- Layers + legend: toggle each feature kind on/off; the swatch is the legend. -->
  <div class="map-layers" role="group" aria-label={$i18nT('viewer.layers')}>
    <div class="ml-title">{$i18nT('viewer.layers')}</div>
    {#each LAYER_DEFS as L}
      <label class="ml-row" class:off={!layersOn[L.key]}>
        <input type="checkbox" checked={layersOn[L.key]} on:change={() => toggleLayer(L.key)} />
        <span class="ml-swatch ml-{L.shape}" style:--sw={L.color}></span>
        <span class="ml-name">{$i18nT(L.label)}</span>
      </label>
    {/each}
    <div class="ml-legend-sel">
      <span class="ml-swatch ml-dot" style:--sw={SELECT_COLOR}></span>
      <span class="ml-name">{$i18nT('viewer.layerSelected')}</span>
    </div>
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

  /* Layers + legend control (top-right) */
  .map-layers {
    position: absolute;
    top: 10px;
    right: 10px;
    z-index: 5;
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 8px 10px;
    border-radius: 10px;
    background: var(--bg-elevated, rgba(255, 255, 255, 0.95));
    border: 1px solid var(--line-soft, #e6eaef);
    box-shadow: 0 2px 10px rgba(0, 0, 0, 0.18);
    backdrop-filter: blur(8px);
    font-size: 0.76rem;
    color: var(--ink-900, #0f172a);
  }
  .ml-title {
    font-size: 0.62rem;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--muted, #64748b);
    font-weight: 700;
    margin-bottom: 2px;
  }
  .ml-row {
    display: flex;
    align-items: center;
    gap: 6px;
    cursor: pointer;
    padding: 1px 0;
  }
  .ml-row.off .ml-name {
    opacity: 0.45;
    text-decoration: line-through;
  }
  .ml-row input {
    margin: 0;
    cursor: pointer;
    accent-color: var(--brand-500, #2f88d8);
  }
  .ml-swatch {
    width: 14px;
    height: 14px;
    flex: none;
    display: inline-block;
  }
  .ml-dot {
    border-radius: 50%;
    background: var(--sw);
    border: 1.5px solid #fff;
    box-shadow: 0 0 0 1px rgba(0, 0, 0, 0.15);
  }
  .ml-line {
    height: 3px;
    border-radius: 2px;
    background: var(--sw);
  }
  .ml-area {
    border-radius: 3px;
    background: color-mix(in srgb, var(--sw) 22%, transparent);
    border: 1.5px solid var(--sw);
  }
  .ml-box {
    border-radius: 2px;
    background: color-mix(in srgb, var(--sw) 30%, transparent);
    border: 1.5px solid var(--sw);
  }
  .ml-text {
    background: linear-gradient(var(--sw), var(--sw)) left 60% / 100% 2px no-repeat;
    font: 700 11px/14px serif;
    color: var(--sw);
    text-align: center;
  }
  .ml-text::before {
    content: 'A';
  }
  .ml-legend-sel {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-top: 4px;
    padding-top: 4px;
    border-top: 1px solid var(--line-soft, #e6eaef);
    color: var(--muted, #64748b);
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
