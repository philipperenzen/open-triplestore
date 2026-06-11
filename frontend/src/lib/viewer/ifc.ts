// IFC model loading via web-ifc (WASM). Parses an IFC STEP file into three.js
// meshes with the IFC GlobalId stamped on every mesh's userData — so viewers
// can pick individual "atoms" (a beam, a floor slab) and link them back to the
// element's linked-data IRI (the viewer feed exposes the same GlobalId as
// `ifc_guid`). The heavy WASM engine and the parsed model are both cached, so
// the building view and any number of per-element panels share one parse.
//
// The wasm binary is served as a static file from /wasm/web-ifc.wasm (copied
// from node_modules — Vite's hashed asset names would break web-ifc's
// path-based loader).

import * as THREE from 'three';

interface ParsedIfc {
  /** Master group — never added to a scene; callers receive clones. */
  master: THREE.Group;
  /** All 22-char GlobalIds that own at least one mesh. */
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

async function parseIfc(url: string): Promise<ParsedIfc> {
  let p = parseCache.get(url);
  if (!p) {
    p = (async () => {
      const [api, res] = await Promise.all([engine(), fetch(url)]);
      if (!res.ok) throw new Error(`IFC fetch failed: ${res.status}`);
      const buffer = new Uint8Array(await res.arrayBuffer());
      const modelID = api.OpenModel(buffer, { COORDINATE_TO_ORIGIN: true });
      try {
        const master = new THREE.Group();
        const guids = new Set<string>();
        const guidByExpress = new Map<number, string>();
        const materials = new Map<string, THREE.Material>();

        const materialFor = (c: { x: number; y: number; z: number; w: number }) => {
          const key = `${c.x.toFixed(3)}:${c.y.toFixed(3)}:${c.z.toFixed(3)}:${c.w.toFixed(3)}`;
          let m = materials.get(key);
          if (!m) {
            m = new THREE.MeshStandardMaterial({
              color: new THREE.Color(c.x, c.y, c.z),
              opacity: c.w,
              transparent: c.w < 0.999,
              roughness: 0.85,
              side: THREE.DoubleSide,
            });
            materials.set(key, m);
          }
          return m;
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

        api.StreamAllMeshes(modelID, (mesh: any) => {
          const guid = guidOf(mesh.expressID);
          const placed = mesh.geometries;
          for (let i = 0; i < placed.size(); i++) {
            const pg = placed.get(i);
            const geom = api.GetGeometry(modelID, pg.geometryExpressID);
            const verts = api.GetVertexArray(geom.GetVertexData(), geom.GetVertexDataSize());
            const idx = api.GetIndexArray(geom.GetIndexData(), geom.GetIndexDataSize());
            // web-ifc interleaves position+normal as 6 floats per vertex.
            const count = verts.length / 6;
            const pos = new Float32Array(count * 3);
            const nrm = new Float32Array(count * 3);
            for (let v = 0; v < count; v++) {
              pos[v * 3] = verts[v * 6];
              pos[v * 3 + 1] = verts[v * 6 + 1];
              pos[v * 3 + 2] = verts[v * 6 + 2];
              nrm[v * 3] = verts[v * 6 + 3];
              nrm[v * 3 + 1] = verts[v * 6 + 4];
              nrm[v * 3 + 2] = verts[v * 6 + 5];
            }
            const g = new THREE.BufferGeometry();
            g.setAttribute('position', new THREE.BufferAttribute(pos, 3));
            g.setAttribute('normal', new THREE.BufferAttribute(nrm, 3));
            g.setIndex(new THREE.BufferAttribute(new Uint32Array(idx), 1));
            const m = new THREE.Mesh(g, materialFor(pg.color));
            m.applyMatrix4(new THREE.Matrix4().fromArray(pg.flatTransformation));
            m.userData.expressID = mesh.expressID;
            if (guid) {
              m.userData.ifcGuid = guid;
              guids.add(guid);
            }
            master.add(m);
            geom.delete();
          }
        });

        // IFC is Z-up; three.js is Y-up.
        master.rotation.x = -Math.PI / 2;
        master.updateMatrixWorld(true);
        return { master, guids };
      } finally {
        api.CloseModel(modelID);
      }
    })();
    parseCache.set(url, p);
    p.catch(() => parseCache.delete(url));
  }
  return p;
}

/**
 * Load an IFC model (or one element of it, when the URL carries a `#GlobalId`
 * fragment) as a fresh three.js group. Clones share geometry with the cached
 * master parse, so repeated loads are cheap.
 */
export async function loadIfcGroup(url: string): Promise<THREE.Group> {
  const base = ifcBaseUrl(url);
  const guid = ifcGuidFragment(url);
  const { master, guids } = await parseIfc(base);
  // An element fragment isolates that element's meshes — unless the element has
  // no geometry of its own (e.g. a storey), in which case show the whole model.
  if (guid && guids.has(guid)) {
    const sub = new THREE.Group();
    for (const child of master.children) {
      if ((child as THREE.Mesh).userData?.ifcGuid === guid) {
        sub.add(child.clone());
      }
    }
    sub.rotation.copy(master.rotation);
    return sub;
  }
  return master.clone();
}
