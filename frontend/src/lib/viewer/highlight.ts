// Shared selection-highlight geometry for the 3D viewers.
//
// A merged IFC building is a handful of per-material meshes, so a single wall
// cannot be lit by swapping a mesh's material: the highlight is a *copy* of the
// selected element's triangles, drawn over the original.
//
// The copy used to be drawn with `depthTest = false` on top of a ghosted (34 %
// opaque, non-occluding) building — an x-ray that made the selected slab float
// through the walls, and let the contact-shadow disc show through as a dark ring
// on the ground. That is now opt-in: by default the overlay RESPECTS depth, so
// it sits exactly on the geometry it belongs to and anything in front of it
// still covers it, and the surrounding building keeps its full opacity.
//
// Because the overlay shares the source's exact vertices and transform, its
// fragments tie with the original's in the depth buffer. A negative polygon
// offset wins that tie without inflating the geometry (a larger offset would
// make a 200 mm slab poke through its neighbours).

import * as THREE from 'three';
import { subGeometryForGuids } from './ifc';
import { subCityGeometryForObjects } from './cityjson';

/** Vivid highlight colour for the selected element(s). */
export const HL_COLOR = 0xff6a00;

/** Emissive intensity a settled highlight overlay reaches. */
export const HL_EMISSIVE = 0.9;

/** Render order of the overlay: just above the model in the default (depth
 *  respecting) mode, far above everything in x-ray mode. */
export const HL_RENDER_ORDER = 3;
export const HL_RENDER_ORDER_XRAY = 12;

/**
 * A highlight material derived from `base` (so it keeps the source's shading
 * model), starting fully transparent so a caller can ease it in.
 *
 * `xray` turns the depth test off — the old always-visible-through-the-building
 * behaviour, now only reachable from the map's explicit X-ray toggle.
 */
export function makeHighlightMaterial(base: THREE.Material | null, xray = false): THREE.Material {
  const m = (base ? base.clone() : new THREE.MeshStandardMaterial()) as THREE.MeshStandardMaterial;
  if (m.color) m.color.setHex(HL_COLOR);
  if ('metalness' in m) m.metalness = 0;
  if ('roughness' in m) m.roughness = 0.5;
  if ('emissive' in m && m.emissive) {
    m.emissive.setHex(HL_COLOR);
    m.emissiveIntensity = 0;
  }
  // Render BOTH faces: a thin floor slab seen from above shows its underside,
  // which a single-sided material would cull — the "no highlight at all" case.
  m.side = THREE.DoubleSide;
  m.depthWrite = true;
  m.depthTest = !xray;
  // Coplanar decal: the overlay is the same triangles at the same transform, so
  // it ties with the source in the depth buffer. A small negative offset makes
  // it win the tie; anything genuinely in front of it still occludes it.
  m.polygonOffset = true;
  m.polygonOffsetFactor = -2;
  m.polygonOffsetUnits = -4;
  // Start invisible; the caller tweens opacity/emissive to their targets.
  m.transparent = true;
  m.opacity = 0;
  return m;
}

/**
 * Materialise an already-extracted sub-geometry group as a highlight overlay.
 * Shared by both selection kinds — an IFC element (GlobalId) and a CityJSON
 * building (CityObject id) — because the two differ only in how the triangles
 * are sliced out, never in how the highlight should look or behave.
 */
function materialiseOverlay(
  group: THREE.Object3D,
  ov: THREE.Group,
  xray: boolean,
): THREE.Group | null {
  if (!ov.children.length) return null;
  // subGeometryForGuids rebuilds the buffers when it slices a MERGED mesh, but
  // returns a plain `mesh.clone()` for an already-isolated per-element mesh —
  // and a clone SHARES its geometry with the source. Disposing that on the next
  // selection would blank the model itself, so record which geometries the
  // overlay actually owns.
  const shared = new Set<THREE.BufferGeometry>();
  group.traverse((n: THREE.Object3D) => {
    const mesh = n as THREE.Mesh;
    if (mesh.isMesh && mesh.geometry) shared.add(mesh.geometry);
  });
  ov.traverse((n: THREE.Object3D) => {
    const mesh = n as THREE.Mesh;
    if (!mesh.isMesh) return;
    const base = Array.isArray(mesh.material) ? mesh.material[0] : mesh.material;
    mesh.material = makeHighlightMaterial(base ?? null, xray);
    mesh.renderOrder = xray ? HL_RENDER_ORDER_XRAY : HL_RENDER_ORDER;
    mesh.userData.isOverlay = true;
    mesh.userData.ownsGeometry = !shared.has(mesh.geometry);
    mesh.raycast = () => {}; // the merged model under it owns picking
  });
  ov.userData.isOverlay = true;
  return ov;
}

/**
 * A non-pickable copy of `wantedGuids`' triangles inside `group`, materialised
 * as a highlight. Returns null when nothing in this model matches, so callers
 * can skip the add/dispose dance entirely.
 *
 * The result must be added as a child of the SAME group the geometry came from,
 * so it inherits the identical placement transform and the polygon offset stays
 * exact.
 */
export function buildHighlightOverlay(
  group: THREE.Object3D,
  wantedGuids: Set<string>,
  xray = false,
): THREE.Group | null {
  if (!group || !wantedGuids || wantedGuids.size === 0) return null;
  return materialiseOverlay(group, subGeometryForGuids(group, wantedGuids), xray);
}

/** The CityJSON counterpart of [buildHighlightOverlay]: highlights whole
 *  CityObjects (one building of a merged 3DBAG block) instead of IFC elements. */
export function buildCityHighlightOverlay(
  group: THREE.Object3D,
  wantedObjects: Set<string>,
  xray = false,
): THREE.Group | null {
  if (!group || !wantedObjects || wantedObjects.size === 0) return null;
  return materialiseOverlay(group, subCityGeometryForObjects(group, wantedObjects), xray);
}

/** Free an overlay's materials, and only the geometry it built for itself (see
 *  `ownsGeometry` above — a shared buffer belongs to the source model). */
export function disposeHighlightOverlay(ov: THREE.Object3D | null | undefined): void {
  if (!ov) return;
  ov.traverse((n: THREE.Object3D) => {
    const mesh = n as THREE.Mesh;
    if (!mesh.isMesh) return;
    if (mesh.userData.ownsGeometry !== false) mesh.geometry?.dispose?.();
    const mats = Array.isArray(mesh.material) ? mesh.material : [mesh.material];
    for (const m of mats) m?.dispose?.();
  });
}
