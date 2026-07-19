// Shared "studio" render look for the 3D viewers (Model3D, Walkthrough, the
// map's model layer). Two ingredients:
//
//  - An image-based light: three's RoomEnvironment baked through PMREM and set
//    as `scene.environment`. PBR materials pick it up automatically, which is
//    what turns the old flat "plastic" shading into soft, directional studio
//    light with believable reflections — including INSIDE a building, where
//    point-light-only setups left interiors murky.
//  - ACES filmic tone mapping, which rolls highlights off naturally instead of
//    clipping them (the main cause of the washed-out / oversaturated look).
//
// The environment is generated once per WebGL renderer (a one-off ~1 ms bake)
// and cached per renderer instance, so the map layer, modals and walkthrough
// each pay it at most once per context.

import * as THREE from 'three';
import { RoomEnvironment } from 'three/addons/environments/RoomEnvironment.js';

const envByRenderer = new WeakMap<THREE.WebGLRenderer, THREE.Texture>();

/** Configure filmic output on a renderer (idempotent). */
export function applyStudioLook(renderer: THREE.WebGLRenderer): void {
  renderer.toneMapping = THREE.ACESFilmicToneMapping;
  renderer.toneMappingExposure = 1.0;
}

/** The cached PMREM studio environment texture for `renderer`. */
export function studioEnvironment(renderer: THREE.WebGLRenderer): THREE.Texture {
  let tex = envByRenderer.get(renderer);
  if (!tex) {
    const pmrem = new THREE.PMREMGenerator(renderer);
    const room = new RoomEnvironment();
    tex = pmrem.fromScene(room, 0.04).texture;
    pmrem.dispose();
    room.dispose?.();
    envByRenderer.set(renderer, tex);
  }
  return tex;
}
