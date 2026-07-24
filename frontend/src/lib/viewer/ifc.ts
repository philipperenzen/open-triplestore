// IFC model loading via web-ifc (WASM). Parses an IFC STEP file into a small set
// of MERGED three.js meshes — one per material — instead of one mesh per
// geometry. A 49 MB building has thousands of geometries; rendering them as
// thousands of separate meshes means thousands of draw calls per frame, which is
// the dominant cause of orbit jank. Merging by material collapses that to a
// handful of draws while a per-triangle `guidRanges` map preserves per-element
// picking (raycast → faceIndex → IFC GlobalId), and `subGeometryForGuids` rebuilds
// just one element/subtree on demand (the modal's isolation + the map's x-ray
// "selected" overlay). Every viewer links a picked atom back to its linked-data
// IRI (the viewer feed exposes the same GlobalId as `ifc_guid`).
//
// Parsing runs in a Web Worker (`ifcWorker.ts`) so the fetch + WASM parse +
// merge + BVH build never freeze the page; the worker transfers the merged
// typed arrays back zero-copy and this module only assembles three.js objects.
// The in-thread `parseIfcOnMainThread` path is kept as a fallback for
// environments without module workers. The worker also prebuilds a
// three-mesh-bvh bounds tree per merged geometry: raycasts against a merged
// building test EVERY triangle without one, which made the walkthrough's
// crosshair pick and map clicks O(building) — with the BVH they are O(log n).
//
// The wasm binary is served as a static file from /wasm/web-ifc.wasm (copied
// from node_modules — Vite's hashed asset names would break web-ifc's
// path-based loader).

import * as THREE from 'three';
import { MeshBVH, acceleratedRaycast } from 'three-mesh-bvh';
import { writable } from 'svelte/store';

// Route every mesh raycast through three-mesh-bvh: meshes whose geometry has a
// `boundsTree` use the BVH; all others fall through to three's stock raycast.
// Prototype-level so clones (every viewer clones the cached master) keep it.
THREE.Mesh.prototype.raycast = acceleratedRaycast;

/** Live parse progress of the IFC currently loading (null when idle):
 *  `{ url, phase: 'fetch'|'parse', loaded, total }` — total is 0 when unknown. */
export const ifcProgress = writable<{
  url: string;
  phase: 'fetch' | 'parse';
  loaded: number;
  total: number;
} | null>(null);

/** One element's run of triangles within a merged geometry's index buffer. */
export interface GuidRange {
  /** First index (into the geometry's index buffer) of this run. */
  start: number;
  /** Number of indices in the run (3 per triangle). */
  count: number;
  /** The element's IFC GlobalId (empty for non-rooted geometry). */
  guid: string;
}

interface ParsedIfc {
  /** Master group of MERGED per-material meshes; callers receive clones. Each
   *  mesh's geometry carries `userData.guidRanges` for picking + extraction. */
  master: THREE.Group;
  /** All 22-char GlobalIds that own at least one triangle. */
  guids: Set<string>;
}

let enginePromise: Promise<any> | null = null;

async function engine(): Promise<any> {
  if (!enginePromise) {
    enginePromise = (async () => {
      const WebIFC = await import('web-ifc');
      const api = new WebIFC.IfcAPI();
      api.SetWasmPath('/wasm/', true);
      await api.Init();
      return api;
    })();
    enginePromise.catch(() => (enginePromise = null));
  }
  return enginePromise;
}

const parseCache = new Map<string, Promise<ParsedIfc>>();

/** Strip a `#GlobalId` fragment from an IFC file URL. */
export function ifcBaseUrl(url: string): string {
  const at = url.indexOf('#');
  return at === -1 ? url : url.slice(0, at);
}

/** The `#GlobalId` fragment of an IFC file URL, if any. */
export function ifcGuidFragment(url: string): string | null {
  const at = url.indexOf('#');
  const frag = at === -1 ? '' : url.slice(at + 1);
  return frag.length === 22 ? frag : null;
}

/** Resolve the IFC GlobalId at a raycast hit: a per-element mesh carries it on
 *  `userData.ifcGuid`; a merged mesh resolves it from the picked triangle via the
 *  geometry's `guidRanges`. Returns null when the hit owns no GlobalId. */
export function ifcGuidAt(mesh: any, faceIndex: number | null | undefined): string | null {
  const direct = mesh?.userData?.ifcGuid;
  if (direct) return direct;
  const ranges: GuidRange[] | undefined = mesh?.geometry?.userData?.guidRanges;
  if (!ranges || faceIndex == null) return null;
  const off = faceIndex * 3;
  // Ranges are in index order → binary-search the last range starting at/below off.
  let lo = 0;
  let hi = ranges.length - 1;
  let ans = -1;
  while (lo <= hi) {
    const mid = (lo + hi) >> 1;
    if (ranges[mid].start <= off) {
      ans = mid;
      lo = mid + 1;
    } else {
      hi = mid - 1;
    }
  }
  if (ans < 0) return null;
  const r = ranges[ans];
  return off < r.start + r.count ? r.guid || null : null;
}

/** Does a group of (possibly merged) meshes own a triangle for any of these guids? */
export function groupHasGuid(group: THREE.Object3D, wanted: Set<string>): boolean {
  let found = false;
  group.traverse((n: any) => {
    if (found || !(n as THREE.Mesh).isMesh) return;
    const direct = n.userData?.ifcGuid;
    if (direct && wanted.has(direct)) {
      found = true;
      return;
    }
    const ranges: GuidRange[] | undefined = n.geometry?.userData?.guidRanges;
    if (ranges) {
      for (const r of ranges) {
        if (r.guid && wanted.has(r.guid)) {
          found = true;
          return;
        }
      }
    }
  });
  return found;
}

/** One element-geometry accumulated for a material bucket before merging. */
interface Piece {
  pos: Float32Array;
  nrm: Float32Array;
  idx: Uint32Array;
  guid: string;
}

/** The one place IFC surface materials are made — worker + fallback paths agree.
 *  Transparent surfaces (glass) don't write depth: with depthWrite on, whichever
 *  pane rendered first blanked the geometry behind it, the classic "milky
 *  windows" artifact that made buildings look wrong. */
export function ifcSurfaceMaterial(r: number, g: number, b: number, a: number): THREE.Material {
  const transparent = a < 0.999;
  return new THREE.MeshStandardMaterial({
    color: new THREE.Color(r, g, b),
    opacity: a,
    transparent,
    depthWrite: !transparent,
    roughness: 0.8,
    metalness: 0.05,
    side: THREE.DoubleSide,
  });
}

/** Assemble the worker's transferred buckets into the merged master group. */
function assembleParsed(msg: {
  buckets: Array<{
    color: [number, number, number, number];
    pos: Float32Array;
    nrm: Float32Array;
    idx: Uint32Array;
    ranges: GuidRange[];
    bvh: { roots: ArrayBuffer[]; index: unknown; indirectBuffer: unknown } | null;
  }>;
  guids: string[];
}): ParsedIfc {
  const master = new THREE.Group();
  for (const b of msg.buckets) {
    const g = new THREE.BufferGeometry();
    g.setAttribute('position', new THREE.BufferAttribute(b.pos, 3));
    g.setAttribute('normal', new THREE.BufferAttribute(b.nrm, 3));
    g.setIndex(new THREE.BufferAttribute(b.idx, 1));
    (g.userData as { guidRanges?: GuidRange[] }).guidRanges = b.ranges;
    if (b.bvh) {
      try {
        (g as unknown as { boundsTree: MeshBVH }).boundsTree = MeshBVH.deserialize(b.bvh, g, {
          setIndex: false, // indirect build — the index was never modified
        });
      } catch {
        /* no BVH → raycasts fall back to plain three.js */
      }
    }
    const [r, gc, bc, a] = b.color;
    const m = new THREE.Mesh(g, ifcSurfaceMaterial(r, gc, bc, a));
    m.matrixAutoUpdate = false; // identity, transforms baked into vertices
    master.add(m);
  }
  master.updateMatrixWorld(true);
  return { master, guids: new Set(msg.guids) };
}

/** Parse in the dedicated worker; one worker per file, terminated when done so
 *  the WASM heap (hundreds of MB for a big model) is released immediately. */
function parseIfcInWorker(url: string): Promise<ParsedIfc> {
  return new Promise((resolve, reject) => {
    let worker: Worker;
    try {
      worker = new Worker(new URL('./ifcWorker.ts', import.meta.url), { type: 'module' });
    } catch (e) {
      reject(e);
      return;
    }
    const finish = () => {
      worker.terminate();
      ifcProgress.set(null);
    };
    worker.onerror = (e) => {
      finish();
      reject(new Error(e.message || 'IFC worker failed'));
    };
    worker.onmessage = (ev) => {
      const msg = ev.data || {};
      if (msg.type === 'progress') {
        ifcProgress.set({ url, phase: msg.phase, loaded: msg.loaded, total: msg.total });
      } else if (msg.type === 'error') {
        finish();
        reject(new Error(msg.message));
      } else if (msg.type === 'done') {
        try {
          resolve(assembleParsed(msg));
        } catch (e) {
          reject(e);
        } finally {
          finish();
        }
      }
    };
    worker.postMessage({ url });
  });
}

async function parseIfc(url: string): Promise<ParsedIfc> {
  let p = parseCache.get(url);
  if (!p) {
    p = parseIfcInWorker(url).catch((e) => {
      // eslint-disable-next-line no-console
      console.warn('IFC worker unavailable — parsing on the main thread', e);
      return parseIfcOnMainThread(url);
    });
    parseCache.set(url, p);
    p.catch(() => parseCache.delete(url));
  }
  return p;
}

/** In-thread fallback parse (identical output, no BVH; freezes the UI while it
 *  runs, so it is only used when the worker cannot start). */
async function parseIfcOnMainThread(url: string): Promise<ParsedIfc> {
  return (async () => {
      const [api, res] = await Promise.all([engine(), fetch(url)]);
      if (!res.ok) throw new Error(`IFC fetch failed: ${res.status}`);
      const buffer = new Uint8Array(await res.arrayBuffer());
      const modelID = api.OpenModel(buffer, { COORDINATE_TO_ORIGIN: true });
      try {
        const guids = new Set<string>();
        const guidByExpress = new Map<number, string>();
        // One bucket per material colour — every piece in a bucket merges into one
        // mesh sharing that material.
        const buckets = new Map<
          string,
          { material: THREE.Material; pieces: Piece[]; verts: number; inds: number }
        >();

        const materialFor = (c: { x: number; y: number; z: number; w: number }) => {
          const key = `${c.x.toFixed(3)}:${c.y.toFixed(3)}:${c.z.toFixed(3)}:${c.w.toFixed(3)}`;
          let b = buckets.get(key);
          if (!b) {
            b = {
              material: ifcSurfaceMaterial(c.x, c.y, c.z, c.w),
              pieces: [],
              verts: 0,
              inds: 0,
            };
            buckets.set(key, b);
          }
          return b;
        };

        const guidOf = (expressID: number): string => {
          let g = guidByExpress.get(expressID);
          if (g === undefined) {
            g = '';
            try {
              const line = api.GetLine(modelID, expressID);
              const v = line?.GlobalId?.value;
              if (typeof v === 'string' && v.length === 22) g = v;
            } catch {
              /* non-rooted entity — no GlobalId */
            }
            guidByExpress.set(expressID, g);
          }
          return g;
        };

        const m4 = new THREE.Matrix4();
        const nm = new THREE.Matrix3();
        const vp = new THREE.Vector3();
        const vn = new THREE.Vector3();

        api.StreamAllMeshes(modelID, (mesh: any) => {
          const guid = guidOf(mesh.expressID);
          const placed = mesh.geometries;
          for (let i = 0; i < placed.size(); i++) {
            const pg = placed.get(i);
            const geom = api.GetGeometry(modelID, pg.geometryExpressID);
            const verts = api.GetVertexArray(geom.GetVertexData(), geom.GetVertexDataSize());
            const idx = api.GetIndexArray(geom.GetIndexData(), geom.GetIndexDataSize());
            const count = verts.length / 6; // web-ifc interleaves position+normal
            if (count === 0 || idx.length === 0) {
              geom.delete();
              continue;
            }
            // Bake the element's placement into the vertices so the merged mesh can
            // use an identity transform (the placement no longer needs per-mesh state).
            m4.fromArray(pg.flatTransformation);
            nm.getNormalMatrix(m4);
            const pos = new Float32Array(count * 3);
            const nrm = new Float32Array(count * 3);
            for (let v = 0; v < count; v++) {
              vp.set(verts[v * 6], verts[v * 6 + 1], verts[v * 6 + 2]).applyMatrix4(m4);
              vn.set(verts[v * 6 + 3], verts[v * 6 + 4], verts[v * 6 + 5])
                .applyMatrix3(nm)
                .normalize();
              pos[v * 3] = vp.x;
              pos[v * 3 + 1] = vp.y;
              pos[v * 3 + 2] = vp.z;
              nrm[v * 3] = vn.x;
              nrm[v * 3 + 1] = vn.y;
              nrm[v * 3 + 2] = vn.z;
            }
            const b = materialFor(pg.color);
            b.pieces.push({ pos, nrm, idx: new Uint32Array(idx), guid });
            b.verts += count;
            b.inds += idx.length;
            if (guid) guids.add(guid);
            geom.delete();
          }
        });

        // Concatenate each bucket's pieces into one merged geometry + guidRanges.
        const master = new THREE.Group();
        for (const b of buckets.values()) {
          if (b.verts === 0) continue;
          const pos = new Float32Array(b.verts * 3);
          const nrm = new Float32Array(b.verts * 3);
          const idx = new Uint32Array(b.inds);
          const ranges: GuidRange[] = [];
          let vBase = 0;
          let iOff = 0;
          for (const piece of b.pieces) {
            pos.set(piece.pos, vBase * 3);
            nrm.set(piece.nrm, vBase * 3);
            for (let k = 0; k < piece.idx.length; k++) idx[iOff + k] = vBase + piece.idx[k];
            ranges.push({ start: iOff, count: piece.idx.length, guid: piece.guid });
            vBase += piece.pos.length / 3;
            iOff += piece.idx.length;
          }
          const g = new THREE.BufferGeometry();
          g.setAttribute('position', new THREE.BufferAttribute(pos, 3));
          g.setAttribute('normal', new THREE.BufferAttribute(nrm, 3));
          g.setIndex(new THREE.BufferAttribute(idx, 1));
          g.userData.guidRanges = ranges;
          const m = new THREE.Mesh(g, b.material);
          m.matrixAutoUpdate = false; // identity, transforms baked into vertices
          master.add(m);
          // Release this bucket's per-element source arrays now that they're merged
          // — keeps the transient peak near 1× instead of holding pieces + merged
          // for the whole 49 MB building at once.
          b.pieces.length = 0;
        }
        master.updateMatrixWorld(true);
        return { master, guids };
      } finally {
        api.CloseModel(modelID);
      }
    })();
}

/**
 * Build a group containing ONLY the triangles of `wanted` guids, extracted from a
 * group of merged meshes. Vertices are remapped so the result is compact (one
 * element is a tiny mesh, not the whole building's buffer). Used for the modal's
 * isolation and the map's x-ray "selected" overlay. A sub-mesh of a single guid
 * carries `userData.ifcGuid`; a multi-guid one keeps remapped `guidRanges`.
 */
export function subGeometryForGuids(group: THREE.Object3D, wanted: Set<string>): THREE.Group {
  const out = new THREE.Group();
  group.traverse((node: any) => {
    const mesh = node as THREE.Mesh;
    if (!mesh.isMesh) return;
    const ranges: GuidRange[] | undefined = (mesh.geometry as any)?.userData?.guidRanges;
    const geom = mesh.geometry as THREE.BufferGeometry;
    if (!ranges) {
      // Per-element mesh (already isolated) — keep if its guid is wanted.
      if (mesh.userData?.ifcGuid && wanted.has(mesh.userData.ifcGuid)) {
        out.add(mesh.clone());
      }
      return;
    }
    const keep = ranges.filter((r) => r.guid && wanted.has(r.guid));
    if (!keep.length) return;
    const srcPos = geom.attributes.position.array as ArrayLike<number>;
    const srcNrm = (geom.attributes.normal?.array as ArrayLike<number>) || null;
    const srcIdx = geom.index!.array as ArrayLike<number>;
    const remap = new Map<number, number>();
    const pos: number[] = [];
    const nrm: number[] = [];
    const idx: number[] = [];
    const subRanges: GuidRange[] = [];
    for (const r of keep) {
      const rangeStart = idx.length;
      for (let k = r.start; k < r.start + r.count; k++) {
        const old = srcIdx[k];
        let ni = remap.get(old);
        if (ni === undefined) {
          ni = pos.length / 3;
          remap.set(old, ni);
          pos.push(srcPos[old * 3], srcPos[old * 3 + 1], srcPos[old * 3 + 2]);
          if (srcNrm) nrm.push(srcNrm[old * 3], srcNrm[old * 3 + 1], srcNrm[old * 3 + 2]);
        }
        idx.push(ni);
      }
      subRanges.push({ start: rangeStart, count: r.count, guid: r.guid });
    }
    const sg = new THREE.BufferGeometry();
    sg.setAttribute('position', new THREE.Float32BufferAttribute(pos, 3));
    sg.setIndex(idx);
    // Merged source geometry always carries normals (parseIfc bakes them); the
    // fallback runs only for a hypothetical normal-less source, and needs the
    // index set first so it computes per the actual (remapped) triangles.
    if (nrm.length) sg.setAttribute('normal', new THREE.Float32BufferAttribute(nrm, 3));
    else sg.computeVertexNormals();
    const uniq = new Set(keep.map((r) => r.guid));
    const sm = new THREE.Mesh(sg, mesh.material);
    sm.matrixAutoUpdate = false;
    if (uniq.size === 1) sm.userData.ifcGuid = [...uniq][0];
    else sg.userData.guidRanges = subRanges;
    out.add(sm);
  });
  return out;
}

/**
 * Load an IFC model (or one element/subtree of it, when the URL carries a
 * `#GlobalId` fragment or `opts.guids`) as a fresh three.js group. The
 * whole-building clone shares the merged geometry with the cache; an isolation
 * extracts just the requested elements.
 */
export async function loadIfcGroup(
  url: string,
  opts: { guids?: string[] } = {},
): Promise<THREE.Group> {
  const base = ifcBaseUrl(url);
  const fragGuid = ifcGuidFragment(url);
  const { master, guids } = await parseIfc(base);
  // Decide which GlobalIds to isolate. A `#GlobalId` fragment isolates that one
  // element (a wall, a beam). `opts.guids` — a spatial container's descendant
  // leaf elements — isolates a whole subtree, which is how a storey/building is
  // shown: spatial containers own NO mesh of their own, so isolating *their* guid
  // is futile; isolating their descendants is what makes their contents visible.
  // Only requested guids that actually own meshes are kept.
  const wanted = new Set<string>();
  if (fragGuid && guids.has(fragGuid)) wanted.add(fragGuid);
  for (const g of opts.guids || []) if (guids.has(g)) wanted.add(g);
  // Nothing renderable requested (the whole-building ref, or a bare container ref
  // with no descendant set) → the merged whole building (few draw calls).
  if (wanted.size === 0) return master.clone();
  return subGeometryForGuids(master, wanted);
}
