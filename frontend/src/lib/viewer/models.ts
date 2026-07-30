// Shared 3D-model utilities for the viewers: URL format detection, a cached
// three.js loader and real-world sizing for georeferenced map placement. All
// three.js work stays in this module + the viewer components so the heavy
// 'three' chunk loads only on demand.

import * as THREE from 'three';
import { GLTFLoader } from 'three/addons/loaders/GLTFLoader.js';
import { STLLoader } from 'three/addons/loaders/STLLoader.js';
import { parseCityJSON, parseCityGML, type CityObject } from './cityjson';
import { cityBaseUrl, cityObjectFragment } from './detect';
// Shared with Model3D's re-render guard. It lives in refsSignature.ts (which
// deliberately imports no 'three', so it stays out of the WebGL chunk and is
// unit-testable on its own) — this module is the one that must follow, or the
// cache key and the reload signature could drift apart.
import { guidsSignature } from './refsSignature';

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
  /** Selectable CityObjects in a CityJSON/CityGML model (empty otherwise) — the
   *  per-building metadata a pick resolves to (BAG id, function, storeys …). */
  cityObjects: CityObject[];
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
  opts: { upAxis?: string | null; guids?: string[] } = {},
): Promise<THREE.Group> {
  const upAxis = (opts.upAxis || '').toUpperCase() === 'Z' ? 'Z' : null;
  // A subtree (an IFC container's descendant leaf guids) must not collide in the
  // cache with the whole model or with another subtree of the same file: fold a
  // compact, order-independent digest of the guid set into the key.
  const guidKey = guidsSignature(opts.guids) || '-';
  const key = `${format}:${upAxis ?? '-'}:${guidKey}:${url}`;
  let p = modelCache.get(key);
  if (!p) {
    p = (async () => {
      const group = new THREE.Group();
      let anchorLonLat: [number, number] | null = null;
      let cityObjects: CityObject[] = [];
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
        group.add(await loadIfcGroup(url, { guids: opts.guids }));
      } else {
        // A `#objectId` fragment isolates one CityObject from a shared file (the
        // CityJSON analogue of an IFC element fragment); the fetch drops it.
        const frag = cityObjectFragment(url);
        const only = frag ? new Set([frag]) : undefined;
        const res = await fetch(cityBaseUrl(url));
        if (!res.ok) throw new Error(`fetch failed: ${res.status}`);
        const city =
          format === 'cityjson'
            ? parseCityJSON(await res.json(), { only })
            : parseCityGML(await res.text(), { only });
        group.add(city.group);
        anchorLonLat = city.anchorLonLat;
        cityObjects = city.objects;
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
        cityObjects,
      };
      group.userData.geo = geo;
      // IFC: the parse records the file's own elevation-0 (see ifc.ts). Resting
      // the bbox MINIMUM on the ground instead floats the ground floor by the
      // depth of the foundations/basement below grade — the "floating house".
      const groundY =
        format === 'ifc' ? (group.children[0]?.userData?.ifcGroundY as number | undefined) : undefined;
      normalise(group, { groundY });
      return group;
    })();
    modelCache.set(key, p);
    p.catch(() => modelCache.delete(key)); // allow retry after transient failures
  }
  return p;
}

/** The box size normalise() scales a model's largest dimension to. */
export const NORMALISED_DIM = 1.6;

/**
 * Scale + centre `object3d` into a ~1.6-unit box standing on the ground plane.
 *
 * `groundY` (source units) is the model's own ground line — for IFC, the
 * file's elevation-0 from the coordination matrix. When it is plausible (at or
 * above the bbox floor, within the lower 35% of the model — i.e. a basement,
 * not a mis-georeferenced file) the model rests on THAT line, so below-grade
 * geometry sits below y=0 instead of lifting the whole building.
 */
export function normalise(object3d: THREE.Object3D, opts: { groundY?: number } = {}): void {
  const box = new THREE.Box3().setFromObject(object3d);
  const size = box.getSize(new THREE.Vector3());
  const maxDim = Math.max(size.x, size.y, size.z) || 1;
  const s = NORMALISED_DIM / maxDim;
  object3d.scale.setScalar(s);
  const scaled = new THREE.Box3().setFromObject(object3d);
  const centre = scaled.getCenter(new THREE.Vector3());
  object3d.position.x -= centre.x;
  object3d.position.z -= centre.z;
  let rest = scaled.min.y;
  const g = opts.groundY;
  if (typeof g === 'number' && Number.isFinite(g)) {
    const gs = g * s; // source units → the scaled space `scaled` is measured in
    const height = scaled.max.y - scaled.min.y;
    if (gs >= scaled.min.y - 1e-9 && gs <= scaled.min.y + height * 0.35) {
      rest = gs;
    }
  }
  object3d.position.y -= rest;
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
