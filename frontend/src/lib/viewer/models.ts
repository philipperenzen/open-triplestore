// Shared 3D-model utilities for the viewers: URL format detection, a cached
// three.js loader and real-world sizing for georeferenced map placement. All
// three.js work stays in this module + the viewer components so the heavy
// 'three' chunk loads only on demand.

import * as THREE from 'three';
import { GLTFLoader } from 'three/addons/loaders/GLTFLoader.js';
import { STLLoader } from 'three/addons/loaders/STLLoader.js';
import { parseCityJSON, parseCityGML } from './cityjson';

export type { ModelFormat, ModelRef } from './detect';
export { modelFormatFromUrl, modelRefOf } from './detect';
import type { ModelFormat } from './detect';

/** Extra facts loadModel() records on the group for georeferenced placement. */
export interface ModelGeoData {
  format: ModelFormat;
  /** Pre-normalisation bounding box in source units (metres where trustworthy). */
  realSize: { x: number; y: number; z: number };
  /** WGS84 anchor carried by the model itself (CityJSON/CityGML with a known CRS). */
  anchorLonLat: [number, number] | null;
}

/** Default mesh colour for formats without materials (STL), theme-aware. */
export function defaultMaterial(dark: boolean): THREE.Material {
  return new THREE.MeshStandardMaterial({
    color: dark ? 0xaebfd0 : 0x8fa6ba,
    roughness: 0.8,
  });
}

const modelCache = new Map<string, Promise<THREE.Group>>();

/**
 * Load a model into a normalised group (unit-ish bounding box, sitting on the
 * ground plane, centred on x/z) with [ModelGeoData] in `userData`. Cached per
 * URL (+ orientation); callers must `.clone()` before adding to a scene so
 * cached geometry is never mutated per-consumer.
 *
 * `upAxis: 'Z'` rotates a Z-up model into the scene's Y-up convention. There is
 * no reliable way to detect a file's up-axis (3D-print STLs are usually Z-up,
 * but plenty are exported Y-up — a tower and a bridge can't both win under one
 * default), so orientation is *data*: the linked-data geometry node may carry
 * `ots:modelUpAxis "Z"`, which the viewer feed forwards per element.
 */
export function loadModel(
  url: string,
  format: ModelFormat,
  opts: { upAxis?: string | null } = {},
): Promise<THREE.Group> {
  const upAxis = (opts.upAxis || '').toUpperCase() === 'Z' ? 'Z' : null;
  const key = `${format}:${upAxis ?? '-'}:${url}`;
  let p = modelCache.get(key);
  if (!p) {
    p = (async () => {
      const group = new THREE.Group();
      let anchorLonLat: [number, number] | null = null;
      if (format === 'gltf') {
        const gltf = await new GLTFLoader().loadAsync(url);
        group.add(gltf.scene);
      } else if (format === 'stl') {
        const geom = await new STLLoader().loadAsync(url);
        geom.computeVertexNormals();
        group.add(new THREE.Mesh(geom, defaultMaterial(false)));
      } else if (format === 'ifc') {
        // web-ifc (WASM) loads on demand; a `#GlobalId` fragment isolates one
        // element. Meshes carry `userData.ifcGuid` for per-element picking.
        const { loadIfcGroup } = await import('./ifc');
        group.add(await loadIfcGroup(url));
      } else {
        const res = await fetch(url);
        if (!res.ok) throw new Error(`fetch failed: ${res.status}`);
        const city = format === 'cityjson' ? parseCityJSON(await res.json()) : parseCityGML(await res.text());
        group.add(city.group);
        anchorLonLat = city.anchorLonLat;
      }
      // Annotated Z-up content rotates into the Y-up scene BEFORE measuring, so
      // realSize.y is the real-world height. IFC manages its own axes.
      if (upAxis === 'Z' && format !== 'ifc') {
        for (const child of group.children) {
          child.rotation.x = -Math.PI / 2;
        }
        group.updateMatrixWorld(true);
      }
      const box = new THREE.Box3().setFromObject(group);
      const size = box.getSize(new THREE.Vector3());
      const geo: ModelGeoData = {
        format,
        realSize: { x: size.x, y: size.y, z: size.z },
        anchorLonLat,
      };
      group.userData.geo = geo;
      normalise(group);
      return group;
    })();
    modelCache.set(key, p);
    p.catch(() => modelCache.delete(key)); // allow retry after transient failures
  }
  return p;
}

/** The box size normalise() scales a model's largest dimension to. */
export const NORMALISED_DIM = 1.6;

/** Scale + centre `object3d` into a ~1.6-unit box resting on the ground plane. */
export function normalise(object3d: THREE.Object3D): void {
  const box = new THREE.Box3().setFromObject(object3d);
  const size = box.getSize(new THREE.Vector3());
  const maxDim = Math.max(size.x, size.y, size.z) || 1;
  object3d.scale.setScalar(NORMALISED_DIM / maxDim);
  const scaled = new THREE.Box3().setFromObject(object3d);
  const centre = scaled.getCenter(new THREE.Vector3());
  object3d.position.x -= centre.x;
  object3d.position.z -= centre.z;
  object3d.position.y -= scaled.min.y;
}

/**
 * Real-world size (largest dimension, metres) to render a loaded model at.
 * glTF and georeferenced city models declare metres, so their size is trusted
 * within a sanity clamp; STL units are arbitrary (print scale, mm, …), so an
 * implausible size falls back to `fallbackMeters`.
 */
export function realWorldMeters(group: THREE.Object3D, fallbackMeters: number): number {
  const geo = group.userData?.geo as ModelGeoData | undefined;
  if (!geo) return fallbackMeters;
  const maxDim = Math.max(geo.realSize.x, geo.realSize.y, geo.realSize.z);
  if (!Number.isFinite(maxDim) || maxDim <= 0) return fallbackMeters;
  const trusted = geo.format !== 'stl';
  const [lo, hi] = trusted ? [1, 4000] : [10, 1500];
  return maxDim >= lo && maxDim <= hi ? maxDim : fallbackMeters;
}
