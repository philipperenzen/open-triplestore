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
  import { Map as MapIcon, Satellite, Crosshair } from 'lucide-svelte';
  import { isDark } from '../../lib/theme.js';
  import { elementsToGeoJSON, featureBounds, toMapFeature, modelAnchor } from '../../lib/viewer/geometry';
  import { modelRefOf, modelRefsOf, cityBaseUrl, cityObjectFragment } from '../../lib/viewer/detect';
  import { loadModel, realWorldMeters, defaultMaterial, NORMALISED_DIM } from '../../lib/viewer/models';
  import { ifcGuidAt, groupHasGuid, ifcProgress } from '../../lib/viewer/ifc';
  import { cityObjectIdAt, groupHasCityObject } from '../../lib/viewer/cityjson';
  import { applyStudioLook, studioEnvironment } from '../../lib/viewer/studio';
  import {
    styleFor,
    add3dBuildings,
    buildingLayerIds,
    OSM_BUILDING_SOURCE_LAYER,
  } from '../../lib/viewer/basemaps';
  import {
    buildingIdFilter,
    buildingSuppressionFilter,
    combineFilters,
    footprintPolygon,
    footprintsFromLocalPoints,
    footprintsToMultiPolygon,
    polygonsIntersect,
  } from '../../lib/viewer/footprint';
  import {
    HL_COLOR,
    HL_EMISSIVE,
    buildHighlightOverlay,
    buildCityHighlightOverlay,
    disposeHighlightOverlay,
  } from '../../lib/viewer/highlight';

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
  /** Initial basemap ('streets' | 'satellite') — the toggle still switches live. */
  export let basemap = 'streets';
  // Layer visibility (doubles as the legend). 3D models are a custom WebGL layer,
  // so they're toggled via each model group's `.visible` rather than a layout prop.
  let layersOn = { points: true, lines: true, areas: true, models: true, labels: true, osm3d: true };
  // Opt-in x-ray: ghost the surrounding building and draw the selected element
  // THROUGH it. Off by default — an automatic x-ray on every pick made the model
  // dissolve around the selection and let the ground contact-shadow show through
  // as a dark ring, which is what "a x-ray circle … through the 3d model" was.
  let xrayOn = false;
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
  let envFailed = false; // PMREM failed on this context — don't retry per frame
  const camera = new THREE.Camera();
  let lastProj = null; // latest map projection matrix (for raycasting)
  let fitted = false;
  let hoverPopup = null;
  // Persistent popup for a picked CityObject that has NO backing RDF element
  // (e.g. a 3DBAG house — geometry only). Buildings that DO map to an element
  // open the rich ElementModal instead. `citySel` is the locally-picked CityObject
  // to x-ray when it has no element; a selection that maps to an element drives
  // the highlight through `selected` (see reconcileCitySel).
  let cityPopup = null;
  let citySel = null; // { entryId, objId } | null
  // Client-side model-load progress (web-ifc parse etc.) for the loading chip.
  let modelsLoading = 0;
  let modelsFailed = 0;
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
      // Filmic tone mapping for the standing models (same studio look as the
      // modal viewer / walkthrough) — only affects our own draw calls.
      applyStudioLook(renderer);
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
        // Entry scenes are built as soon as their model loads, which can be
        // before this layer exists — attach the per-renderer environment on
        // first sight (a WeakMap-cached texture; a no-op after the first frame).
        // Guarded: a PMREM failure on the shared MapLibre context must degrade
        // to lights-only shading (once), never break model rendering.
        if (!e.scene.environment && !envFailed) {
          try {
            e.scene.environment = studioEnvironment(renderer);
          } catch {
            envFailed = true;
          }
        }
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
  /**
   * Soft radial "contact shadow" under a model — a cheap grounding cue.
   *
   * Toned down from 0.42 to 0.26 alpha: while a selection ghosted the building
   * the disc was no longer depth-rejected and surfaced as a dark circle roughly
   * 1.24× the footprint, hanging inside the model. It is now also hidden
   * whenever its model is ghosted (see highlightModels) and drawn first, so it
   * can only ever read as ground shading.
   */
  function makeShadowDisc(radius) {
    if (!shadowTexture) {
      const c = document.createElement('canvas');
      c.width = c.height = 128;
      const ctx = c.getContext('2d');
      const g = ctx.createRadialGradient(64, 64, 8, 64, 64, 64);
      g.addColorStop(0, 'rgba(0,0,0,0.26)');
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
    mesh.renderOrder = -1;
    return mesh;
  }

  function buildEntryScene(holder) {
    // The studio environment (attached in the layer's render pass) provides the
    // fill light; these two only add directional definition, so they are far
    // dimmer than the pre-environment values.
    const scene = new THREE.Scene();
    const hemi = new THREE.HemisphereLight(0xffffff, 0x46506b, dark ? 0.4 : 0.5);
    const sun = new THREE.DirectionalLight(0xffffff, dark ? 1.1 : 1.4);
    sun.position.set(0.6, 1, 0.8);
    scene.add(hemi, sun, holder);
    return scene;
  }

  async function attachModel(entry, el) {
    const ref = modelRefOf(el);
    if (!ref) return;
    // Surface a "loading building model" chip: the 49 MB IFC is parsed by web-ifc
    // client-side with no progress UI, so until it resolves the building isn't on
    // the map and the scene reads as empty/broken.
    modelsLoading += 1;
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
      // A declared real-world size (ots:modelSizeMeters) wins over guessing from
      // the model's own units — STL landmarks (Big Ben, Empire State) have
      // arbitrary units, so without this they fall back to ~90 m and look tiny.
      const meters =
        el.size_meters && el.size_meters > 0
          ? el.size_meters
          : realWorldMeters(cached, FALLBACK_FOOTPRINT_M);
      const box = new THREE.Box3().setFromObject(model);
      const radius = Math.max(box.max.x - box.min.x, box.max.z - box.min.z) * 0.62;
      const holder = new THREE.Group();
      const disc = makeShadowDisc(radius);
      holder.add(model, disc);
      entry.modelGroup = model;
      entry.shadow = disc;
      entry.box = box;
      entry.scene = buildEntryScene(holder);
      entry.meters = meters;
      entry.anchorUsed = anchor;
      entry.footprint = null; // computed on demand by the suppression pass
      entry.isIfc = ref.format === 'ifc'; // multi-element model → eligible for x-ray
      // CityJSON/CityGML blocks carry per-CityObject picking (the building-level
      // x-ray + info); cache the metadata by object id for hover/popup.
      entry.isCity = ref.format === 'cityjson' || ref.format === 'citygml';
      entry.cityObjectById = new Map((cached.userData.geo?.cityObjects || []).map((o) => [o.id, o]));
      entry.mercMatrix = mercMatrixFor(anchor, meters);
      themeMaterials();
      highlightModels();
      updateWalkSuggest(); // a building may have loaded while already zoomed in
      // The basemap blocks this model now covers can only be resolved once its
      // geometry exists, and nothing else fires after an async load — the map may
      // already be idle, so a landmark would otherwise z-fight the basemap block
      // until the next pan.
      scheduleBuildingSuppression();
      map?.triggerRepaint();
    } catch {
      modelsFailed += 1; // model failed to load — the vector dot remains
    } finally {
      modelsLoading -= 1;
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

  // Ghost opacity for the rest of the building during an x-ray. Kept high enough
  // that the building still reads as context (0.16 made it vanish entirely).
  const GHOST_OPACITY = 0.34;
  /** Emissive intensity of a selected (but not overlaid) model. */
  const SELECT_EMISSIVE = 0.6;

  /** Stash a material's original render flags once, so any paint is reversible. */
  function stashMat(m) {
    if (m.userData.origOpacity === undefined) {
      m.userData.origOpacity = m.opacity;
      m.userData.origTransparent = m.transparent;
      m.userData.origDepthWrite = m.depthWrite;
      m.userData.origDepthTest = m.depthTest;
      // Stash the base colour too, so a colour-override highlight is reversible.
      if (m.color && m.userData.origColor === undefined) m.userData.origColor = m.color.getHex();
    }
  }

  // ── Eased highlight transitions ─────────────────────────────────────────────
  // Selection/x-ray used to be an INSTANT material swap (opacity 1↔0.16, emissive
  // 0↔0.55) with one forced repaint — the single biggest "feels unsmooth" cause.
  // Now paintMat sets the discrete flags (transparent/depthWrite/depthTest) up
  // front and registers the two *scalar* changes (opacity, emissiveIntensity) as
  // a tween; a short rAF loop eases them over TWEEN_MS, repainting each frame.
  // Re-selecting RETARGETS from the current value (no restart-snap), so rapid
  // clicks stay fluid.
  const TWEEN_MS = 200;
  const tweenMats = new Set();
  let tweenStart = 0;
  let tweenRAF = 0;
  const easeOutCubic = (t) => 1 - Math.pow(1 - t, 3);
  const nowMs = () => (typeof performance !== 'undefined' ? performance.now() : Date.now());

  function runTweens() {
    const t = Math.min(1, (nowMs() - tweenStart) / TWEEN_MS);
    const k = easeOutCubic(t);
    for (const m of tweenMats) {
      const td = m.userData.tween;
      if (!td) continue;
      m.opacity = td.fromOpacity + (td.toOpacity - td.fromOpacity) * k;
      if ('emissiveIntensity' in m) {
        m.emissiveIntensity = td.fromEmis + (td.toEmis - td.fromEmis) * k;
      }
    }
    map?.triggerRepaint();
    if (t >= 1) {
      for (const m of tweenMats) {
        const td = m.userData.tween;
        if (!td) continue;
        // Settle the final discrete flags (e.g. transparent back to false on a
        // now-solid selected material) once the scalars have arrived.
        m.transparent = td.finalTransparent;
        m.depthWrite = td.finalDepthWrite;
        m.depthTest = td.finalDepthTest;
        m.opacity = td.toOpacity;
        if ('emissiveIntensity' in m) m.emissiveIntensity = td.toEmis;
        m.userData.tween = null;
      }
      tweenMats.clear();
      tweenRAF = 0;
      map?.triggerRepaint();
      return;
    }
    tweenRAF = requestAnimationFrame(runTweens);
  }

  function startTween() {
    tweenStart = nowMs();
    if (!tweenRAF) tweenRAF = requestAnimationFrame(runTweens);
  }

  /**
   * Stage a material for one of four states, easing the scalar changes:
   *  - `selected`     — glow (whole non-IFC model picked); normal depth.
   *  - `selectedXray` — glow AND render ON TOP (depthTest off) so the picked IFC
   *                     sub-element is visible through the ghosted shell. Only
   *                     reachable while the X-ray toggle is on.
   *  - `ghost`        — faint, see-through, doesn't occlude (x-ray of the rest).
   *  - `normal`       — restore the stashed original flags.
   */
  function paintMat(m, state) {
    if (!m) return;
    stashMat(m);
    const emis = 'emissive' in m;
    let toOpacity;
    let toEmis;
    let finalTransparent;
    let finalDepthWrite;
    let finalDepthTest;
    if (state === 'selected' || state === 'selectedXray') {
      // Non-destructive glow: light the material's OWN colour rather than
      // repainting it orange, and leave opacity/transparency exactly as they
      // were, so a CityJSON facade keeps its per-surface colours and a glassy
      // surface stays glassy. A material with no emissive channel (a plain
      // MeshBasicMaterial) has no other way to read as selected, so those — and
      // only those — still take the colour override.
      if (emis) m.emissive.setHex(HL_COLOR);
      else if (m.color) m.color.setHex(HL_COLOR);
      toEmis = emis ? SELECT_EMISSIVE : 0;
      toOpacity = m.userData.origOpacity;
      finalTransparent = m.userData.origTransparent;
      finalDepthWrite = m.userData.origDepthWrite;
      finalDepthTest = state === 'selectedXray' ? false : m.userData.origDepthTest;
      m.depthWrite = finalDepthWrite;
      m.depthTest = finalDepthTest;
    } else if (state === 'ghost') {
      toEmis = 0;
      toOpacity = Math.min(m.userData.origOpacity, GHOST_OPACITY);
      finalTransparent = true;
      finalDepthWrite = false;
      finalDepthTest = m.userData.origDepthTest;
      m.depthWrite = false; // stop occluding the selected element immediately
      m.depthTest = finalDepthTest;
    } else {
      // Restore the stashed base colour (undo a 'selected' colour override).
      if (m.color && m.userData.origColor !== undefined) m.color.setHex(m.userData.origColor);
      toEmis = 0;
      toOpacity = m.userData.origOpacity;
      finalTransparent = m.userData.origTransparent;
      finalDepthWrite = m.userData.origDepthWrite;
      finalDepthTest = m.userData.origDepthTest;
      m.depthWrite = m.userData.origDepthWrite;
      m.depthTest = m.userData.origDepthTest;
    }
    const curOpacity = m.opacity;
    const curEmis = emis ? m.emissiveIntensity : 0;
    // No scalar change and flags already settled → no tween needed.
    if (
      Math.abs(curOpacity - toOpacity) < 0.001 &&
      Math.abs(curEmis - toEmis) < 0.001 &&
      m.transparent === finalTransparent
    ) {
      m.transparent = finalTransparent;
      m.opacity = toOpacity;
      if (emis) m.emissiveIntensity = toEmis;
      m.userData.tween = null;
      return;
    }
    // Keep transparent during the ease so partial opacity actually blends.
    m.transparent = true;
    m.userData.tween = {
      fromOpacity: curOpacity,
      toOpacity,
      fromEmis: curEmis,
      toEmis,
      finalTransparent,
      finalDepthWrite,
      finalDepthTest,
    };
    tweenMats.add(m);
  }

  // CityObject ↔ element linkage, derived from each element's CityJSON
  // `#objectId` fragment ref (the seed points a building's model at its object).
  // A picked CityObject resolves to its RDF element (→ open the inspector), and a
  // selected element resolves to the CityObject to x-ray in the block's model.
  let cityObjectToElement = new Map(); // objId → elementId
  let elementToCityObject = new Map(); // elementId → objId
  $: {
    const o2e = new Map();
    const e2o = new Map();
    for (const el of elements) {
      for (const ref of modelRefsOf(el)) {
        if (ref.format !== 'cityjson' && ref.format !== 'citygml') continue;
        const obj = cityObjectFragment(ref.url);
        if (!obj) continue;
        if (!o2e.has(obj)) o2e.set(obj, el.id);
        if (!e2o.has(el.id)) e2o.set(el.id, obj);
      }
    }
    cityObjectToElement = o2e;
    elementToCityObject = e2o;
  }

  // Children index (parent id → child elements) for subtree highlighting, so
  // selecting a storey lights every wall/slab it contains. Rebuilt per elements.
  let childrenByParent = new Map();
  $: {
    const m = new Map();
    for (const el of elements) {
      if (el.parent) {
        const arr = m.get(el.parent);
        if (arr) arr.push(el);
        else m.set(el.parent, [el]);
      }
    }
    childrenByParent = m;
  }

  /** GlobalIds of an element + all its BOT descendants (a container's subtree). */
  function descendantGuidSet(id) {
    const set = new Set();
    const self = elements.find((e) => e.id === id);
    if (self?.ifc_guid) set.add(self.ifc_guid);
    const stack = [id];
    const seen = new Set([id]);
    while (stack.length) {
      for (const k of childrenByParent.get(stack.pop()) || []) {
        if (seen.has(k.id)) continue;
        seen.add(k.id);
        if (k.ifc_guid) set.add(k.ifc_guid);
        stack.push(k.id);
      }
    }
    return set;
  }

  function highlightModels() {
    // The selected element's GlobalId set (its whole BOT subtree for a spatial
    // container — a storey lights every wall/slab it contains).
    const selGuids = selected ? descendantGuidSet(selected) : null;
    const byGuid = selGuids && selGuids.size > 0;
    // The CityObject to x-ray inside a shared block: a locally-picked object with
    // no RDF element (citySel) wins; otherwise the selected element's own
    // CityObject (when it is one of the block's buildings).
    const activeCityObj = citySel?.objId ?? (selected ? elementToCityObject.get(selected) : null);
    const cityWanted = activeCityObj ? new Set([activeCityObj]) : null;
    for (const [id, e] of entries) {
      if (e.isIfc) {
        // The IFC building is MERGED (a few per-material meshes), so a single
        // element can't be lit by swapping a mesh's material — the selection is
        // drawn as a highlight copy over the original instead. The surrounding
        // building KEEPS its full opacity and keeps occluding; only the opt-in
        // X-ray ghosts it so an enclosed element shows through.
        const isSel = byGuid && groupHasGuid(e.modelGroup, selGuids);
        const ghost = isSel && xrayOn;
        e.modelGroup?.traverse((n) => {
          if (!n.isMesh || !n.material || n.userData.isOverlay) return;
          n.renderOrder = 0;
          const mats = Array.isArray(n.material) ? n.material : [n.material];
          for (const m of mats) paintMat(m, ghost ? 'ghost' : 'normal');
        });
        // A ghosted shell stops writing depth, which un-hides the contact-shadow
        // disc underneath it as a dark ring floating inside the building.
        if (e.shadow) e.shadow.visible = !ghost;
        setIfcOverlay(e, isSel ? selGuids : null, xrayOn);
      } else if (e.isCity) {
        // A CityJSON block is MERGED by colour, so — exactly like IFC — the
        // picked building is drawn as a highlight copy over the block. Ghosting
        // the rest of the block is opt-in for the same reason it is for IFC.
        const isSel = cityWanted && groupHasCityObject(e.modelGroup, cityWanted);
        const ghost = isSel && xrayOn;
        e.modelGroup?.traverse((n) => {
          if (!n.isMesh || !n.material || n.userData.isOverlay) return;
          n.renderOrder = 0;
          const mats = Array.isArray(n.material) ? n.material : [n.material];
          for (const m of mats) paintMat(m, ghost ? 'ghost' : 'normal');
        });
        if (e.shadow) e.shadow.visible = !ghost;
        setCityOverlay(e, isSel ? cityWanted : null, xrayOn);
      } else {
        // Single-object model (STL/glTF landmark) — light the whole model by id.
        const on = !byGuid && id === selected;
        e.modelGroup?.traverse((n) => {
          if (!n.isMesh || !n.material) return;
          n.renderOrder = 0;
          const mats = Array.isArray(n.material) ? n.material : [n.material];
          for (const m of mats) paintMat(m, on ? 'selected' : 'normal');
        });
      }
    }
    // Ease the staged scalar changes (falls back to a single repaint if nothing
    // actually needs animating).
    if (tweenMats.size) startTween();
    else map?.triggerRepaint();
  }

  /** Drop the entry's current highlight copy, if any. */
  function clearOverlay(e) {
    if (!e.overlay) return;
    e.overlay.parent?.remove(e.overlay);
    disposeHighlightOverlay(e.overlay);
    e.overlay = null;
  }

  /** Add (or remove) a glowing copy of `selGuids` over the merged IFC building —
   *  the per-element highlight a merged mesh can't do by material swap. The copy
   *  is non-pickable and disposed on the next change. In the default mode it
   *  RESPECTS depth (it sits on the geometry, and whatever stands in front still
   *  covers it); `xray` restores the draw-through behaviour. */
  function setIfcOverlay(e, selGuids, xray) {
    clearOverlay(e);
    if (!selGuids || !e.modelGroup) return;
    const ov = buildHighlightOverlay(e.modelGroup, selGuids, xray);
    if (ov) attachOverlay(e, ov, xray);
  }

  /** The CityJSON counterpart of {@link setIfcOverlay}: highlights the picked
   *  building(s) of a merged block, one CityObject at a time. */
  function setCityOverlay(e, objIds, xray) {
    clearOverlay(e);
    if (!objIds || !e.modelGroup) return;
    const ov = buildCityHighlightOverlay(e.modelGroup, objIds, xray);
    if (ov) attachOverlay(e, ov, xray);
  }

  /** Attach `ov` (already materialised as a highlight) to the model group and
   *  fade it in on the same 200 ms curve the rest of the selection uses —
   *  buildHighlightOverlay hands it over fully transparent and unlit. */
  function attachOverlay(e, ov, xray) {
    // Child of the model group so it inherits the same placement transform as the
    // merged meshes the geometry was extracted from.
    e.modelGroup.add(ov);
    e.overlay = ov;
    ov.traverse((n) => {
      if (!n.isMesh) return;
      const mats = Array.isArray(n.material) ? n.material : [n.material];
      for (const m of mats) {
        if (!m) continue;
        m.userData.tween = {
          fromOpacity: 0,
          toOpacity: 1,
          fromEmis: 0,
          toEmis: HL_EMISSIVE,
          finalTransparent: false,
          finalDepthWrite: true,
          finalDepthTest: !xray,
        };
        tweenMats.add(m);
      }
    });
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
      let cityObj = null;
      if (precise && e.modelGroup && (e.isIfc || e.isCity)) {
        e.scene.updateMatrixWorld(); // refresh world matrices if a frame hasn't since load
        RAYCASTER.ray.origin.copy(RAY.origin);
        RAYCASTER.ray.direction.copy(RAY.direction);
        const hits = RAYCASTER.intersectObject(e.modelGroup, true);
        // Take the nearest hit that owns an identity — a merged IFC building
        // resolves the GlobalId from the picked triangle, a CityJSON block the
        // CityObject; overlay/non-rooted triangles carry none and are skipped.
        for (const hit of hits) {
          if (e.isIfc) {
            const g = ifcGuidAt(hit.object, hit.faceIndex);
            if (g) {
              guid = g;
              break;
            }
          } else {
            const oid = cityObjectIdAt(hit.object, hit.faceIndex);
            if (oid) {
              cityObj = oid;
              break;
            }
          }
        }
      }
      if (!best || d < best.d) best = { id, guid, cityObj, d };
    }
    return best ? { id: best.id, guid: best.guid, cityObj: best.cityObj } : null;
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
    scheduleBuildingSuppression();
  }

  // ── Basemap-building suppression ────────────────────────────────────────────
  // Our georeferenced models stand where the basemap also raises a generic grey
  // block — the two overlap and z-fight (Big Ben inside a slab). The fix is a
  // DATA-level filter: each model's real ground footprint is computed in WGS84
  // (lib/viewer/footprint.ts) and every basemap building that INTERSECTS it is
  // filtered out, in every layer of the style that draws buildings.
  //
  // What this replaces: the old pass queried the *rendered* features inside a
  // screen-space square around the model's anchor POINT. That under-covered on
  // four independent counts — the radius was capped at 70 px, the
  // metres-per-pixel conversion used the 256-px-tile constant while MapLibre's
  // zoom is in the 512-px scheme (so it was 2× too small again), a pitched
  // camera turns a screen square into a ground trapezoid, and
  // queryRenderedFeatures sees nothing off-screen, below the layer's minzoom or
  // before tiles have arrived. It also only ever filtered our own layer, so the
  // hosted Liberty style's `building-3d` kept drawing the block straight through
  // the model. A geometry filter is independent of the camera entirely: it is
  // computed once per model and stays correct across pans, zooms and style swaps.
  /** Absorbs the mismatch between a generalised OSM footprint and a survey
   *  accurate IFC/CityJSON one, without swallowing the neighbours. */
  const SUPPRESSION_BUFFER_M = 0.5;
  /** Vertices sampled per model to trace its outline (see sampleModelXZ). */
  const FOOTPRINT_SAMPLE_CAP = 4000;
  /** Each building layer's own filter, stashed before we ever touch it. Cleared
   *  on style.load — layer objects are recreated by setStyle. */
  let baseFilters = new Map();
  let suppressTimer = 0;
  /** 'geometry' uses MapLibre's `distance` expression (evaluated in the tile
   *  worker, viewport-independent); 'ids' is the fallback below. Latched once. */
  let suppressionMode = 'geometry';
  /** Bumped when a building source finishes loading tiles — only the id fallback
   *  depends on which tiles are present. */
  let sourceGen = 0;
  let lastSuppressionKey = '';

  /**
   * A bounded sample of a model's vertices as model-local `[x, z]` pairs — the
   * space `mercMatrixFor()` consumes. An IFC building has millions of vertices
   * and the footprint only needs its outline, so this strides through the
   * position buffers instead of reading every vertex.
   */
  function sampleModelXZ(entry) {
    const out = [];
    const group = entry.modelGroup;
    if (!group) return out;
    group.updateMatrixWorld(true);
    let total = 0;
    group.traverse((n) => {
      if (n.isMesh && !n.userData.isOverlay && n.geometry?.attributes?.position) {
        total += n.geometry.attributes.position.count;
      }
    });
    if (!total) return out;
    const stride = Math.max(1, Math.ceil(total / FOOTPRINT_SAMPLE_CAP));
    const v = new THREE.Vector3();
    group.traverse((n) => {
      if (!n.isMesh || n.userData.isOverlay) return;
      const pos = n.geometry?.attributes?.position;
      if (!pos) return;
      for (let i = 0; i < pos.count; i += stride) {
        v.fromBufferAttribute(pos, i).applyMatrix4(n.matrixWorld);
        out.push([v.x, v.z]);
      }
    });
    return out;
  }

  /** Ground footprint polygons of a built model, cached on the entry: they are
   *  camera-independent, so they only change when the model itself does. */
  function entryFootprints(e) {
    const anchor = e.anchorUsed ?? e.anchor;
    if (!anchor || !e.modelGroup || !e.box) return [];
    if (e.footprint) return e.footprint;
    const meters = e.meters || 30;
    const pts = sampleModelXZ(e);
    e.footprint = pts.length
      ? footprintsFromLocalPoints(pts, meters, anchor, { normalisedDim: NORMALISED_DIM })
      : // No readable geometry (a placeholder, an exotic loader) — fall back to
        // the bounding box, padded, which is what the old code approximated.
        [
          footprintPolygon(e.box, meters, anchor, {
            normalisedDim: NORMALISED_DIM,
            padMeters: SUPPRESSION_BUFFER_M,
          }),
        ];
    return e.footprint;
  }

  /**
   * Set `filter` on every building layer, preserving the layer's own filter.
   * Returns 'ok', 'retry' (a transient failure — the style was mid-load) or
   * 'unsupported' (MapLibre would not accept the expression, which latches the
   * id fallback).
   */
  function applyFilterToBuildingLayers(filter) {
    let result = 'ok';
    for (const id of buildingLayerIds(map)) {
      if (!baseFilters.has(id)) baseFilters.set(id, map.getFilter(id) ?? null);
      const combined = combineFilters(baseFilters.get(id), filter);
      if (combined === undefined) {
        // A LEGACY (non-expression) filter cannot be wrapped in ['all', …] —
        // the style would fail validation — so that layer is left untouched.
        if (import.meta.env.DEV) {
          // eslint-disable-next-line no-console
          console.warn(`[viewer] ${id} has a legacy filter; basemap suppression skipped there`);
        }
        continue;
      }
      try {
        map.setFilter(id, combined);
      } catch {
        result = 'retry'; // e.g. "Style is not done loading"
        continue;
      }
      // setFilter does NOT throw on an expression it rejects: it fires an
      // 'error' event and leaves the layer's filter untouched. Read it back to
      // find out whether the filter actually landed.
      if (JSON.stringify(map.getFilter(id) ?? null) !== JSON.stringify(combined ?? null)) {
        result = 'unsupported';
      }
    }
    return result;
  }

  /**
   * Fallback for an engine or style where the `distance` expression is not
   * available: intersect the footprint with each LOADED building feature in JS
   * and hide the matches by feature id. `querySourceFeatures` (not
   * `queryRenderedFeatures`) is used so the answer does not depend on the
   * viewport or the layer's zoom range — but it still only sees tiles that have
   * been fetched, hence the re-run on 'sourcedata'. Features that carry no id
   * cannot be addressed by an id filter at all and therefore stay visible;
   * OpenFreeMap's building features all carry planetiler ids, so in practice
   * this only loses coverage on an exotic tile source.
   */
  function applyIdSuppression(multi) {
    const rings = multi ? multi.coordinates.map((c) => c[0]) : [];
    const ids = new Set();
    if (rings.length) {
      const sources = new Set();
      for (const id of buildingLayerIds(map)) {
        const src = map.getLayer(id)?.source;
        if (src) sources.add(src);
      }
      for (const src of sources) {
        let feats = [];
        try {
          feats = map.querySourceFeatures(src, { sourceLayer: OSM_BUILDING_SOURCE_LAYER });
        } catch {
          continue;
        }
        for (const f of feats) {
          if (f.id === undefined || f.id === null) continue;
          const g = f.geometry;
          const polys =
            g?.type === 'Polygon' ? [g.coordinates] : g?.type === 'MultiPolygon' ? g.coordinates : [];
          for (const p of polys) {
            if (rings.some((r) => polygonsIntersect(r, p[0]))) {
              ids.add(f.id);
              break;
            }
          }
        }
      }
    }
    applyFilterToBuildingLayers(buildingIdFilter([...ids]));
  }

  function applyBuildingSuppression() {
    if (!map || !styleReady) return;
    // Nothing to suppress on a raster basemap — and bailing before the key is
    // latched means a style that hasn't attached its layers yet retries later.
    if (!buildingLayerIds(map).length) return;
    const polys = [];
    for (const e of entries.values()) {
      // A hidden model (the "3D models" layer toggled off) gives its blocks back.
      if (!e.modelGroup || e.modelGroup.visible === false) continue;
      polys.push(...entryFootprints(e));
    }
    const multi = footprintsToMultiPolygon(polys);
    // setFilter marks the whole vector source for reload, so skip a no-op pass.
    // The tile generation only matters to the id fallback (see applyIdSuppression).
    const key = `${suppressionMode}|${suppressionMode === 'ids' ? sourceGen : 0}|${
      multi ? JSON.stringify(multi.coordinates) : ''
    }`;
    if (key === lastSuppressionKey) return;
    lastSuppressionKey = key;
    if (suppressionMode === 'ids') {
      applyIdSuppression(multi);
      return;
    }
    const res = applyFilterToBuildingLayers(buildingSuppressionFilter(multi, SUPPRESSION_BUFFER_M));
    if (res === 'retry') {
      lastSuppressionKey = ''; // transient — let the next trigger try again
    } else if (res === 'unsupported') {
      suppressionMode = 'ids';
      lastSuppressionKey = '';
      applyIdSuppression(multi);
    }
  }

  /** Coalesce suppression passes: several models finishing within a frame of
   *  each other would otherwise reload the vector source once each. */
  function scheduleBuildingSuppression() {
    if (suppressTimer) return;
    suppressTimer = setTimeout(() => {
      suppressTimer = 0;
      applyBuildingSuppression();
    }, 60);
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
    // Every building layer of the active style, not just ours: the hosted light
    // style draws its own `building` fill and `building-3d` extrusion, which the
    // toggle never reached before.
    for (const id of buildingLayerIds(map)) setLayerVis(id, layersOn.osm3d);
    for (const e of entries.values()) {
      if (e.modelGroup) e.modelGroup.visible = layersOn.models;
    }
    map.triggerRepaint?.();
  }
  function toggleLayer(key) {
    layersOn = { ...layersOn, [key]: !layersOn[key] };
    applyLayerVisibility();
    // Hiding the models must give their basemap blocks back.
    if (key === 'models') scheduleBuildingSuppression();
  }

  /** Toggle the see-through selection. Called directly (not via a reactive
   *  statement) so it does not fight the `lastSelectedPaint` guard below. */
  function toggleXray() {
    xrayOn = !xrayOn;
    highlightModels();
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
    for (const e of entries.values()) {
      if (e.overlay) {
        e.overlay.parent?.remove(e.overlay);
        disposeHighlightOverlay(e.overlay);
      }
    }
    entries = new Map();
    citySel = null;
    hideCityPopup();
    // Which base CityJSON/CityGML files carry a WHOLE-file (no-fragment) reference:
    // those render the entire block once; every `#objectId` fragment ref for the
    // same file then only maps a pick back to its element (no second render).
    const cityWholeFileBases = new Set();
    for (const el of elements) {
      for (const ref of modelRefsOf(el)) {
        if ((ref.format === 'cityjson' || ref.format === 'citygml') && !cityObjectFragment(ref.url)) {
          cityWholeFileBases.add(cityBaseUrl(ref.url));
        }
      }
    }
    const placedCityKeys = new Set(); // dedup identical (file[, object]) renders
    for (const el of elements) {
      const anchor = modelAnchor(el);
      const entry = { anchor, anchorUsed: null, scene: null, modelGroup: null, box: null, mercMatrix: null };
      entries.set(el.id, entry);
      // Load a model only when it can actually be placed: it has an anchor, or
      // it self-georeferences (CityJSON/CityGML). Anchorless IFC *element* refs
      // (a `#GlobalId` fragment with no WKT) would just parse the whole building
      // and bail — the anchored Site already stands it on the map.
      const ref = modelRefOf(el);
      const isCity = ref && (ref.format === 'cityjson' || ref.format === 'citygml');
      if (isCity) {
        // A self-georeferenced CityJSON places itself, so several elements pointing
        // at the SAME file (a zone + its buildings, or duplicate block refs across
        // graphs) would each re-render it at the identical spot — the "duplicates"
        // artefact. Render each file ONCE: a whole-file ref supersedes its object
        // fragments, and identical refs are deduped.
        const base = cityBaseUrl(ref.url);
        const frag = cityObjectFragment(ref.url);
        if (frag && cityWholeFileBases.has(base)) continue; // the whole-file entry renders it
        const key = `${base}#${frag || ''}`;
        if (placedCityKeys.has(key)) continue; // this exact geometry is already placed
        placedCityKeys.add(key);
        attachModel(entry, el);
        continue;
      }
      if (anchor) attachModel(entry, el);
    }
    if (import.meta.env.DEV) window.__otsViewerEntries = entries; // dev: re-point after reassign
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

  // ── Framing a target ────────────────────────────────────────────────────────
  // Selecting something must never move the camera by itself; that was the
  // "camera collapse" — every pick flew in to zoom 16.4 and slammed the pitch to
  // 55°, throwing away whatever viewpoint the user had set up. focusElement now
  // only runs for an EXPLICIT action (the modal's "Show on map", the map's own
  // "Zoom to selection" button) and is conservative even then.
  /** Zoom a framing action pulls up to when the target is too far away. */
  const COMFORT_ZOOM = 16.4;
  /** Fraction of the canvas that counts as "comfortably framed". */
  const COMFORT_BOX = 0.6;
  /** Fraction of the canvas an extended feature must cover to count as framed. */
  const COMFORT_SPAN = 0.12;

  const reducedMotion = () =>
    typeof window !== 'undefined' &&
    window.matchMedia?.('(prefers-reduced-motion: reduce)').matches === true;
  const flightMs = () => (reducedMotion() ? 0 : 800);

  /**
   * "Comfortably in view" — the rule that decides whether a framing action moves
   * the camera at all. Two conditions, both required:
   *  1. every given point projects inside the middle 60 % of the canvas, so the
   *     target is not tucked behind the map chrome or the panel dock; and
   *  2. it is big enough to actually look at — an extended feature must cover at
   *     least 12 % of the canvas, and a bare anchor point (which has no extent)
   *     falls back to "the camera is at least at COMFORT_ZOOM".
   * When both hold the camera stays exactly where the user put it.
   */
  function comfortablyInView(points) {
    if (!map || !points.length) return false;
    const canvas = map.getCanvas();
    const w = canvas.clientWidth || 1;
    const h = canvas.clientHeight || 1;
    const mx = (w * (1 - COMFORT_BOX)) / 2;
    const my = (h * (1 - COMFORT_BOX)) / 2;
    let x0 = Infinity;
    let x1 = -Infinity;
    let y0 = Infinity;
    let y1 = -Infinity;
    for (const p of points) {
      const c = map.project({ lng: p[0], lat: p[1] });
      if (c.x < mx || c.x > w - mx || c.y < my || c.y > h - my) return false;
      x0 = Math.min(x0, c.x);
      x1 = Math.max(x1, c.x);
      y0 = Math.min(y0, c.y);
      y1 = Math.max(y1, c.y);
    }
    const span = Math.max(x1 - x0, y1 - y0);
    return span >= COMFORT_SPAN * Math.min(w, h) || map.getZoom() >= COMFORT_ZOOM;
  }

  /**
   * Bring an element into view. Pitch and bearing are left exactly as the user
   * set them (fitBounds resets the bearing unless it is passed explicitly), the
   * zoom is only ever RAISED, and a target that is already comfortably framed
   * does not move the camera at all — pass `{ force: true }` to frame it anyway.
   */
  export function focusElement(id, opts = {}) {
    if (!map) return;
    let el = elements.find((e) => e.id === id);
    if (!el) return;
    // An element with no geometry of its own (an IFC sub-element — a wall, a
    // door) frames its nearest located ancestor (the building's Site anchor).
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
      if (!b) return;
      const corners = [
        [b[0][1], b[0][0]],
        [b[1][1], b[1][0]],
      ];
      if (!opts.force && comfortablyInView(corners)) return;
      map.fitBounds([corners[0], corners[1]], {
        padding: 90,
        bearing: map.getBearing(),
        duration: flightMs(),
      });
      return;
    }
    const anchor = entry?.anchorUsed ?? modelAnchor(el);
    if (!anchor) return;
    if (!opts.force && comfortablyInView([anchor])) return;
    // easeTo, not flyTo: the zoom-out-and-back arc is what read as the camera
    // "collapsing" away from the building.
    map.easeTo({
      center: anchor,
      zoom: Math.max(map.getZoom(), COMFORT_ZOOM),
      duration: flightMs(),
    });
  }

  /** The map's own "zoom to selection" control — the explicit replacement for
   *  the camera travel that used to happen on every pick. */
  function zoomToSelection() {
    if (selected) focusElement(selected, { force: true });
  }

  // ── "Walk through this building" suggestion ─────────────────────────────────
  // When the user is zoomed in close on an IFC building, tell the parent which
  // one, so it can offer a first-person walkthrough of that model.
  let walkSuggestId = null;
  function updateWalkSuggest() {
    if (!map) return;
    let suggest = null;
    if (map.getZoom() >= 17.6) {
      const c = map.getCenter();
      const bounds = map.getBounds();
      let best = null;
      for (const [id, e] of entries) {
        if (!e.isIfc || !e.modelGroup || e.modelGroup.visible === false) continue;
        const a = e.anchorUsed ?? e.anchor;
        if (!a || !bounds.contains(a)) continue;
        const dx = a[0] - c.lng;
        const dy = a[1] - c.lat;
        const d = dx * dx + dy * dy;
        if (!best || d < best.d) best = { id, d };
      }
      if (best) {
        const el = elements.find((x) => x.id === best.id);
        suggest = { id: best.id, label: el?.label || best.id.split(/[/#]/).pop() };
      }
    }
    if ((suggest?.id || null) !== walkSuggestId) {
      walkSuggestId = suggest?.id || null;
      dispatch('walksuggest', suggest);
    }
  }

  // ── Interaction ─────────────────────────────────────────────────────────────
  const HIT_LAYERS = ['ots-point', 'ots-line', 'ots-fill'];
  const hitLayers = () => HIT_LAYERS.filter((l) => map.getLayer(l));

  function onClick(e) {
    const pick = raycastModels(e.point, true);
    if (pick) {
      if (pick.guid) {
        // guid → DatasetViewer resolves it to the specific IFC sub-element and
        // opens that element's window.
        citySel = null;
        hideCityPopup();
        dispatch('select', { id: pick.id, guid: pick.guid });
        return;
      }
      if (pick.cityObj) {
        const elId = cityObjectToElement.get(pick.cityObj);
        if (elId) {
          // The building maps to an RDF element → open its rich inspector; the
          // highlight then follows `selected`.
          citySel = null;
          hideCityPopup();
          dispatch('select', { id: elId });
        } else {
          // Geometry-only building (e.g. a 3DBAG house) → local highlight + info popup.
          selectCityObject(pick.id, pick.cityObj, e.lngLat);
        }
        return;
      }
      // Whole single-object model (STL/glTF landmark).
      citySel = null;
      hideCityPopup();
      dispatch('select', { id: pick.id });
      return;
    }
    // Vector features (dots/lines/areas) — clear any floating building selection.
    citySel = null;
    hideCityPopup();
    highlightModels();
    const pad = 6;
    const box = [[e.point.x - pad, e.point.y - pad], [e.point.x + pad, e.point.y + pad]];
    const fs = map.queryRenderedFeatures(box, { layers: hitLayers() });
    if (fs.length) dispatch('select', { id: fs[0].properties.id });
  }

  // ── Geometry-only CityObject selection (a building with no RDF element) ──────
  const escHtml = (s) =>
    String(s).replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' })[c]);

  /** Local highlight + attributes popup for a picked building that has no
   *  linked-data element (the 3DBAG block's houses — geometry only). */
  function selectCityObject(entryId, objId, lngLat) {
    const info = entries.get(entryId)?.cityObjectById?.get(objId);
    citySel = { entryId, objId };
    highlightModels();
    const label = escHtml(info?.label || objId);
    const attrs = info?.attributes || {};
    const rows = Object.entries(attrs)
      .filter(([k]) => k !== 'identificatie')
      .map(([k, v]) => `<div class="cbp-row"><span>${escHtml(k)}</span><b>${escHtml(v)}</b></div>`)
      .join('');
    const ident = attrs.identificatie
      ? `<div class="cbp-id">${$i18nT('viewer.bagId')}: ${escHtml(attrs.identificatie)}</div>`
      : '';
    const html = `<div class="city-bldg-popup"><div class="cbp-title">${label}</div>${ident}${rows}</div>`;
    if (!cityPopup) {
      cityPopup = new maplibregl.Popup({
        closeButton: true, closeOnClick: false, offset: 14, className: 'city-popup', maxWidth: '260px',
      });
      // User closed it via the ✕ → drop the highlight too (a programmatic remove
      // clears citySel first, so this only fires for genuine user closes).
      cityPopup.on('close', () => {
        if (citySel) {
          citySel = null;
          highlightModels();
        }
      });
    }
    cityPopup.setLngLat(lngLat).setHTML(html).addTo(map);
  }

  function hideCityPopup() {
    cityPopup?.remove();
  }

  function onMouseMove(e) {
    if (!map) return;
    let label = null;
    // Hover uses the CHEAP box-level pick (model name), never the per-triangle
    // cast: the building is now a few MERGED meshes whose bounding spheres each
    // span the whole building, so a precise hover would triangle-test the entire
    // building every mouse-move. The exact wall/slab is still resolved on click
    // (a one-off precise cast), where the cost is paid once.
    const pick = raycastModels(e.point, false);
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
      maxPitch: 80, // low angle for inspecting building facades
      maxZoom: 23.5, // zoom right in on individual walls/beams (basemap over-zooms)
    });
    map.addControl(new maplibregl.NavigationControl({ visualizePitch: true }), 'top-right');
    map.addControl(new maplibregl.ScaleControl({ maxWidth: 110 }), 'bottom-left');
    map.on('style.load', () => {
      styleReady = true;
      // setStyle recreates every layer, so the stashed originals and the
      // last-applied key belong to the previous style and must go.
      baseFilters = new Map();
      lastSuppressionKey = '';
      ensureOverlays();
      themeMaterials();
    });
    // No 'idle' handler any more: the suppression is a data-level filter, so
    // nothing about the camera can change its answer — and re-running it per
    // idle both cost a queryRenderedFeatures and made the hidden set depend on
    // where the user had happened to look. Only the id fallback cares which
    // tiles have arrived.
    map.on('sourcedata', (ev) => {
      if (suppressionMode !== 'ids' || !ev.isSourceLoaded) return;
      sourceGen += 1;
      scheduleBuildingSuppression();
    });
    map.on('moveend', updateWalkSuggest);
    map.on('click', onClick);
    map.on('mousemove', onMouseMove);
    map.on('mouseout', () => {
      hoverPopup?.remove();
      hoverPopup = null;
      map.getCanvas().style.cursor = '';
    });
    if (import.meta.env.DEV) {
      window.__otsViewerMap = map; // dev console handle
      window.__otsViewerEntries = entries; // dev: inspect model groups + overlays
    }
    rebuildData();
  });

  onDestroy(() => {
    unsubTheme();
    if (tweenRAF) cancelAnimationFrame(tweenRAF);
    if (suppressTimer) clearTimeout(suppressTimer);
    for (const e of entries.values()) {
      if (e.overlay) {
        e.overlay.parent?.remove(e.overlay);
        disposeHighlightOverlay(e.overlay);
      }
    }
    hoverPopup?.remove();
    cityPopup?.remove();
    cityPopup = null;
    if (map) map.remove();
    map = null;
    entries = new Map();
  });

  $: if (map && elements) rebuildData();
  /** Label for the selection chip; falls back to the IRI's last segment. */
  $: selectedLabel = selected
    ? elements.find((e) => e.id === selected)?.label || String(selected).split(/[/#]/).pop()
    : '';
  // Guard against redundant repaints: the parent re-sets `selected` to the same
  // value on every panel focus/close, which used to re-run a full per-mesh
  // traversal + repaint each time (a hitch on the 4000-element building).
  let lastSelectedPaint = null;
  $: if (map && selected !== undefined && selected !== lastSelectedPaint) {
    lastSelectedPaint = selected;
    // A parent-driven selection supersedes a floating geometry-only building pick.
    if (citySel) {
      citySel = null;
      hideCityPopup();
    }
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

  <!-- Selection chip. A depth-respecting highlight is invisible when the picked
       element is fully enclosed (an interior wall), so the chip is what tells the
       user the selection landed — and it carries the explicit framing control
       that replaced the camera flight every pick used to trigger. -->
  {#if selected}
    <div class="sel-chip">
      <span class="sel-dot" style:--sw={SELECT_COLOR}></span>
      <span class="sel-name" title={selectedLabel}>{selectedLabel}</span>
      <button
        class="sel-zoom"
        title={$i18nT('viewer.zoomToSelection')}
        aria-label={$i18nT('viewer.zoomToSelection')}
        on:click={zoomToSelection}
      ><Crosshair size={13} /></button>
    </div>
  {/if}

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
    <!-- Selection presentation. Off by default: a pick is drawn in place and
         respects depth, so the building around it stays solid and keeps
         occluding. Turning this on restores the old see-through x-ray. -->
    <label class="ml-row ml-divider" class:off={!xrayOn} title={$i18nT('viewer.layerXrayHint')}>
      <input type="checkbox" checked={xrayOn} on:change={toggleXray} />
      <span class="ml-swatch ml-xray" style:--sw={SELECT_COLOR}></span>
      <span class="ml-name">{$i18nT('viewer.layerXray')}</span>
    </label>
    <div class="ml-legend-sel">
      <span class="ml-swatch ml-dot" style:--sw={SELECT_COLOR}></span>
      <span class="ml-name">{$i18nT('viewer.layerSelected')}</span>
    </div>
  </div>

  {#if modelsLoading > 0 || modelsFailed > 0}
    <div class="model-load-chip" class:err={modelsLoading === 0 && modelsFailed > 0} role="status">
      {#if modelsLoading > 0}
        <span class="mlc-spin"></span>
        {#if $ifcProgress?.phase === 'parse'}
          {$i18nT('viewer.parsingModel')}
        {:else if $ifcProgress?.phase === 'fetch' && $ifcProgress.total > 0}
          {$i18nT('viewer.loadingModels')} {Math.min(99, Math.round(($ifcProgress.loaded / $ifcProgress.total) * 100))}%
        {:else if $ifcProgress?.phase === 'fetch'}
          {$i18nT('viewer.loadingModels')} {($ifcProgress.loaded / 1048576).toFixed(0)} MB
        {:else}
          {$i18nT('viewer.loadingModels')}
        {/if}
      {:else}
        {modelsFailed} {$i18nT('viewer.modelsFailed')}
      {/if}
    </div>
  {/if}
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

  /* Selection chip (top-left, under the basemap toggle) */
  .sel-chip {
    position: absolute;
    top: 48px;
    left: 10px;
    z-index: 5;
    max-width: min(46%, 260px);
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 4px 4px 4px 9px;
    border-radius: 999px;
    background: var(--bg-elevated, rgba(255, 255, 255, 0.95));
    border: 1px solid var(--line-soft, #e6eaef);
    box-shadow: 0 2px 10px rgba(0, 0, 0, 0.18);
    backdrop-filter: blur(8px);
    font-size: 0.74rem;
    color: var(--ink-900, #0f172a);
  }
  .sel-dot {
    width: 9px;
    height: 9px;
    flex: none;
    border-radius: 50%;
    background: var(--sw);
  }
  .sel-name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .sel-zoom {
    flex: none;
    display: flex;
    align-items: center;
    justify-content: center;
    width: 22px;
    height: 22px;
    padding: 0;
    border: 0;
    border-radius: 50%;
    background: var(--bg-accent-soft, #e7f0fb);
    color: var(--brand-600, #2563a8);
    cursor: pointer;
  }
  .sel-zoom:hover {
    background: var(--bg-accent, #d7e7fa);
  }
  .sel-zoom:focus-visible {
    outline: 2px solid var(--brand-500, #2f88d8);
    outline-offset: 2px;
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
  .ml-row input:focus-visible {
    outline: 2px solid var(--brand-500, #2f88d8);
    outline-offset: 1px;
  }
  .ml-divider {
    margin-top: 4px;
    padding-top: 5px;
    border-top: 1px solid var(--line-soft, #e6eaef);
  }
  /* X-ray swatch: a ringed dot, i.e. "seen through something". */
  .ml-xray {
    border-radius: 50%;
    background: color-mix(in srgb, var(--sw) 28%, transparent);
    border: 1.5px dashed var(--sw);
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

  /* Attributes popup for a geometry-only building (a 3DBAG house with no RDF). */
  :global(.city-popup .maplibregl-popup-content) {
    padding: 9px 12px;
    border-radius: 9px;
    background: var(--bg-elevated, #fff);
    color: var(--ink-900, #0f172a);
    box-shadow: 0 3px 14px rgba(0, 0, 0, 0.32);
    font-size: 0.78rem;
  }
  :global(.city-popup .maplibregl-popup-tip) {
    border-top-color: var(--bg-elevated, #fff);
  }
  :global(.city-popup .cbp-title) {
    font-weight: 600;
    font-size: 0.84rem;
    margin-bottom: 2px;
    color: var(--ink-900, #0f172a);
  }
  :global(.city-popup .cbp-id) {
    font-size: 0.68rem;
    color: var(--muted, #64748b);
    margin-bottom: 6px;
    word-break: break-all;
  }
  :global(.city-popup .cbp-row) {
    display: flex;
    justify-content: space-between;
    gap: 12px;
    padding: 1px 0;
  }
  :global(.city-popup .cbp-row span) {
    color: var(--muted, #64748b);
    text-transform: capitalize;
  }
  :global(.city-popup .cbp-row b) {
    color: var(--ink-900, #0f172a);
    font-weight: 600;
  }

  /* "Loading building model…" chip while web-ifc parses a heavy model. */
  .model-load-chip {
    position: absolute;
    left: 50%;
    bottom: 14px;
    transform: translateX(-50%);
    z-index: 5;
    display: inline-flex;
    align-items: center;
    gap: 7px;
    padding: 6px 13px;
    border-radius: 999px;
    background: var(--bg-elevated, rgba(255, 255, 255, 0.96));
    color: var(--ink-900, #0f172a);
    border: 1px solid var(--line-soft, #e6eaef);
    box-shadow: 0 2px 10px rgba(0, 0, 0, 0.18);
    font-size: 0.76rem;
    backdrop-filter: blur(8px);
  }
  .model-load-chip.err {
    color: var(--danger-500, #c0392b);
  }
  .mlc-spin {
    width: 12px;
    height: 12px;
    border: 2px solid color-mix(in srgb, var(--brand-500, #2f88d8) 35%, transparent);
    border-top-color: var(--brand-500, #2f88d8);
    border-radius: 50%;
    animation: mlc-spin 0.8s linear infinite;
  }
  @keyframes mlc-spin {
    to {
      transform: rotate(360deg);
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .mlc-spin {
      animation: none;
    }
  }
</style>
