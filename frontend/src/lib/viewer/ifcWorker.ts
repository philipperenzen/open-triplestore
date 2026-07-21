/// <reference lib="webworker" />
// IFC parse worker: fetches the STEP file and runs web-ifc (WASM) + the
// per-material geometry merge + BVH build OFF the main thread. Parsing a
// 50 MB building used to freeze the whole UI (map pans, spinners, clicks) for
// seconds; here the main thread only assembles transferred typed arrays into
// three.js objects, which is milliseconds. The merged layout (one geometry per
// material colour, `guidRanges` runs into the index buffer) is identical to
// the in-thread fallback in `ifc.ts` — keep the two in sync.
//
// The BVH is built with `indirect: true` so the index buffer keeps its
// original order — `guidRanges` (triangle-run → IFC GlobalId) indexes into it,
// and the default BVH build would reorder the triangles out from under them.

import * as WebIFC from 'web-ifc';
import { BufferGeometry, BufferAttribute } from 'three';
import { MeshBVH } from 'three-mesh-bvh';

interface GuidRange {
  start: number;
  count: number;
  guid: string;
}

interface Piece {
  pos: Float32Array;
  nrm: Float32Array;
  idx: Uint32Array;
  guid: string;
}

interface Bucket {
  color: [number, number, number, number];
  pieces: Piece[];
  verts: number;
  inds: number;
}

const scope = self as unknown as DedicatedWorkerGlobalScope;

function post(msg: unknown, transfer?: Transferable[]) {
  scope.postMessage(msg, transfer || []);
}

/** Fetch the file, streaming download progress back to the page. */
async function fetchBytes(url: string): Promise<Uint8Array> {
  const res = await fetch(url);
  if (!res.ok) throw new Error(`IFC fetch failed: ${res.status}`);
  // Content-Length counts *encoded* bytes; when the transfer is compressed the
  // decoded stream overshoots it, so only trust it for identity transfers.
  const encoded = res.headers.get('content-encoding');
  const total = encoded ? 0 : Number(res.headers.get('content-length')) || 0;
  if (!res.body) return new Uint8Array(await res.arrayBuffer());
  const reader = res.body.getReader();
  const chunks: Uint8Array[] = [];
  let loaded = 0;
  let lastPost = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    chunks.push(value);
    loaded += value.byteLength;
    const now = Date.now();
    if (now - lastPost > 150) {
      lastPost = now;
      post({ type: 'progress', phase: 'fetch', loaded, total });
    }
  }
  const out = new Uint8Array(loaded);
  let off = 0;
  for (const c of chunks) {
    out.set(c, off);
    off += c.byteLength;
  }
  return out;
}

async function parse(url: string) {
  const [api, buffer] = await Promise.all([
    (async () => {
      const engine = new WebIFC.IfcAPI();
      // Absolute path: the wasm binary is a static file (see /public/wasm) —
      // same base the main-thread engine uses.
      engine.SetWasmPath('/wasm/', true);
      await engine.Init();
      return engine;
    })(),
    fetchBytes(url),
  ]);

  post({ type: 'progress', phase: 'parse', loaded: 0, total: 0 });
  const modelID = api.OpenModel(buffer, { COORDINATE_TO_ORIGIN: true });
  try {
    const guids = new Set<string>();
    const guidByExpress = new Map<number, string>();
    const buckets = new Map<string, Bucket>();

    const bucketFor = (c: { x: number; y: number; z: number; w: number }): Bucket => {
      const key = `${c.x.toFixed(3)}:${c.y.toFixed(3)}:${c.z.toFixed(3)}:${c.w.toFixed(3)}`;
      let b = buckets.get(key);
      if (!b) {
        b = { color: [c.x, c.y, c.z, c.w], pieces: [], verts: 0, inds: 0 };
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

    let meshCount = 0;
    let lastPost = 0;
    api.StreamAllMeshes(modelID, (mesh: WebIFC.FlatMesh) => {
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
        // Bake the placement into the vertices (column-major 4×4) so the merged
        // mesh renders with an identity transform.
        const e = pg.flatTransformation;
        // Normal matrix = inverse-transpose of the upper-left 3×3 = cofactor
        // matrix / det. Normals are re-normalised, so only det's SIGN matters
        // (mirrored placements must keep normals outward).
        const c00 = e[5] * e[10] - e[6] * e[9];
        const c01 = e[6] * e[8] - e[4] * e[10];
        const c02 = e[4] * e[9] - e[5] * e[8];
        const c10 = e[2] * e[9] - e[1] * e[10];
        const c11 = e[0] * e[10] - e[2] * e[8];
        const c12 = e[1] * e[8] - e[0] * e[9];
        const c20 = e[1] * e[6] - e[2] * e[5];
        const c21 = e[2] * e[4] - e[0] * e[6];
        const c22 = e[0] * e[5] - e[1] * e[4];
        const det = e[0] * c00 + e[1] * c01 + e[2] * c02;
        const s = det < 0 ? -1 : 1;
        const pos = new Float32Array(count * 3);
        const nrm = new Float32Array(count * 3);
        for (let v = 0; v < count; v++) {
          const x = verts[v * 6];
          const y = verts[v * 6 + 1];
          const z = verts[v * 6 + 2];
          pos[v * 3] = e[0] * x + e[4] * y + e[8] * z + e[12];
          pos[v * 3 + 1] = e[1] * x + e[5] * y + e[9] * z + e[13];
          pos[v * 3 + 2] = e[2] * x + e[6] * y + e[10] * z + e[14];
          const nx = verts[v * 6 + 3];
          const ny = verts[v * 6 + 4];
          const nz = verts[v * 6 + 5];
          let ox = (c00 * nx + c10 * ny + c20 * nz) * s;
          let oy = (c01 * nx + c11 * ny + c21 * nz) * s;
          let oz = (c02 * nx + c12 * ny + c22 * nz) * s;
          const len = Math.sqrt(ox * ox + oy * oy + oz * oz) || 1;
          ox /= len;
          oy /= len;
          oz /= len;
          nrm[v * 3] = ox;
          nrm[v * 3 + 1] = oy;
          nrm[v * 3 + 2] = oz;
        }
        const b = bucketFor(pg.color);
        b.pieces.push({ pos, nrm, idx: new Uint32Array(idx), guid });
        b.verts += count;
        b.inds += idx.length;
        if (guid) guids.add(guid);
        geom.delete();
      }
      meshCount += 1;
      const now = Date.now();
      if (now - lastPost > 250) {
        lastPost = now;
        post({ type: 'progress', phase: 'parse', loaded: meshCount, total: 0 });
      }
    });

    // Merge each bucket's pieces into one geometry + guidRanges, then build the
    // raycast BVH here (it is the second-most expensive step after parsing).
    const outBuckets = [];
    const transfer = new Set<Transferable>();
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
      b.pieces.length = 0; // release per-element arrays before the BVH allocates

      let bvh = null;
      try {
        const g = new BufferGeometry();
        g.setAttribute('position', new BufferAttribute(pos, 3));
        g.setIndex(new BufferAttribute(idx, 1));
        // `indirect` keeps the index buffer untouched (guidRanges stay valid);
        // serialize without cloning — the buffers transfer to the page.
        const tree = new MeshBVH(g, { indirect: true });
        bvh = MeshBVH.serialize(tree, { cloneBuffers: false });
      } catch {
        bvh = null; // raycasting falls back to plain three.js on the page
      }

      outBuckets.push({ color: b.color, pos, nrm, idx, ranges, bvh });
      transfer.add(pos.buffer);
      transfer.add(nrm.buffer);
      transfer.add(idx.buffer);
      if (bvh) {
        for (const r of bvh.roots) transfer.add(r);
        if (bvh.indirectBuffer) transfer.add(bvh.indirectBuffer.buffer);
        if (bvh.index) transfer.add(bvh.index.buffer); // usually === idx.buffer (deduped by the Set)
      }
    }
    post({ type: 'done', buckets: outBuckets, guids: [...guids] }, [...transfer]);
  } finally {
    api.CloseModel(modelID);
  }
}

scope.onmessage = (ev: MessageEvent<{ url: string }>) => {
  parse(ev.data.url).catch((e) => {
    post({ type: 'error', message: e?.message || 'IFC parse failed' });
  });
};
