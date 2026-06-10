<script>
  // 3D viewer for the dataset viewer: loads each element's 3D model (glTF
  // preferred, STL fallback — e.g. the Wikidata/Commons landmark models) on a
  // ground grid, normalised to a unit bounding box. Clicking a model selects its
  // element (raycaster); the selected model is highlighted. three.js is a
  // bundled npm dependency; loaders come from three/addons.
  import { onMount, onDestroy, createEventDispatcher } from 'svelte';
  import * as THREE from 'three';
  import { OrbitControls } from 'three/addons/controls/OrbitControls.js';
  import { GLTFLoader } from 'three/addons/loaders/GLTFLoader.js';
  import { STLLoader } from 'three/addons/loaders/STLLoader.js';
  import { modelRefs } from '../../lib/viewer/geometry';

  /** @type {import('../../lib/viewer/geometry').ViewerElement[]} */
  export let elements = [];
  export let selected = '';
  export let height = '100%';

  const dispatch = createEventDispatcher();

  let canvasEl;
  let renderer = null;
  let scene, camera, controls, raycaster, frameId;
  /** root group per element IRI, for picking + highlight */
  let groupsById = new Map();
  let loadErrors = 0;
  let loadedCount = 0;
  let refs = [];

  const SELECT_COLOR = new THREE.Color('#e8590c');

  function normalise(object3d) {
    // Scale + centre the model into a unit box sitting on the ground plane.
    const box = new THREE.Box3().setFromObject(object3d);
    const size = box.getSize(new THREE.Vector3());
    const maxDim = Math.max(size.x, size.y, size.z) || 1;
    const scale = 1.6 / maxDim;
    object3d.scale.setScalar(scale);
    const scaled = new THREE.Box3().setFromObject(object3d);
    const centre = scaled.getCenter(new THREE.Vector3());
    object3d.position.x -= centre.x;
    object3d.position.z -= centre.z;
    object3d.position.y -= scaled.min.y;
  }

  function highlight() {
    for (const [id, group] of groupsById) {
      const isSel = id === selected;
      group.traverse((node) => {
        if (node.isMesh && node.material && 'emissive' in node.material) {
          node.material.emissive = isSel ? SELECT_COLOR : new THREE.Color(0x000000);
          node.material.emissiveIntensity = isSel ? 0.55 : 0;
        }
      });
    }
  }

  async function loadModels() {
    refs = modelRefs(elements);
    const gltfLoader = new GLTFLoader();
    const stlLoader = new STLLoader();
    for (const ref of refs) {
      const group = new THREE.Group();
      group.userData.elementId = ref.id;
      group.position.set(ref.slot[0], 0, ref.slot[1]);
      scene.add(group);
      groupsById.set(ref.id, group);
      try {
        if (ref.format === 'gltf') {
          const gltf = await gltfLoader.loadAsync(ref.url);
          normalise(gltf.scene);
          group.add(gltf.scene);
        } else {
          const geom = await stlLoader.loadAsync(ref.url);
          geom.computeVertexNormals();
          const mesh = new THREE.Mesh(
            geom,
            new THREE.MeshStandardMaterial({ color: 0x9db4c8, roughness: 0.8 })
          );
          normalise(mesh);
          group.add(mesh);
        }
        loadedCount += 1;
      } catch {
        // Network/CORS/parse failure: show a placeholder box so the element
        // stays selectable, and count the failure for the status line.
        loadErrors += 1;
        const placeholder = new THREE.Mesh(
          new THREE.BoxGeometry(1, 1, 1),
          new THREE.MeshStandardMaterial({ color: 0xcccccc, wireframe: true })
        );
        placeholder.position.y = 0.5;
        group.add(placeholder);
      }
    }
    highlight();
  }

  function onClick(event) {
    if (!renderer) return;
    const rect = renderer.domElement.getBoundingClientRect();
    const pointer = new THREE.Vector2(
      ((event.clientX - rect.left) / rect.width) * 2 - 1,
      -((event.clientY - rect.top) / rect.height) * 2 + 1
    );
    raycaster.setFromCamera(pointer, camera);
    const hits = raycaster.intersectObjects([...groupsById.values()], true);
    if (!hits.length) return;
    let node = hits[0].object;
    while (node && !node.userData.elementId) node = node.parent;
    if (node?.userData.elementId) dispatch('select', { id: node.userData.elementId });
  }

  onMount(() => {
    scene = new THREE.Scene();
    scene.background = new THREE.Color(0x10151c);
    camera = new THREE.PerspectiveCamera(50, 1, 0.01, 1000);
    camera.position.set(3.5, 2.5, 5);
    renderer = new THREE.WebGLRenderer({ canvas: canvasEl, antialias: true });
    raycaster = new THREE.Raycaster();

    scene.add(new THREE.AmbientLight(0xffffff, 0.9));
    const sun = new THREE.DirectionalLight(0xffffff, 1.4);
    sun.position.set(4, 8, 5);
    scene.add(sun);
    scene.add(new THREE.GridHelper(20, 20, 0x335577, 0x223344));

    controls = new OrbitControls(camera, renderer.domElement);
    controls.target.set(1, 0.6, 1);

    const resize = () => {
      const w = canvasEl.clientWidth || 480;
      const h = canvasEl.clientHeight || 360;
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
    loadModels();

    return () => observer.disconnect();
  });

  onDestroy(() => {
    if (frameId) cancelAnimationFrame(frameId);
    if (renderer) renderer.dispose();
    renderer = null;
  });

  $: if (scene && selected !== undefined) highlight();
</script>

<div class="viewer-3d" style:height>
  <canvas bind:this={canvasEl} on:click={onClick} aria-label="3D viewer"></canvas>
  {#if refs.length === 0}
    <div class="overlay">No 3D models in this dataset</div>
  {:else if loadErrors > 0}
    <div class="overlay subtle">{loadedCount}/{refs.length} models loaded</div>
  {/if}
</div>

<style>
  .viewer-3d {
    position: relative;
    width: 100%;
    min-height: 240px;
    border-radius: 8px;
    overflow: hidden;
    background: #10151c;
  }
  canvas {
    width: 100%;
    height: 100%;
    display: block;
    cursor: pointer;
  }
  .overlay {
    position: absolute;
    inset: auto 8px 8px auto;
    padding: 4px 10px;
    border-radius: 6px;
    background: rgba(0, 0, 0, 0.55);
    color: #dbe4ee;
    font-size: 0.8rem;
    pointer-events: none;
  }
  .overlay.subtle {
    opacity: 0.8;
  }
</style>
