<script context="module">
  // Camera pose per model URL, so leaving the walkthrough (to inspect an
  // element's data, or closing it by accident) and coming back RESUMES exactly
  // where you stood instead of restarting outside the front door. Module-level:
  // survives component unmount for the whole SPA session.
  const wtPoseByUrl = new Map(); // url → { pos:[x,y,z], quat:[x,y,z,w], mode }
</script>

<script>
  // First-person "walk through the building" viewer. Loads the IFC model at real
  // metre scale into a standalone three.js scene and lets you walk through it:
  // mouse-look (pointer lock) + WASD/arrows to move, Space/Q-E for up/down, Shift
  // to sprint. A centre crosshair names the wall/door/furniture you're looking at;
  // click it to select + inspect (persistent edge/fill highlight, X toggles
  // x-ray). WebXR/VR is supported where the browser + headset allow.
  import { onMount, onDestroy, createEventDispatcher } from 'svelte';
  import { t as i18nT } from 'svelte-i18n';
  import * as THREE from 'three';
  import { PointerLockControls } from 'three/addons/controls/PointerLockControls.js';
  import { VRButton } from 'three/addons/webxr/VRButton.js';
  import { X, Footprints, Mouse, MousePointerClick } from 'lucide-svelte';
  import { isDark } from '../../lib/theme.js';
  import { loadModel, realWorldMeters, NORMALISED_DIM } from '../../lib/viewer/models';
  import { ifcGuidAt, ifcProgress } from '../../lib/viewer/ifc';
  import { buildHighlightOverlay, disposeHighlightOverlay, HL_EMISSIVE } from '../../lib/viewer/highlight';
  import { applyStudioLook, studioEnvironment } from '../../lib/viewer/studio';
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
  // Pointer lock is best-effort: browsers refuse it in plenty of real
  // situations (the ~1.3 s cooldown after Esc, iframes without the permission,
  // kiosk/headless setups). When the request errors, the walkthrough falls back
  // to DRAG-look — hold the left button and move the mouse to look around —
  // instead of a dead "Click to walk in" button that appears to do nothing.
  let dragLook = false;
  let dragging = false;
  let dragMoved = 0; // px travelled while dragging (a real drag isn't a pick)
  const DRAG_EULER = new THREE.Euler(0, 0, 0, 'YXZ');
  let hoverLabel = '';
  let picked = null; // { label, type, guid, id }
  let wtNeedsRender = true; // draw a frame while paused (render-on-demand)

  // Movement mode: 'walk' = first-person, bound by gravity to the floors/stairs
  // (Space jumps, Ctrl crouches) — the natural way to inspect an interior;
  // 'fly' = free/creative "god" mode (Space/E up, Ctrl/Q/C down). Toggle with
  // the header buttons or `F`.
  let mode = 'walk';
  let vy = 0; // vertical velocity (m/s) in walk mode
  let grounded = false;
  let groundRay = null; // downward raycaster for the floor under the camera
  const EYE_HEIGHT = 1.7; // metres — camera height above the floor when standing
  const EYE_CROUCH = 1.05; // metres — eye height while Ctrl is held in walk mode
  let eyeNow = EYE_HEIGHT; // eased toward the target so crouching doesn't snap
  const GRAVITY = 18; // m/s²
  const JUMP_SPEED = 4.6; // m/s
  const DOWN = new THREE.Vector3(0, -1, 0);
  // Highest rise the walk mode steps onto. Anything taller under your feet is a
  // wall / table / parapet — WITHOUT this limit the floor-follow snapped the eye
  // onto any surface below, so brushing a wall top ratcheted you storey by
  // storey until you were walking on the roof.
  const MAX_STEP = 0.5;

  // ── Selection highlight + x-ray ─────────────────────────────────────────────
  let selOverlay = null; // persistent copy of the picked element's triangles
  let xrayOn = false; // ghost the rest of the building around the selection

  /** Ghost (or restore) every non-overlay mesh so the selected element reads
   *  solid through the building. Original material state is stashed once. */
  function applyGhost(on) {
    if (!model) return;
    model.traverse((n) => {
      if (!n.isMesh || !n.material || n.userData.isOverlay) return;
      const mats = Array.isArray(n.material) ? n.material : [n.material];
      for (const m of mats) {
        if (m.userData.wtOrig === undefined) {
          m.userData.wtOrig = { transparent: m.transparent, opacity: m.opacity, depthWrite: m.depthWrite };
        }
        if (on) {
          m.transparent = true;
          m.opacity = 0.18;
          m.depthWrite = false;
        } else {
          m.transparent = m.userData.wtOrig.transparent;
          m.opacity = m.userData.wtOrig.opacity;
          m.depthWrite = m.userData.wtOrig.depthWrite;
        }
      }
    });
  }

  /** (Re)build the persistent highlight for `guid` — orange fill + always-visible
   *  edge outline; in x-ray the fill renders through the ghosted building. */
  function applySelection(guid) {
    if (selOverlay) {
      selOverlay.parent?.remove(selOverlay);
      disposeHighlightOverlay(selOverlay);
      selOverlay = null;
    }
    const ghost = xrayOn && !!guid;
    applyGhost(ghost);
    if (guid && model) {
      const ov = buildHighlightOverlay(model, new Set([guid]), ghost);
      if (ov) {
        // No tween loop here — settle the fill to its final look immediately.
        ov.traverse((n) => {
          if (!n.isMesh || !n.material || n.userData.isOverlayEdges) return;
          n.material.opacity = 1;
          n.material.transparent = false;
          if ('emissiveIntensity' in n.material) n.material.emissiveIntensity = HL_EMISSIVE;
        });
        model.add(ov);
        selOverlay = ov;
      }
    }
    wtNeedsRender = true;
  }

  function toggleXray() {
    xrayOn = !xrayOn;
    applySelection(picked?.guid || null);
  }

  const move = { f: false, b: false, l: false, r: false, up: false, down: false, fast: false, crouch: false };
  let prevT = 0;
  let lastPickT = 0; // throttle the crosshair raycast against the merged building
  const CENTER = new THREE.Vector2(0, 0);

  function setMode(m) {
    if (m === mode) return;
    mode = m;
    vy = 0;
    wtNeedsRender = true;
  }

  /** World-space Y of the floor directly under `pos` (−∞ when nothing is below —
   *  e.g. standing outside the building), via a BVH-accelerated downward ray. */
  function floorUnder(pos) {
    if (!model || !groundRay) return -Infinity;
    groundRay.set(pos, DOWN);
    const hit = groundRay.intersectObject(model, true)[0];
    return hit ? hit.point.y : -Infinity;
  }

  /** Walk-mode vertical: gravity pulls the eye to `floor + EYE_HEIGHT`; over open
   *  space (no floor below) the camera hovers so you can walk onto a floor.
   *
   *  Step limit: a surface more than MAX_STEP above the feet (a wall top, a
   *  table, a parapet you brushed against) is NOT a floor — the old
   *  snap-onto-anything popped the eye onto it, and from there the next "floor"
   *  was the ceiling, ratcheting you storey by storey onto the roof. Such an
   *  obstacle now just holds the current level (pass-through stays possible);
   *  stairs (risers ≪ MAX_STEP) and deliberate jumps still land normally. */
  function applyWalkGravity(dt) {
    // Ease the eye height toward standing/crouched (Ctrl) so it doesn't snap.
    const eyeTargetH = move.crouch ? EYE_CROUCH : EYE_HEIGHT;
    eyeNow += (eyeTargetH - eyeNow) * Math.min(1, 12 * dt);
    const floorY = floorUnder(camera.position);
    if (!Number.isFinite(floorY)) {
      vy = 0;
      grounded = false;
      return;
    }
    const rise = floorY - (camera.position.y - eyeNow);
    if (rise > MAX_STEP) {
      vy = 0;
      grounded = false;
      return; // obstacle underfoot — glide at the current level
    }
    vy -= GRAVITY * dt;
    camera.position.y += vy * dt;
    const eyeTarget = floorY + eyeNow;
    if (camera.position.y <= eyeTarget || (grounded && move.crouch)) {
      camera.position.y = eyeTarget;
      vy = 0;
      grounded = true;
    } else {
      grounded = false;
    }
  }

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
      case 'Space':
        // Walk: jump (once, when grounded). Fly: hold to ascend.
        if (mode === 'walk') {
          if (down && grounded) {
            vy = JUMP_SPEED;
            grounded = false;
          }
        } else {
          move.up = down;
        }
        break;
      case 'KeyE': move.up = down; break;
      // Descend keys: Q (existing) plus C — reachable with the left hand on
      // WASD, and layout-independent enough for AZERTY users where Q sits under A.
      case 'KeyQ': case 'KeyC': move.down = down; break;
      // Ctrl: crouch in walk mode, descend in fly mode (the FPS convention).
      case 'ControlLeft': case 'ControlRight':
        move.crouch = down;
        move.down = down;
        break;
      case 'ShiftLeft': case 'ShiftRight': move.fast = down; break;
      case 'KeyF': // toggle walk ↔ fly
        if (down) setMode(mode === 'walk' ? 'fly' : 'walk');
        break;
      case 'KeyX': // toggle x-ray around the selected element
        if (down) toggleXray();
        break;
      case 'Escape':
        // Native pointer lock pauses via the browser's own Esc; the drag-look
        // fallback has no such hook, so pause it here.
        if (down && dragLook && locked) {
          locked = false;
          wtNeedsRender = true;
        }
        break;
      default: hit = false;
    }
    if (hit) e.preventDefault();
  }
  const kd = (e) => onKey(e, true);
  const ku = (e) => onKey(e, false);

  /** Enter the first-person view: real pointer lock when the browser grants it,
   *  the drag-look fallback when it doesn't (see `dragLook`). */
  function enterWalk() {
    if (!controls) return;
    if (dragLook) {
      locked = true;
      wtNeedsRender = true;
      return;
    }
    try {
      controls.lock();
    } catch {
      enableDragFallback();
    }
  }

  function enableDragFallback() {
    dragLook = true;
    locked = true;
    wtNeedsRender = true;
  }

  // Drag-to-look handlers (fallback mode only): hold the left button and move.
  function onPointerDown(e) {
    if (!dragLook || !locked || e.button !== 0) return;
    dragging = true;
    dragMoved = 0;
    canvasEl?.setPointerCapture?.(e.pointerId);
  }
  function onPointerMove(e) {
    if (!dragging || !camera) return;
    const dx = e.movementX ?? 0;
    const dy = e.movementY ?? 0;
    dragMoved += Math.abs(dx) + Math.abs(dy);
    DRAG_EULER.setFromQuaternion(camera.quaternion);
    DRAG_EULER.y -= dx * 0.0025;
    DRAG_EULER.x -= dy * 0.0025;
    DRAG_EULER.x = Math.max(-1.55, Math.min(1.55, DRAG_EULER.x));
    camera.quaternion.setFromEuler(DRAG_EULER);
    wtNeedsRender = true;
  }
  function onPointerUp() {
    dragging = false;
  }

  function onCanvasClick() {
    if (!controls) return;
    if (!locked) {
      enterWalk();
      return;
    }
    // A drag that just ended is looking around, not a pick.
    if (dragLook && dragMoved > 5) {
      dragMoved = 0;
      return;
    }
    const p = pickAhead();
    if (p?.el) {
      picked = { label: p.el.label || shortenIRI(p.el.id), type: shortType(p.el.types), guid: p.guid, id: p.el.id };
      // Persistent highlight (edge outline + fill) until deselected — the brief
      // reticle flash alone never told you WHAT was selected.
      applySelection(p.guid);
    }
  }

  function clearPicked() {
    picked = null;
    applySelection(null);
  }

  onMount(() => {
    let ro;
    (async () => {
      // Daylight scene: an architectural walkthrough at "midnight" (the old
      // near-black background) read as unnatural and left interiors murky. A
      // soft overcast sky + the shared studio environment lights rooms evenly.
      scene = new THREE.Scene();
      scene.background = new THREE.Color(0xdce6f0);
      scene.fog = new THREE.Fog(0xdce6f0, 120, 500);
      camera = new THREE.PerspectiveCamera(75, 1, 0.03, 3000);
      renderer = new THREE.WebGLRenderer({ canvas: canvasEl, antialias: true, powerPreference: 'high-performance' });
      // Cap DPR — a retina building walkthrough is heavily fill-rate bound.
      renderer.setPixelRatio(Math.min(window.devicePixelRatio || 1, 1.75));
      renderer.xr.enabled = true;
      raycaster = new THREE.Raycaster();
      // Dedicated downward ray for the walk-mode floor follow — firstHitOnly keeps
      // it O(log n) against the merged building's BVH.
      groundRay = new THREE.Raycaster();
      groundRay.firstHitOnly = true;

      applyStudioLook(renderer);
      scene.environment = studioEnvironment(renderer);
      scene.add(new THREE.HemisphereLight(0xffffff, 0x9aa4b0, 0.5));
      const sun = new THREE.DirectionalLight(0xfff4e0, 1.6);
      sun.position.set(18, 40, 12);
      scene.add(sun);
      grid = new THREE.GridHelper(500, 100, 0x8fa0b2, 0xaebbc9);
      grid.material.transparent = true;
      grid.material.opacity = 0.45;
      scene.add(grid);

      controls = new PointerLockControls(camera, renderer.domElement);
      scene.add(camera);
      controls.addEventListener('lock', () => (locked = true));
      controls.addEventListener('unlock', () => {
        locked = false;
        wtNeedsRender = true; // draw the final paused frame once
      });
      // A refused pointer lock (Esc-cooldown, iframe policy, kiosk browsers)
      // used to leave a dead "Click to walk in" button — fall back to drag-look.
      document.addEventListener('pointerlockerror', enableDragFallback);

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
        // Resume the previous session's pose for this model (leaving to inspect
        // an element's data no longer restarts you outside the front door); a
        // first visit starts at eye height just outside the building, facing in.
        const saved = wtPoseByUrl.get(url);
        if (saved) {
          camera.position.fromArray(saved.pos);
          camera.quaternion.fromArray(saved.quat);
          mode = saved.mode || mode;
        } else {
          const box = new THREE.Box3().setFromObject(model);
          const size = box.getSize(new THREE.Vector3());
          const c = box.getCenter(new THREE.Vector3());
          const eye = box.min.y + 1.6;
          camera.position.set(c.x, eye, box.max.z + Math.max(3, size.z * 0.35));
          camera.lookAt(c.x, eye, c.z);
        }
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
          // moveForward/moveRight travel on the horizontal plane regardless of
          // look pitch, so both modes stay level; only the vertical axis differs.
          if (move.f) controls.moveForward(v);
          if (move.b) controls.moveForward(-v);
          if (move.r) controls.moveRight(v);
          if (move.l) controls.moveRight(-v);
          if (mode === 'fly') {
            if (move.up) camera.position.y += v;
            if (move.down) camera.position.y -= v;
          } else {
            applyWalkGravity(dt); // first-person: gravity + floor/stair follow
          }
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
      document.removeEventListener('pointerlockerror', enableDragFallback);
    };
  });

  onDestroy(() => {
    // Remember where the user stood so re-opening this model resumes in place.
    if (camera && model) {
      wtPoseByUrl.set(url, {
        pos: camera.position.toArray(),
        quat: camera.quaternion.toArray(),
        mode,
      });
    }
    if (selOverlay) {
      selOverlay.parent?.remove(selOverlay);
      disposeHighlightOverlay(selOverlay);
      selOverlay = null;
    }
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
  <canvas
    bind:this={canvasEl}
    on:click={onCanvasClick}
    on:pointerdown={onPointerDown}
    on:pointermove={onPointerMove}
    on:pointerup={onPointerUp}
    on:pointercancel={onPointerUp}
    aria-label="3D walkthrough"
  ></canvas>

  <!-- Centre reticle + what it's aimed at -->
  {#if !loading && !error}
    <div class="reticle" class:hot={!!hoverLabel}></div>
    {#if hoverLabel && locked}
      <div class="reticle-label">{hoverLabel}</div>
    {/if}
    {#if locked}
      <!-- Always-visible control hints; Esc is the one nobody guesses. -->
      <div class="wt-hints">
        {mode === 'walk' ? $i18nT('viewer.walkHintsWalk') : $i18nT('viewer.walkHintsFly')}
      </div>
    {/if}
  {/if}

  <header class="wt-head">
    <span class="wt-title"><Footprints size={15} /> {$i18nT('viewer.walkthrough')}{label ? ` · ${label}` : ''}</span>
    <div class="wt-mode" role="group" aria-label={$i18nT('viewer.walkthrough')}>
      <button
        class:active={mode === 'walk'}
        on:click={() => setMode('walk')}
        title={$i18nT('viewer.walkModeTitle')}
      >{$i18nT('viewer.walkMode')}</button>
      <button
        class:active={mode === 'fly'}
        on:click={() => setMode('fly')}
        title={$i18nT('viewer.flyModeTitle')}
      >{$i18nT('viewer.flyMode')}</button>
    </div>
    <div class="wt-mode" role="group" aria-label={$i18nT('viewer.layerXray')}>
      <button
        class:active={xrayOn}
        disabled={!picked}
        on:click={toggleXray}
        title={$i18nT('viewer.walkXrayTitle')}
      >{$i18nT('viewer.layerXray')}</button>
    </div>
    <span class="vr-slot" bind:this={vrSlot}></span>
    <button class="wt-close" on:click={() => dispatch('close')} aria-label={$i18nT('viewer.close')}><X size={18} /></button>
  </header>

  {#if loading}
    <div class="wt-overlay">
      <div class="spin"></div>
      <p>
        {#if $ifcProgress?.phase === 'parse'}
          {$i18nT('viewer.parsingModel')}
        {:else if $ifcProgress?.phase === 'fetch' && $ifcProgress.total > 0}
          {$i18nT('viewer.walkLoading')} {Math.min(99, Math.round(($ifcProgress.loaded / $ifcProgress.total) * 100))}%
        {:else}
          {$i18nT('viewer.walkLoading')}
        {/if}
      </p>
    </div>
  {:else if error}
    <div class="wt-overlay"><p class="err">{error}</p></div>
  {:else if !locked}
    <button class="wt-enter" on:click={enterWalk}>
      <span class="wt-enter-title"><MousePointerClick size={18} /> {$i18nT('viewer.walkClickToEnter')}</span>
      <!-- Keycap cheat-sheet: the controls at a glance, before you're inside. -->
      <span class="wt-keys" aria-hidden="true">
        <span class="wt-keygroup">
          <span class="wasd">
            <kbd class="k">W</kbd>
            <span class="wasd-row"><kbd class="k">A</kbd><kbd class="k">S</kbd><kbd class="k">D</kbd></span>
          </span>
          <small>{$i18nT('viewer.keyMove')}</small>
        </span>
        <span class="wt-keygroup">
          <span class="mouse-ico"><Mouse size={34} /></span>
          <small>{dragLook ? $i18nT('viewer.keyLookDrag') : $i18nT('viewer.keyLook')}</small>
        </span>
        <span class="wt-keygroup">
          <kbd class="k space">Space</kbd>
          <small>{mode === 'walk' ? $i18nT('viewer.keyJump') : $i18nT('viewer.keyUp')}</small>
        </span>
        <span class="wt-keygroup">
          <kbd class="k wide">Ctrl</kbd>
          <small>{mode === 'walk' ? $i18nT('viewer.keyCrouch') : $i18nT('viewer.keyDown')}</small>
        </span>
        <span class="wt-keygroup">
          <kbd class="k wide">Shift</kbd>
          <small>{$i18nT('viewer.keyRun')}</small>
        </span>
      </span>
      <small>{$i18nT('viewer.walkMoreKeys')}</small>
    </button>
  {/if}

  <!-- Picked-element info card -->
  {#if picked}
    <div class="wt-info">
      <div class="wt-info-head">
        <strong>{picked.label}</strong>
        <button on:click={clearPicked} aria-label={$i18nT('viewer.close')}><X size={14} /></button>
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
    background: #dce6f0; /* matches the daylight scene while it loads */
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
  /* Walk ↔ Fly mode toggle (usable while unlocked; `F` toggles while walking). */
  .wt-mode {
    pointer-events: auto;
    display: inline-flex;
    margin-left: 12px;
    border-radius: 8px;
    overflow: hidden;
    border: 1px solid rgba(255, 255, 255, 0.18);
  }
  .wt-mode button {
    border: 0;
    background: rgba(20, 30, 44, 0.82);
    color: #cdd9e6;
    font-size: 0.76rem;
    font-weight: 600;
    padding: 5px 12px;
    cursor: pointer;
  }
  .wt-mode button + button {
    border-left: 1px solid rgba(255, 255, 255, 0.18);
  }
  .wt-mode button.active {
    background: var(--brand-600, #2563a8);
    color: #fff;
  }
  .wt-mode button:focus-visible {
    outline: none;
    box-shadow: inset 0 0 0 2px #ff8a2a;
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
  .wt-hints {
    position: absolute;
    left: 50%;
    bottom: 14px;
    transform: translateX(-50%);
    background: rgba(8, 13, 22, 0.72);
    color: #c8d6e4;
    padding: 5px 14px;
    border-radius: 8px;
    font-size: 0.74rem;
    letter-spacing: 0.01em;
    white-space: nowrap;
    pointer-events: none;
    max-width: 92vw;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .wt-mode button:disabled {
    opacity: 0.45;
    cursor: default;
  }
  .wt-overlay {
    position: absolute;
    inset: 0;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 12px;
    color: #3c4a5a; /* readable over the daylight backdrop */
  }
  .err {
    color: #c0392b;
  }
  .spin {
    width: 34px;
    height: 34px;
    border: 3px solid rgba(20, 30, 44, 0.18);
    border-top-color: #e8590c;
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
  .wt-enter-title {
    display: inline-flex;
    align-items: center;
    gap: 7px;
  }
  /* Keycap cheat-sheet on the enter overlay. */
  .wt-keys {
    display: flex;
    align-items: flex-end;
    gap: 22px;
    margin: 14px 4px 6px;
  }
  .wt-keygroup {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 7px;
  }
  .wt-keygroup small {
    font-weight: 500;
    font-size: 0.68rem;
    color: #9fb2c6;
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }
  .wasd {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 4px;
  }
  .wasd-row {
    display: flex;
    gap: 4px;
  }
  kbd.k {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    min-width: 28px;
    height: 28px;
    padding: 0 7px;
    border-radius: 6px;
    border: 1px solid rgba(255, 255, 255, 0.28);
    border-bottom-width: 2.5px;
    background: rgba(255, 255, 255, 0.08);
    color: #eef4fb;
    font-family: inherit;
    font-size: 0.74rem;
    font-weight: 700;
    line-height: 1;
  }
  kbd.k.space {
    min-width: 84px;
  }
  kbd.k.wide {
    min-width: 46px;
  }
  .mouse-ico {
    display: flex;
    align-items: center;
    justify-content: center;
    height: 60px;
    color: #cdd9e6;
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
