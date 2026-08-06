<script>
  // Reusable interactive 3D model viewer: orbit (rotate / pan / zoom) over one
  // or more models, theme-aware (light/dark scene follows the app theme).
  // Used by the dataset explorer modal, the resource detail page and the
  // global term-preview overlay.
  import { onMount, onDestroy, createEventDispatcher } from 'svelte';
  import { t as i18nT } from 'svelte-i18n';
  import * as THREE from 'three';
  import { OrbitControls } from 'three/addons/controls/OrbitControls.js';
  import { isDark } from '../../lib/theme.js';
  import { loadModel, defaultMaterial } from '../../lib/viewer/models';
  import { ifcGuidAt } from '../../lib/viewer/ifc';
  import { applyStudioLook, studioEnvironment } from '../../lib/viewer/studio';
  import { refsSignature, guidsSignature } from '../../lib/viewer/refsSignature';
  import { buildHighlightOverlay, disposeHighlightOverlay } from '../../lib/viewer/highlight';
  import { fitDistance } from '../../lib/viewer/fitCamera';

  /** Models to show: [{ id, label, url, format, slot?: [x, z] }]. */
  export let refs = [];
  /** Gate wheel-zoom behind a click (for viewers embedded in scrollable pages). */
  export let wheelGate = false;
  export let height = '100%';
  /** Currently selected model id (highlighted). */
  export let selected = '';
  /**
   * IFC GlobalIds to light up *inside* the loaded model, without isolating them.
   * The highlight is a copy of those triangles drawn over the original, so the
   * element is shown in the context of the building rather than floating alone.
   */
  export let highlightGuids = [];
  /** Frame the model set after a (re)load. Hosts that manage their own camera
   *  can turn this off; a re-render never refits either way (see `sig` below). */
  export let autoFit = true;

  const dispatch = createEventDispatcher();

  let canvasEl;
  let renderer = null;
  let scene, camera, controls, raycaster, frameId, grid;
  let groupsById = new Map();
  let loadedCount = 0;
  let failedCount = 0;
  /** Models still in flight for the current refs set — drives the busy overlay. */
  let pending = 0;
  let dark = false;
  let needsRender = true; // render-on-demand flag (see the animate loop)
  const unsubTheme = isDark.subscribe((v) => {
    dark = v;
    applyTheme();
  });

  const SELECT_COLOR = new THREE.Color('#e8590c');

  function applyTheme() {
    if (!scene) return;
    scene.background = new THREE.Color(dark ? 0x10151c : 0xeef2f6);
    if (grid) {
      grid.material.opacity = dark ? 0.5 : 0.35;
    }
    for (const group of groupsById.values()) {
      group.traverse((n) => {
        if (n.isMesh && n.userData.stl) {
          // Preserve side across the re-skin (volumetric WKT meshes draw
          // DoubleSide because polyhedral ring winding is arbitrary).
          const side = n.material?.side;
          n.material = defaultMaterial(dark);
          if (side !== undefined) n.material.side = side;
        }
      });
    }
    highlight();
    needsRender = true;
  }

  // Frames remaining to ease emissive intensity toward its per-material target
  // (set by highlight()). Bounds the per-frame traversal so an idle viewer does
  // no extra work once the glow has settled.
  let emisFrames = 0;

  function highlight() {
    for (const [id, group] of groupsById) {
      const isSel = id === selected && groupsById.size > 1;
      group.traverse((node) => {
        // Highlight overlays run their own ease (applyGuidHighlight) — leaving
        // them in here would reset their target to 0 on the next selection.
        if (node.userData.isOverlay) return;
        if (node.isMesh && node.material && 'emissive' in node.material) {
          // Set the highlight colour once; the intensity eases in the render loop
          // (no per-mesh Color allocation per call, no instant snap).
          if (isSel) node.material.emissive.copy(SELECT_COLOR);
          node.material.userData.emisTarget = isSel ? 0.55 : 0;
        }
      });
    }
    emisFrames = 24; // ~0.4s of easing at the 0.18 lerp factor below
  }

  /** Direction the framing camera sits in, relative to the model centre. */
  const VIEW_DIR = new THREE.Vector3(0.7, 0.55, 1).normalize();
  /** Slack left around the framed model. 1.0 = corners exactly on the edges. */
  const FIT_PADDING = 1.06;

  /**
   * Frame the loaded model set: centre the orbit target and place the camera so
   * the whole bounding box fills the viewport.
   *
   * The fit is EXACT rather than a bounding-sphere approximation. The old code
   * fitted a sphere sized by the largest single axis, which for the usual shape
   * of a building model — a wide, shallow footprint — parked the camera far
   * enough away that the model used well under half the frame. Here each of the
   * eight box corners is projected onto the camera's own basis and the distance
   * is the smallest one that keeps every corner inside BOTH frustum planes, so
   * wide viewports and tall models are framed equally tightly.
   *
   * Runs on each refs change only, so a user's mid-session orbit is never yanked.
   */
  function fitView() {
    if (!camera || !controls || groupsById.size === 0) return;
    const box = new THREE.Box3();
    let any = false;
    for (const g of groupsById.values()) {
      // The freshly-added clones carry the cache master's matrixWorld and their
      // meshes opt out of auto-update; no render has composed them yet when the
      // post-load fit runs, so measure against explicitly composed matrices.
      g.updateMatrixWorld(true);
      const b = new THREE.Box3().setFromObject(g);
      if (b.isEmpty()) continue;
      box.union(b);
      any = true;
    }
    if (!any) return;
    const center = box.getCenter(new THREE.Vector3());
    const size = box.getSize(new THREE.Vector3());
    const radius = Math.max(size.length() * 0.5, 0.001);
    const dist = fitDistance(box, {
      fov: camera.fov,
      aspect: camera.aspect || 1,
      dir: VIEW_DIR,
      padding: FIT_PADDING,
    });

    controls.target.copy(center);
    camera.position.copy(center).addScaledVector(VIEW_DIR, dist);
    // Near/far track the MODEL, not the initial distance: they used to be derived
    // from the fit distance, so zooming in close clipped the geometry away.
    camera.near = Math.max(radius / 5000, 1e-4);
    camera.far = dist * 20 + radius * 20;
    camera.updateProjectionMatrix();
    // Keep the dolly inside a sane band — close enough to read a door handle,
    // never so far that the model becomes a dot you cannot orbit back to.
    controls.minDistance = Math.max(radius * 0.01, 1e-3);
    controls.maxDistance = dist * 8;
    controls.update();
    needsRender = true;
  }

  /** Dispose the per-instance materials of a group (the geometry is shared with
   *  the loadModel cache, so it must NOT be disposed — only owned materials are). */
  function disposeGroup(group) {
    group.traverse((n) => {
      if (!n.isMesh) return;
      const mats = Array.isArray(n.material) ? n.material : [n.material];
      for (const m of mats) {
        if (!m) continue;
        m.map?.dispose?.();
        m.dispose?.();
      }
    });
  }

  async function loadAll(sig) {
    // Clear any previous set: the modal/preview overlay reuse one live
    // instance across `refs` changes, so stale groups must not linger. Dispose the
    // evicted groups' cloned materials so a long session doesn't leak them.
    for (const group of groupsById.values()) {
      scene.remove(group);
      disposeHighlightOverlay(overlaysById.get(group.userData.elementId));
      disposeGroup(group);
    }
    overlaysById = new Map();
    groupsById = new Map();
    loadedCount = 0;
    failedCount = 0;
    const wanted = refs;
    // Big IFC/glTF payloads can take many seconds to fetch and tessellate. Say
    // so: an empty grid with no indicator reads as a broken viewer, which is a
    // large part of why loading "feels" slow.
    pending = wanted.length;
    // Load every model concurrently: each task owns its group, the counters
    // are order-independent and loadModel caches per URL, so parallelism is
    // safe and much faster than the old one-await-per-model loop.
    const tasks = wanted.map(async (ref) => {
      const group = new THREE.Group();
      group.userData.elementId = ref.id;
      const [x, z] = ref.slot || [0, 0];
      group.position.set(x, 0, z);
      scene.add(group);
      groupsById.set(ref.id, group);
      try {
        const model = (await loadModel(ref.url, ref.format, { upAxis: ref.upAxis, guids: ref.guids })).clone(true);
        if (sig !== lastSig) return; // a newer refs set superseded this load
        // clone(true) shares material instances with the loadModel cache -
        // clone materials per instance so highlight()/theming never mutates
        // the cache (other viewers, incl. the map layer, clone from it too).
        model.traverse((n) => {
          if (n.isMesh) {
            if (ref.format === 'stl') {
              n.userData.stl = true;
              n.material = defaultMaterial(dark);
            } else if (n.material?.clone) {
              n.material = n.material.clone();
            }
          }
        });
        group.add(model);
        // The overlay has to hang off the NORMALISED model, not off this slot
        // wrapper: loadModel() scales and re-centres the model it returns, and
        // subGeometryForGuids slices raw vertices out of the meshes underneath
        // it. Attached a level too high, a highlight would render at raw IFC
        // metres — tens of units next to a 1.6-unit building.
        group.userData.model = model;
        loadedCount += 1;
      } catch (err) {
        if (sig !== lastSig) return;
        failedCount += 1;
        // Name the failure — a silent wireframe cube reads as "a box that does
        // not work", with nothing to diagnose it by.
        // eslint-disable-next-line no-console
        console.warn(`[model3d] failed to load ${ref.format} ${ref.url}`, err);
        const placeholder = new THREE.Mesh(
          new THREE.BoxGeometry(1, 1, 1),
          new THREE.MeshStandardMaterial({ color: 0x9aa6b2, wireframe: true })
        );
        placeholder.position.y = 0.5;
        placeholder.userData.loadFailed = true;
        group.add(placeholder);
      }
    });
    await Promise.allSettled(tasks);
    if (sig !== lastSig) return;
    pending = 0;
    highlight();
    applyGuidHighlight(); // the groups only exist now, so re-attach the overlays
    // Frame the loaded set (multi-model layouts used to spill off-screen). This
    // only runs on a genuine content change, never on a re-render.
    if (autoFit) fitView();
  }

  // ── In-place sub-element highlight ──────────────────────────────────────────
  // A copy of `highlightGuids`' triangles, added to the same group as the source
  // meshes so it inherits the identical transform. It respects depth (see
  // lib/viewer/highlight.ts), so the element is lit where it actually sits
  // instead of being cut out of its building.
  let overlaysById = new Map();
  let lastHighlightSig = null;

  function applyGuidHighlight() {
    for (const ov of overlaysById.values()) {
      ov.parent?.remove(ov);
      disposeHighlightOverlay(ov);
    }
    overlaysById = new Map();
    if (!highlightGuids?.length) {
      needsRender = true;
      return;
    }
    const wanted = new Set(highlightGuids);
    for (const [id, group] of groupsById) {
      // `group` is the slot wrapper; the model inside it carries normalise()'s
      // scale + translation, so both the slice and the attach happen there.
      const host = group.userData.model || group;
      const ov = buildHighlightOverlay(host, wanted, false);
      if (!ov) continue;
      // The tween loop below eases every emissive toward `emisTarget`; give the
      // overlay a target so it fades in with the same feel as a model selection.
      ov.traverse((n) => {
        if (!n.isMesh || !n.material) return;
        n.material.opacity = 1;
        n.material.transparent = false;
        n.material.userData.emisTarget = 0.9;
      });
      host.add(ov);
      overlaysById.set(id, ov);
    }
    emisFrames = 24;
    needsRender = true;
  }

  function onClick(event) {
    if (!renderer || groupsById.size === 0) return;
    const rect = renderer.domElement.getBoundingClientRect();
    const pointer = new THREE.Vector2(
      ((event.clientX - rect.left) / rect.width) * 2 - 1,
      -((event.clientY - rect.top) / rect.height) * 2 + 1
    );
    raycaster.setFromCamera(pointer, camera);
    const hits = raycaster.intersectObjects([...groupsById.values()], true);
    if (!hits.length) return;
    const hit = hits[0].object;
    // IFC geometry carries its element's GlobalId — picking one selects that
    // *atom* (a beam, a slab). A merged mesh resolves it from the picked triangle.
    const guid = ifcGuidAt(hit, hits[0].faceIndex);
    let node = hit;
    while (node && !node.userData.elementId) node = node.parent;
    const id = node?.userData.elementId || null;
    if (guid || (id && groupsById.size > 1)) {
      dispatch('select', { id, guid });
    }
  }

  onMount(() => {
    scene = new THREE.Scene();
    camera = new THREE.PerspectiveCamera(50, 1, 0.01, 1000);
    camera.position.set(2.6, 2.0, 3.4);
    renderer = new THREE.WebGLRenderer({ canvas: canvasEl, antialias: true, powerPreference: 'high-performance' });
    // Cap the device-pixel-ratio: a retina DPR of 2-3 renders 4-9× the fragments
    // for no visible gain on these shaded models — the single biggest fill-rate win.
    renderer.setPixelRatio(Math.min(window.devicePixelRatio || 1, 1.75));
    raycaster = new THREE.Raycaster();

    // Studio look: image-based fill light + filmic tone mapping (see studio.ts)
    // with one directional key light for shading definition — replaces the old
    // flat ambient+sun combo that made models look like unlit plastic.
    applyStudioLook(renderer);
    scene.environment = studioEnvironment(renderer);
    const sun = new THREE.DirectionalLight(0xffffff, 1.3);
    sun.position.set(4, 8, 5);
    scene.add(sun);
    grid = new THREE.GridHelper(20, 20, 0x5a7a9a, 0x44607a);
    grid.material.transparent = true;
    scene.add(grid);
    applyTheme();

    controls = new OrbitControls(camera, renderer.domElement);
    controls.target.set(0, 0.6, 0);
    controls.enableDamping = true;
    // Dolly toward the CURSOR rather than the orbit centre. Zooming used to
    // always drive at the model's middle, so getting close to a detail off to
    // one side meant zoom-pan-zoom-pan; now the point under the pointer stays
    // put, which is what every other 3D tool does.
    controls.zoomToCursor = true;
    controls.zoomSpeed = 0.9;
    // Damped orbit + a slightly slower rotate reads as much less twitchy on a
    // trackpad, where one flick used to spin the model right past the face
    // you were aiming at.
    controls.dampingFactor = 0.08;
    controls.rotateSpeed = 0.85;
    // Embedded viewers (chat answers, inspector panels) sit in scrollable
    // pages: an always-on wheel-zoom hijacks the page scroll the moment the
    // cursor crosses the widget. Zoom arms on pointerdown inside the canvas
    // and disarms when the cursor leaves, so scrolling past a 3D answer works
    // and zooming still takes exactly one click.
    if (wheelGate) {
      controls.enableZoom = false;
      renderer.domElement.addEventListener('pointerdown', () => {
        controls.enableZoom = true;
      });
      renderer.domElement.addEventListener('pointerleave', () => {
        controls.enableZoom = false;
      });
    }

    const resize = () => {
      const w = canvasEl.clientWidth || 480;
      const h = canvasEl.clientHeight || 320;
      renderer.setSize(w, h, false);
      camera.aspect = w / h;
      camera.updateProjectionMatrix();
      needsRender = true;
    };
    resize();
    const observer = new ResizeObserver(resize);
    observer.observe(canvasEl);

    // Render on demand: a static orbit viewer rendering at 60fps forever burns GPU
    // (and battery) for nothing. controls.update() returns true while the camera is
    // moving/damping; otherwise we only draw when something actually changed
    // (selection ease, theme, (re)load, resize). rAF auto-pauses on a hidden tab.
    const animate = () => {
      frameId = requestAnimationFrame(animate);
      const moving = controls.update();
      // Ease emissive intensity toward each material's target for a few frames
      // after a selection change (set by highlight()) — no instant snap.
      if (emisFrames > 0) {
        emisFrames -= 1;
        needsRender = true;
        for (const group of groupsById.values()) {
          group.traverse((node) => {
            if (!node.isMesh || !node.material || !('emissiveIntensity' in node.material)) return;
            const target = node.material.userData?.emisTarget;
            if (target === undefined) return; // never highlighted → leave as-is
            const cur = node.material.emissiveIntensity;
            node.material.emissiveIntensity = cur + (target - cur) * 0.18;
          });
        }
      }
      if (moving || needsRender) {
        renderer.render(scene, camera);
        needsRender = false;
      }
    };
    animate();

    return () => observer.disconnect();
  });

  onDestroy(() => {
    unsubTheme();
    if (frameId) cancelAnimationFrame(frameId);
    // Free the cloned materials + grid; the shared cached geometry is left for the
    // model cache to own. Without this a session that opens many panels leaks GPU
    // resources until the browser drops the WebGL context (models turn black).
    for (const ov of overlaysById.values()) disposeHighlightOverlay(ov);
    overlaysById = new Map();
    for (const group of groupsById.values()) disposeGroup(group);
    groupsById = new Map();
    if (grid) {
      grid.geometry?.dispose?.();
      grid.material?.dispose?.();
    }
    if (renderer) {
      renderer.dispose();
      // dispose() frees three's own GPU objects but leaves the WebGL context
      // itself alive until the browser garbage-collects the canvas. Inspector
      // windows can now be minimised (which unmounts this component) and
      // restored, so a session churns through viewers — and a browser only
      // grants ~16 contexts before it starts dropping the OLDEST one, which is
      // how a still-open model turns black. Hand the context back explicitly.
      try {
        renderer.forceContextLoss();
      } catch {
        /* WEBGL_lose_context unavailable — nothing else to do */
      }
    }
    renderer = null;
  });

  $: if (scene && selected !== undefined) highlight();
  // Reload when the refs set changes CONTENT (modal navigation, next preview).
  // Identity is useless here: hosts pass an inline array literal, which Svelte
  // re-derives on every reactive pass, so an identity check turned any click in
  // the page into a full teardown + reload + camera refit. See refsSignature.ts.
  let lastSig = null;
  $: sig = refsSignature(refs);
  $: if (scene && sig !== lastSig) {
    lastSig = sig;
    loadAll(sig);
  }
  $: highlightSig = guidsSignature(highlightGuids);
  $: if (scene && highlightSig !== lastHighlightSig) {
    lastHighlightSig = highlightSig;
    applyGuidHighlight();
  }
</script>

<div class="model-3d" style:height>
  <canvas bind:this={canvasEl} on:click={onClick} aria-label="3D model viewer"></canvas>
  {#if refs.length === 0}
    <div class="overlay">{$i18nT('viewer.noModels')}</div>
  {:else if pending > 0}
    <div class="loading-veil" role="status" aria-live="polite">
      <span class="spinner" aria-hidden="true"></span>
      <span>
        {#if refs.length > 1}
          {$i18nT('viewer.loadingModelsProgress', { values: { done: loadedCount + failedCount, total: refs.length } })}
        {:else}
          {$i18nT('viewer.loadingModel')}
        {/if}
      </span>
    </div>
  {:else if failedCount > 0}
    <div class="overlay subtle">{loadedCount}/{refs.length}</div>
  {/if}
</div>

<style>
  .model-3d {
    position: relative;
    width: 100%;
    min-height: 180px;
    border-radius: 10px;
    overflow: hidden;
    background: var(--bg-soft, #eef2f6);
  }
  canvas {
    width: 100%;
    height: 100%;
    display: block;
    cursor: grab;
  }
  canvas:active {
    cursor: grabbing;
  }
  .overlay {
    position: absolute;
    inset: auto 8px 8px auto;
    padding: 3px 9px;
    border-radius: 6px;
    background: rgba(0, 0, 0, 0.55);
    color: #dbe4ee;
    font-size: 0.75rem;
    pointer-events: none;
  }
  .overlay.subtle {
    opacity: 0.8;
  }
  /* Busy state while models fetch + tessellate. Centred and unmissable: the
     point is that the viewer is working, not broken. */
  .loading-veil {
    position: absolute;
    inset: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 0.55rem;
    pointer-events: none;
    color: var(--ink-500, #64748b);
    font-size: 0.82rem;
  }
  .spinner {
    width: 16px;
    height: 16px;
    border-radius: 50%;
    border: 2px solid currentColor;
    border-top-color: transparent;
    animation: m3d-spin 0.7s linear infinite;
  }
  @keyframes m3d-spin {
    to {
      transform: rotate(360deg);
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .spinner {
      animation-duration: 2.4s;
    }
  }
</style>
