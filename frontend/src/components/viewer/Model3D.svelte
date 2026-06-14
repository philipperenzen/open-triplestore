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

  /** Models to show: [{ id, label, url, format, slot?: [x, z] }]. */
  export let refs = [];
  export let height = '100%';
  /** Currently selected model id (highlighted). */
  export let selected = '';

  const dispatch = createEventDispatcher();

  let canvasEl;
  let renderer = null;
  let scene, camera, controls, raycaster, frameId, grid;
  let groupsById = new Map();
  let loadedCount = 0;
  let failedCount = 0;
  let dark = false;
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
        if (n.isMesh && n.userData.stl) n.material = defaultMaterial(dark);
      });
    }
    highlight();
  }

  function highlight() {
    for (const [id, group] of groupsById) {
      const isSel = id === selected && groupsById.size > 1;
      group.traverse((node) => {
        if (node.isMesh && node.material && 'emissive' in node.material) {
          node.material.emissive = isSel ? SELECT_COLOR : new THREE.Color(0x000000);
          node.material.emissiveIntensity = isSel ? 0.55 : 0;
        }
      });
    }
  }

  async function loadAll() {
    // Clear any previous set: the modal/preview overlay reuse one live
    // instance across `refs` changes, so stale groups must not linger.
    for (const group of groupsById.values()) scene.remove(group);
    groupsById = new Map();
    loadedCount = 0;
    failedCount = 0;
    const wanted = refs;
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
        if (refs !== wanted) return; // a newer refs set superseded this load
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
        loadedCount += 1;
      } catch {
        if (refs !== wanted) return;
        failedCount += 1;
        const placeholder = new THREE.Mesh(
          new THREE.BoxGeometry(1, 1, 1),
          new THREE.MeshStandardMaterial({ color: 0x9aa6b2, wireframe: true })
        );
        placeholder.position.y = 0.5;
        group.add(placeholder);
      }
    });
    await Promise.allSettled(tasks);
    if (refs !== wanted) return;
    highlight();
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
    // IFC meshes carry their element's GlobalId — picking one selects that
    // *atom* (a beam, a slab), not just the whole model.
    const guid = hit.userData?.ifcGuid || null;
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
    renderer = new THREE.WebGLRenderer({ canvas: canvasEl, antialias: true });
    raycaster = new THREE.Raycaster();

    scene.add(new THREE.AmbientLight(0xffffff, 0.9));
    const sun = new THREE.DirectionalLight(0xffffff, 1.4);
    sun.position.set(4, 8, 5);
    scene.add(sun);
    grid = new THREE.GridHelper(20, 20, 0x5a7a9a, 0x44607a);
    grid.material.transparent = true;
    scene.add(grid);
    applyTheme();

    controls = new OrbitControls(camera, renderer.domElement);
    controls.target.set(0, 0.6, 0);
    controls.enableDamping = true;

    const resize = () => {
      const w = canvasEl.clientWidth || 480;
      const h = canvasEl.clientHeight || 320;
      renderer.setSize(w, h, false);
      camera.aspect = w / h;
      camera.updateProjectionMatrix();
    };
    resize();
    const observer = new ResizeObserver(resize);
    observer.observe(canvasEl);

    const animate = () => {
      frameId = requestAnimationFrame(animate);
      controls.update();
      renderer.render(scene, camera);
    };
    animate();

    return () => observer.disconnect();
  });

  onDestroy(() => {
    unsubTheme();
    if (frameId) cancelAnimationFrame(frameId);
    if (renderer) renderer.dispose();
    renderer = null;
  });

  $: if (scene && selected !== undefined) highlight();
  // Reload when the refs set changes identity (modal navigation, next preview).
  let lastRefs = null;
  $: if (scene && refs !== lastRefs) {
    lastRefs = refs;
    loadAll();
  }
</script>

<div class="model-3d" style:height>
  <canvas bind:this={canvasEl} on:click={onClick} aria-label="3D model viewer"></canvas>
  {#if refs.length === 0}
    <div class="overlay">{$i18nT('viewer.noModels')}</div>
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
</style>
