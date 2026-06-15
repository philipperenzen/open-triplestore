<script>
  // First-person "walk through the building" viewer. Loads the IFC model at real
  // metre scale into a standalone three.js scene and lets you walk through it:
  // mouse-look (pointer lock) + WASD/arrows to move, Space/Q-E for up/down, Shift
  // to sprint. A centre crosshair names the wall/door/furniture you're looking at;
  // click it to inspect. WebXR/VR is supported where the browser + headset allow.
  import { onMount, onDestroy, createEventDispatcher } from 'svelte';
  import { t as i18nT } from 'svelte-i18n';
  import * as THREE from 'three';
  import { PointerLockControls } from 'three/addons/controls/PointerLockControls.js';
  import { VRButton } from 'three/addons/webxr/VRButton.js';
  import { X, Footprints, MousePointerClick } from 'lucide-svelte';
  import { isDark } from '../../lib/theme.js';
  import { loadModel, realWorldMeters, NORMALISED_DIM } from '../../lib/viewer/models';
  import { ifcGuidAt } from '../../lib/viewer/ifc';
  import { shortenIRI } from '../../lib/rdf-utils.js';

  /** Whole-building model to walk through. */
  export let url;
  export let format = 'ifc';
  export let upAxis = null;
  export let label = '';
  /** Viewer-feed elements, so a picked mesh's GlobalId resolves to its info. */
  export let elements = [];

  const dispatch = createEventDispatcher();

  let wrapEl, canvasEl, vrSlot;
  let renderer, scene, camera, controls, raycaster, model = null, grid = null;
  let loading = true;
  let error = '';
  let locked = false;
  let hoverLabel = '';
  let picked = null; // { label, type, guid, id }
  let wtNeedsRender = true; // draw a frame while paused (render-on-demand)

  const move = { f: false, b: false, l: false, r: false, up: false, down: false, fast: false };
  let prevT = 0;
  let lastPickT = 0; // throttle the crosshair raycast against the merged building
  const CENTER = new THREE.Vector2(0, 0);

  // mesh GlobalId → feed element (for the crosshair label + click-to-inspect).
  $: guidToEl = (() => {
    const m = new Map();
    for (const e of elements) if (e.ifc_guid) m.set(e.ifc_guid, e);
    return m;
  })();

  function shortType(types) {
    return (types || []).map((t) => shortenIRI(t)).find((s) => /Ifc|Element|Space|Storey|Wall|Slab/i.test(s)) || (types?.[0] ? shortenIRI(types[0]) : '');
  }

  /** Raycast straight ahead (crosshair) → the element under the reticle. The
   *  merged building resolves the GlobalId from the picked triangle (faceIndex). */
  function pickAhead() {
    if (!model || !raycaster) return null;
    raycaster.setFromCamera(CENTER, camera);
    const hits = raycaster.intersectObject(model, true);
    for (const h of hits) {
      const g = ifcGuidAt(h.object, h.faceIndex);
      if (g) return { guid: g, el: guidToEl.get(g) || null };
    }
    return null;
  }

  function onKey(e, down) {
    let hit = true;
    switch (e.code) {
      case 'KeyW': case 'ArrowUp': move.f = down; break;
      case 'KeyS': case 'ArrowDown': move.b = down; break;
      case 'KeyA': case 'ArrowLeft': move.l = down; break;
      case 'KeyD': case 'ArrowRight': move.r = down; break;
      case 'Space': case 'KeyE': move.up = down; break;
      case 'KeyQ': move.down = down; break;
      case 'ShiftLeft': case 'ShiftRight': move.fast = down; break;
      default: hit = false;
    }
    if (hit) e.preventDefault();
  }
  const kd = (e) => onKey(e, true);
  const ku = (e) => onKey(e, false);

  function onCanvasClick() {
    if (!controls) return;
    if (!locked) {
      controls.lock();
      return;
    }
    const p = pickAhead();
    if (p?.el) {
      picked = { label: p.el.label || shortenIRI(p.el.id), type: shortType(p.el.types), guid: p.guid, id: p.el.id };
    }
  }

  onMount(() => {
    let ro;
    (async () => {
      scene = new THREE.Scene();
      scene.background = new THREE.Color(0x0c121c);
      scene.fog = new THREE.Fog(0x0c121c, 60, 240);
      camera = new THREE.PerspectiveCamera(75, 1, 0.03, 3000);
      renderer = new THREE.WebGLRenderer({ canvas: canvasEl, antialias: true, powerPreference: 'high-performance' });
      // Cap DPR — a retina building walkthrough is heavily fill-rate bound.
      renderer.setPixelRatio(Math.min(window.devicePixelRatio || 1, 1.75));
      renderer.xr.enabled = true;
      raycaster = new THREE.Raycaster();

      scene.add(new THREE.HemisphereLight(0xffffff, 0x3a4658, 1.5));
      const sun = new THREE.DirectionalLight(0xffffff, 2.1);
      sun.position.set(18, 40, 12);
      scene.add(sun);
      grid = new THREE.GridHelper(500, 100, 0x2c3e54, 0x1a2533);
      grid.material.transparent = true;
      grid.material.opacity = 0.5;
      scene.add(grid);

      controls = new PointerLockControls(camera, renderer.domElement);
      scene.add(camera);
      controls.addEventListener('lock', () => (locked = true));
      controls.addEventListener('unlock', () => {
        locked = false;
        wtNeedsRender = true; // draw the final paused frame once
      });

      try {
        const cached = await loadModel(url, format, { upAxis });
        model = cached.clone(true);
        // Undo loadModel's normalisation (largest dim = NORMALISED_DIM) back to real
        // metres so eye height, walk speed and the building all share one scale.
        const k = realWorldMeters(cached, 12) / NORMALISED_DIM;
        model.scale.multiplyScalar(k);
        model.traverse((n) => {
          if (n.isMesh && n.material) {
            const mats = Array.isArray(n.material) ? n.material : [n.material];
            // Per-instance clone + double-sided so interior wall faces are visible
            // when you walk inside (IFC faces are often single-sided outward).
            n.material = Array.isArray(n.material)
              ? mats.map((m) => { const c = m.clone(); c.side = THREE.DoubleSide; return c; })
              : (() => { const c = mats[0].clone(); c.side = THREE.DoubleSide; return c; })();
          }
        });
        scene.add(model);
        // Start at eye height just outside the building, facing in.
        const box = new THREE.Box3().setFromObject(model);
        const size = box.getSize(new THREE.Vector3());
        const c = box.getCenter(new THREE.Vector3());
        const eye = box.min.y + 1.6;
        camera.position.set(c.x, eye, box.max.z + Math.max(3, size.z * 0.35));
        camera.lookAt(c.x, eye, c.z);
      } catch (e) {
        error = e?.message || 'Failed to load the building model.';
      }
      loading = false;

      const resize = () => {
        const w = wrapEl?.clientWidth || 1;
        const h = wrapEl?.clientHeight || 1;
        renderer.setSize(w, h, false);
        camera.aspect = w / h;
        camera.updateProjectionMatrix();
        wtNeedsRender = true;
      };
      resize();
      ro = new ResizeObserver(resize);
      ro.observe(wrapEl);

      // VR entry button (no-ops gracefully where WebXR/headset is unavailable).
      try {
        vrSlot?.appendChild(VRButton.createButton(renderer));
      } catch {
        /* WebXR unsupported — desktop walkthrough still works */
      }

      window.addEventListener('keydown', kd);
      window.addEventListener('keyup', ku);

      prevT = performance.now();
      wtNeedsRender = true;
      renderer.setAnimationLoop(() => {
        const t = performance.now();
        const dt = Math.min(0.1, (t - prevT) / 1000);
        prevT = t;
        const active = locked || renderer.xr.isPresenting;
        if (active) {
          const v = (move.fast ? 9 : 3.4) * dt;
          if (move.f) controls.moveForward(v);
          if (move.b) controls.moveForward(-v);
          if (move.r) controls.moveRight(v);
          if (move.l) controls.moveRight(-v);
          if (move.up) camera.position.y += v;
          if (move.down) camera.position.y -= v;
          // Throttle the crosshair pick: it raycasts the MERGED building (whose
          // few meshes each span the whole model), so casting it every frame would
          // triangle-test the entire building 60×/s. ~8/s keeps the reticle label
          // responsive without the per-frame cost.
          if (t - lastPickT > 120) {
            lastPickT = t;
            const p = pickAhead();
            hoverLabel = p ? p.el?.label || shortenIRI(p.guid) : '';
          }
        }
        // Render every frame while walking/VR; when paused (pointer unlocked) the
        // scene is static, so draw just once instead of burning the GPU at 60fps.
        if (active || wtNeedsRender) {
          renderer.render(scene, camera);
          wtNeedsRender = false;
        }
      });
    })();

    return () => {
      ro?.disconnect();
      window.removeEventListener('keydown', kd);
      window.removeEventListener('keyup', ku);
    };
  });

  onDestroy(() => {
    renderer?.setAnimationLoop(null);
    try { controls?.unlock(); } catch { /* noop */ }
    // Free the cloned (per-instance, double-sided) materials + grid; the model's
    // BufferGeometry is shared with the loadModel cache, so it is left intact.
    model?.traverse((n) => {
      if (!n.isMesh) return;
      const mats = Array.isArray(n.material) ? n.material : [n.material];
      for (const m of mats) {
        m?.map?.dispose?.();
        m?.dispose?.();
      }
    });
    grid?.geometry?.dispose?.();
    grid?.material?.dispose?.();
    renderer?.dispose();
  });

  $: dark = $isDark;
</script>

<div class="walk" bind:this={wrapEl} class:dark>
  <canvas bind:this={canvasEl} on:click={onCanvasClick} aria-label="3D walkthrough"></canvas>

  <!-- Centre reticle + what it's aimed at -->
  {#if !loading && !error}
    <div class="reticle" class:hot={!!hoverLabel}></div>
    {#if hoverLabel && locked}
      <div class="reticle-label">{hoverLabel}</div>
    {/if}
  {/if}

  <header class="wt-head">
    <span class="wt-title"><Footprints size={15} /> {$i18nT('viewer.walkthrough')}{label ? ` · ${label}` : ''}</span>
    <span class="vr-slot" bind:this={vrSlot}></span>
    <button class="wt-close" on:click={() => dispatch('close')} aria-label={$i18nT('viewer.close')}><X size={18} /></button>
  </header>

  {#if loading}
    <div class="wt-overlay"><div class="spin"></div><p>{$i18nT('viewer.walkLoading')}</p></div>
  {:else if error}
    <div class="wt-overlay"><p class="err">{error}</p></div>
  {:else if !locked}
    <button class="wt-enter" on:click={() => controls?.lock()}>
      <MousePointerClick size={18} /> {$i18nT('viewer.walkClickToEnter')}
      <small>{$i18nT('viewer.walkControls')}</small>
    </button>
  {/if}

  <!-- Picked-element info card -->
  {#if picked}
    <div class="wt-info">
      <div class="wt-info-head">
        <strong>{picked.label}</strong>
        <button on:click={() => (picked = null)} aria-label={$i18nT('viewer.close')}><X size={14} /></button>
      </div>
      {#if picked.type}<div class="wt-row"><span>{$i18nT('viewer.type')}</span><code>{picked.type}</code></div>{/if}
      <div class="wt-row"><span>IFC GlobalId</span><code>{picked.guid}</code></div>
      <button class="wt-details" on:click={() => dispatch('inspect', { id: picked.id, guid: picked.guid })}>
        {$i18nT('viewer.fullDetails')}
      </button>
    </div>
  {/if}
</div>

<style>
  .walk {
    position: fixed;
    inset: 0;
    z-index: 1400;
    background: #0c121c;
  }
  canvas {
    width: 100%;
    height: 100%;
    display: block;
    cursor: crosshair;
  }
  .wt-head {
    position: absolute;
    top: 0;
    left: 0;
    right: 0;
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 10px 14px;
    background: linear-gradient(180deg, rgba(8, 13, 22, 0.8), transparent);
    color: #dbe6f2;
    pointer-events: none;
  }
  .wt-title {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-weight: 600;
    font-size: 0.9rem;
  }
  .vr-slot {
    margin-left: auto;
    pointer-events: auto;
  }
  /* Re-style the three.js VRButton injected into the slot. */
  .vr-slot :global(button) {
    position: static !important;
    transform: none !important;
    margin: 0 !important;
    width: auto !important;
    padding: 6px 12px !important;
    border-radius: 8px !important;
    font-size: 0.78rem !important;
    background: rgba(20, 30, 44, 0.9) !important;
    border: 1px solid rgba(255, 255, 255, 0.18) !important;
  }
  .wt-close {
    pointer-events: auto;
    border: 0;
    background: rgba(20, 30, 44, 0.9);
    color: #dbe6f2;
    border-radius: 8px;
    width: 32px;
    height: 32px;
    display: flex;
    align-items: center;
    justify-content: center;
    cursor: pointer;
  }
  .wt-close:hover {
    background: rgba(232, 89, 12, 0.85);
  }
  .reticle {
    position: absolute;
    left: 50%;
    top: 50%;
    width: 8px;
    height: 8px;
    margin: -4px 0 0 -4px;
    border: 1.5px solid rgba(255, 255, 255, 0.7);
    border-radius: 50%;
    pointer-events: none;
    box-shadow: 0 0 0 1px rgba(0, 0, 0, 0.4);
  }
  .reticle.hot {
    border-color: #ff8a2a;
    background: rgba(255, 138, 42, 0.35);
  }
  .reticle-label {
    position: absolute;
    left: 50%;
    top: calc(50% + 14px);
    transform: translateX(-50%);
    background: rgba(8, 13, 22, 0.85);
    color: #fff;
    padding: 3px 9px;
    border-radius: 6px;
    font-size: 0.78rem;
    white-space: nowrap;
    pointer-events: none;
    max-width: 60vw;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .wt-overlay {
    position: absolute;
    inset: 0;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 12px;
    color: #c7d4e2;
  }
  .err {
    color: #ff8a6a;
  }
  .spin {
    width: 34px;
    height: 34px;
    border: 3px solid rgba(255, 255, 255, 0.2);
    border-top-color: #ff8a2a;
    border-radius: 50%;
    animation: sp 0.9s linear infinite;
  }
  @keyframes sp {
    to {
      transform: rotate(360deg);
    }
  }
  .wt-enter {
    position: absolute;
    left: 50%;
    top: 50%;
    transform: translate(-50%, -50%);
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 4px;
    padding: 16px 26px;
    border-radius: 12px;
    border: 1px solid rgba(255, 255, 255, 0.18);
    background: rgba(16, 24, 36, 0.86);
    color: #eef4fb;
    font-size: 1rem;
    font-weight: 600;
    cursor: pointer;
    backdrop-filter: blur(6px);
  }
  .wt-enter small {
    font-weight: 400;
    font-size: 0.74rem;
    color: #9fb2c6;
  }
  .wt-enter:hover {
    border-color: #ff8a2a;
  }
  .wt-info {
    position: absolute;
    left: 16px;
    bottom: 16px;
    width: min(320px, 80vw);
    background: rgba(13, 20, 31, 0.92);
    border: 1px solid rgba(255, 255, 255, 0.12);
    border-radius: 10px;
    padding: 10px 12px;
    color: #dbe6f2;
    backdrop-filter: blur(8px);
  }
  .wt-info-head {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 8px;
    margin-bottom: 6px;
  }
  .wt-info-head strong {
    font-size: 0.9rem;
    line-height: 1.2;
  }
  .wt-info-head button {
    border: 0;
    background: transparent;
    color: #9fb2c6;
    cursor: pointer;
    flex: none;
  }
  .wt-row {
    display: flex;
    gap: 8px;
    font-size: 0.76rem;
    padding: 2px 0;
  }
  .wt-row span {
    color: #8ba0b6;
    flex: none;
    min-width: 84px;
  }
  .wt-row code {
    color: #cfe0f0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .wt-details {
    margin-top: 8px;
    width: 100%;
    padding: 6px;
    border-radius: 7px;
    border: 0;
    background: var(--brand-600, #2563a8);
    color: #fff;
    font-size: 0.8rem;
    cursor: pointer;
  }
  .wt-details:hover {
    background: #e8590c;
  }
  /* Visible keyboard focus over the dark immersive scene (dual ring). */
  .wt-close:focus-visible,
  .wt-enter:focus-visible,
  .wt-details:focus-visible {
    outline: none;
    box-shadow: 0 0 0 2px #0c121c, 0 0 0 4px #ff8a2a;
  }
  @media (prefers-reduced-motion: reduce) {
    .spin {
      animation: none;
    }
    .reticle {
      transition: none;
    }
  }
</style>
