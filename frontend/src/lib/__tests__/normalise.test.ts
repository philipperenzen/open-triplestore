// Regression test for the stale-matrixWorld measurement bug.
//
// Since three r177, Object3D.updateWorldMatrix() only recomposes matrixWorld
// for nodes whose LOCAL matrix changed. The merged IFC meshes run with
// matrixAutoUpdate=false, so after normalise() scales their ancestor group,
// Box3.setFromObject() silently kept measuring the matrixWorld those meshes
// carried before the scale — the box came back in raw model units and the
// centre/ground offsets landed ~1/scale off. normalise() must therefore
// force-compose around its measurements and hand back a composed tree.
import { describe, it, expect } from 'vitest';
import * as THREE from 'three';
import { normalise, NORMALISED_DIM } from '../viewer/models';

/** A merged-IFC-style mesh: transforms baked into vertices, auto-update off. */
function bakedMesh(sizeM: number, offset: [number, number, number]): THREE.Mesh {
  const geom = new THREE.BoxGeometry(sizeM, sizeM, sizeM);
  geom.translate(offset[0], offset[1], offset[2]);
  const mesh = new THREE.Mesh(geom, new THREE.MeshBasicMaterial());
  mesh.matrixAutoUpdate = false;
  return mesh;
}

function worldBox(root: THREE.Object3D): THREE.Box3 {
  // Compose explicitly — this measurement models "what a render would show".
  root.updateMatrixWorld(true);
  return new THREE.Box3().setFromObject(root);
}

describe('normalise', () => {
  it('scales, centres and grounds a tree of matrixAutoUpdate=false meshes', () => {
    // 100 m building whose raw bbox is nowhere near the origin: x/z centred at
    // (+50, +50), floor floating at y=+10 — like an IFC exported off-origin.
    const model = new THREE.Group();
    model.add(bakedMesh(100, [50, 60, 50]));
    // Emulate the parse pipeline: matrices were composed once at build time,
    // BEFORE any normalisation, so a lazy re-measure would read this state.
    model.updateMatrixWorld(true);

    const group = new THREE.Group();
    group.add(model);
    normalise(group);

    const box = worldBox(group);
    const size = box.getSize(new THREE.Vector3());
    const centre = box.getCenter(new THREE.Vector3());
    // Largest dimension lands on the normalised size…
    expect(Math.max(size.x, size.y, size.z)).toBeCloseTo(NORMALISED_DIM, 3);
    // …centred on x/z…
    expect(centre.x).toBeCloseTo(0, 3);
    expect(centre.z).toBeCloseTo(0, 3);
    // …and resting ON the ground, not floating at raw-units altitude.
    expect(box.min.y).toBeCloseTo(0, 3);
  });

  it('rests on a plausible in-model ground line instead of the bbox floor', () => {
    // Bbox floor at -3 m (foundations); the file's own elevation-0 is 0 m.
    const model = new THREE.Group();
    model.add(bakedMesh(20, [0, 7, 0])); // raw y-range: -3 … 17
    model.updateMatrixWorld(true);
    const group = new THREE.Group();
    group.add(model);
    normalise(group, { groundY: 0 });

    const box = worldBox(group);
    const s = NORMALISED_DIM / 20;
    // The foundations poke BELOW y=0 by their real (scaled) depth.
    expect(box.min.y).toBeCloseTo(-3 * s, 3);
    expect(box.max.y).toBeCloseTo(17 * s, 3);
  });

  it('leaves the returned tree composed for measure-before-render consumers', () => {
    const model = new THREE.Group();
    model.add(bakedMesh(100, [50, 60, 50]));
    model.updateMatrixWorld(true);
    const group = new THREE.Group();
    group.add(model);
    normalise(group);

    // No render and no explicit compose here: a consumer measuring the clone
    // straight after loadModel() must already see the normalised transforms.
    const clone = group.clone(true);
    const box = new THREE.Box3().setFromObject(clone);
    const size = box.getSize(new THREE.Vector3());
    expect(Math.max(size.x, size.y, size.z)).toBeCloseTo(NORMALISED_DIM, 3);
  });
});
