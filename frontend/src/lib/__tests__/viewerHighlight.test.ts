import { describe, it, expect } from 'vitest';
import * as THREE from 'three';
import {
  HL_COLOR,
  HL_RENDER_ORDER,
  HL_RENDER_ORDER_XRAY,
  buildHighlightOverlay,
  disposeHighlightOverlay,
  makeHighlightMaterial,
} from '../viewer/highlight';

/** A stand-in for an IFC model that was loaded per element (one mesh per guid). */
function building(guids: string[]): THREE.Group {
  const g = new THREE.Group();
  for (const guid of guids) {
    const mesh = new THREE.Mesh(
      new THREE.BoxGeometry(1, 1, 1),
      new THREE.MeshStandardMaterial({ color: 0x334455 }),
    );
    mesh.userData.ifcGuid = guid;
    g.add(mesh);
  }
  return g;
}

describe('makeHighlightMaterial', () => {
  it('respects depth by default and offsets the coplanar decal', () => {
    const m = makeHighlightMaterial(null) as THREE.MeshStandardMaterial;
    expect(m.depthTest).toBe(true);
    expect(m.depthWrite).toBe(true);
    expect(m.polygonOffset).toBe(true);
    expect(m.polygonOffsetFactor).toBeLessThan(0);
    expect(m.side).toBe(THREE.DoubleSide);
    expect(m.color.getHex()).toBe(HL_COLOR);
    expect(m.emissive.getHex()).toBe(HL_COLOR);
    // Starts invisible so the caller can ease it in.
    expect(m.opacity).toBe(0);
    expect(m.emissiveIntensity).toBe(0);
  });

  it('only drops the depth test in x-ray mode', () => {
    expect((makeHighlightMaterial(null, true) as THREE.Material).depthTest).toBe(false);
  });

  it('derives from the source material so shading is preserved', () => {
    const base = new THREE.MeshPhysicalMaterial({ color: 0x102030, roughness: 0.9 });
    const m = makeHighlightMaterial(base) as THREE.MeshPhysicalMaterial;
    expect(m.type).toBe('MeshPhysicalMaterial');
    expect(base.color.getHex()).toBe(0x102030); // the source is never mutated
  });
});

describe('buildHighlightOverlay', () => {
  it('returns null when nothing in the model matches', () => {
    const g = building(['a', 'b']);
    expect(buildHighlightOverlay(g, new Set(['zzz']))).toBeNull();
    expect(buildHighlightOverlay(g, new Set())).toBeNull();
  });

  it('marks the copy non-pickable and orders it above the model', () => {
    const g = building(['a', 'b']);
    const ov = buildHighlightOverlay(g, new Set(['a']))!;
    expect(ov).not.toBeNull();
    const meshes: THREE.Mesh[] = [];
    ov.traverse((n) => {
      if ((n as THREE.Mesh).isMesh) meshes.push(n as THREE.Mesh);
    });
    expect(meshes).toHaveLength(1);
    expect(meshes[0].renderOrder).toBe(HL_RENDER_ORDER);
    expect(meshes[0].userData.isOverlay).toBe(true);
    // A no-op raycast keeps the merged model underneath in charge of picking.
    const hits: unknown[] = [];
    meshes[0].raycast(new THREE.Raycaster(), hits as never);
    expect(hits).toHaveLength(0);
  });

  it('renders on top only when x-ray is asked for', () => {
    const ov = buildHighlightOverlay(building(['a']), new Set(['a']), true)!;
    ov.traverse((n) => {
      const mesh = n as THREE.Mesh;
      if (!mesh.isMesh) return;
      expect(mesh.renderOrder).toBe(HL_RENDER_ORDER_XRAY);
      expect((mesh.material as THREE.Material).depthTest).toBe(false);
    });
  });

  it('never disposes geometry it shares with the source model', () => {
    // An already-isolated mesh is cloned, and a clone shares its geometry — so
    // disposing the overlay must not blank the building it was taken from.
    const g = building(['a']);
    const src = g.children[0] as THREE.Mesh;
    let disposed = false;
    src.geometry.addEventListener('dispose', () => {
      disposed = true;
    });
    const ov = buildHighlightOverlay(g, new Set(['a']))!;
    disposeHighlightOverlay(ov);
    expect(disposed).toBe(false);
    expect(src.geometry.attributes.position).toBeDefined();
  });
});
