// CityJSON / CityGML → three.js meshes for the dataset viewers.
//
// CityJSON (cityjson.org, the JSON encoding of CityGML) is parsed in full for
// the surface types that matter to a viewer: MultiSurface / CompositeSurface /
// Solid / MultiSolid / CompositeSolid boundaries at the highest available LoD,
// coloured by semantic surface (roof / wall / ground / window …) so LoD2 models
// read like buildings rather than grey blobs. CityGML (the XML encoding) gets a
// best-effort treatment: every gml:Polygon, semantics and LoD from ancestor
// element names. Vertices are reprojected from the model's CRS (proj4, see
// ./crs) into local east/up/south metres around a WGS84 anchor, so the map can
// place the model georeferenced and to scale.

import * as THREE from 'three';
import { epsgFromReference, toLonLat, lonLatToLocalMeters } from './crs';

export interface CityModel {
  /** y-up group in local metres: x = east, y = up, z = south. Rests on y = 0. */
  group: THREE.Group;
  /** WGS84 anchor of the model's footprint centre; null when the CRS is unknown. */
  anchorLonLat: [number, number] | null;
  /** Bounding-box size of the built scene, in metres (where the CRS is known). */
  sizeMeters: { x: number; y: number; z: number };
  objectCount: number;
  triangleCount: number;
  /** One entry per top-level CityObject that contributed geometry — the metadata
   *  a picked building resolves to (BAG id, function, storeys …). */
  objects: CityObject[];
}

/** A top-level CityObject (Building, Bridge, …) — a *selectable* entity in a
 *  multi-object CityJSON block. Nested parts (BuildingPart) fold into their root. */
export interface CityObject {
  /** CityObject key (the root; a BuildingPart resolves to its parent Building). */
  id: string;
  /** Display label — `identificatie` attribute where present, else the id. */
  label: string;
  /** CityObject `type` (e.g. "Building"). */
  type: string | null;
  /** The object's CityJSON `attributes` (function, storeysAboveGround, …). */
  attributes: Record<string, unknown>;
}

/** One CityObject's run of vertices within a merged (non-indexed) geometry — the
 *  CityJSON analogue of an IFC {@link import('./ifc').GuidRange}, in *vertex*
 *  units (each triangle = 3 consecutive vertices), so a raycast `faceIndex`
 *  resolves the picked building via {@link cityObjectIdAt}. */
export interface CityObjectRange {
  /** First vertex index of the run. */
  start: number;
  /** Number of vertices in the run (3 per triangle). */
  count: number;
  /** The (root) CityObject id that owns these triangles. */
  objectId: string;
}

type V3 = [number, number, number];

/** One polygon (outer ring + holes) in source coordinates, with its semantics
 *  and the (root) CityObject it belongs to (for per-building picking). */
interface SemPolygon {
  rings: V3[][];
  semantic: string | null;
  objectType: string | null;
  objectId: string | null;
}

/** Keep a hostile/huge file from freezing the tab. */
const MAX_TRIANGLES = 500_000;

// ── Colours ──────────────────────────────────────────────────────────────────

/** CityGML semantic surface → colour. */
const SURFACE_COLORS: Record<string, number> = {
  RoofSurface: 0xb0563c,
  WallSurface: 0xded7c9,
  GroundSurface: 0x8d9b84,
  Window: 0x8fb6d8,
  Door: 0x9a7b52,
  ClosureSurface: 0xb9b9b9,
  OuterCeilingSurface: 0xcfc8ba,
  OuterFloorSurface: 0xb6b0a4,
  WaterSurface: 0x5d93b8,
  TrafficArea: 0x60666e,
  AuxiliaryTrafficArea: 0x7c8a6e,
};

/** CityObject type (prefix-matched) → fallback colour when a surface has no semantics. */
const OBJECT_COLORS: [string, number][] = [
  ['Building', 0xd9d2c4],
  ['Bridge', 0xa9a9ad],
  ['Road', 0x595f66],
  ['Railway', 0x6b6f76],
  ['TransportSquare', 0x686e75],
  ['WaterBody', 0x5d93b8],
  ['PlantCover', 0x6c8f57],
  ['SolitaryVegetationObject', 0x6c8f57],
  ['TINRelief', 0x97a08b],
  ['LandUse', 0x9aa78c],
  ['CityFurniture', 0x8b8f96],
  ['Tunnel', 0x90949b],
];

function colorFor(semantic: string | null, objectType: string | null): number {
  if (semantic && SURFACE_COLORS[semantic]) return SURFACE_COLORS[semantic];
  if (objectType) {
    for (const [prefix, color] of OBJECT_COLORS) {
      if (objectType.startsWith(prefix)) return color;
    }
  }
  return 0xa9adb3;
}

// ── Shared mesh building ─────────────────────────────────────────────────────

/** Newell's method — robust polygon normal for possibly non-planar rings. */
function newellNormal(points: V3[]): THREE.Vector3 {
  const n = new THREE.Vector3();
  for (let i = 0; i < points.length; i++) {
    const a = points[i];
    const b = points[(i + 1) % points.length];
    n.x += (a[1] - b[1]) * (a[2] + b[2]);
    n.y += (a[2] - b[2]) * (a[0] + b[0]);
    n.z += (a[0] - b[0]) * (a[1] + b[1]);
  }
  return n;
}

/** Triangulate one polygon (rings of 3D points) → flat triangle vertices. */
function triangulate(rings: V3[][]): V3[] {
  const outer = rings[0];
  if (!outer || outer.length < 3) return [];
  const normal = newellNormal(outer);
  if (normal.lengthSq() < 1e-12) return [];
  normal.normalize();
  // Build an orthonormal basis (u, v) on the polygon plane and project to 2D.
  const u = new THREE.Vector3();
  if (Math.abs(normal.z) > 0.9) u.set(1, 0, 0);
  else u.set(0, 0, 1);
  u.cross(normal).normalize();
  const v = new THREE.Vector3().crossVectors(normal, u);
  const to2d = (p: V3) =>
    new THREE.Vector2(p[0] * u.x + p[1] * u.y + p[2] * u.z, p[0] * v.x + p[1] * v.y + p[2] * v.z);

  const contour = outer.map(to2d);
  const holes = rings.slice(1).map((r) => r.map(to2d));
  let tris: number[][];
  try {
    tris = THREE.ShapeUtils.triangulateShape(contour, holes);
  } catch {
    return [];
  }
  const flat: V3[] = [...outer, ...rings.slice(1).flat()];
  const out: V3[] = [];
  for (const [a, b, c] of tris) {
    if (flat[a] && flat[b] && flat[c]) out.push(flat[a], flat[b], flat[c]);
  }
  return out;
}

/**
 * Localise + triangulate semantic polygons into a coloured, y-up group.
 * `convert` maps source (x, y) to [lon, lat]; null means "units are local metres".
 */
function buildCityModel(
  polygons: SemPolygon[],
  convert: ((xy: [number, number]) => [number, number]) | null,
  objects: CityObject[]
): CityModel {
  if (!polygons.length) throw new Error('no surface geometry found');

  // Anchor at the footprint centre, ground at the lowest vertex.
  let minX = Infinity, minY = Infinity, minH = Infinity;
  let maxX = -Infinity, maxY = -Infinity;
  for (const poly of polygons) {
    for (const ring of poly.rings) {
      for (const [x, y, h] of ring) {
        if (x < minX) minX = x;
        if (x > maxX) maxX = x;
        if (y < minY) minY = y;
        if (y > maxY) maxY = y;
        if (h < minH) minH = h;
      }
    }
  }
  if (!Number.isFinite(minX)) throw new Error('no finite coordinates');
  if (!Number.isFinite(minH)) minH = 0;

  const centre: [number, number] = [(minX + maxX) / 2, (minY + maxY) / 2];
  const anchorLonLat = convert ? convert(centre) : null;

  // Source (x=east-ish, y=north-ish, h=up) → scene (x=east, y=up, z=south).
  const toLocal = (p: V3): V3 => {
    if (convert && anchorLonLat) {
      const [east, north] = lonLatToLocalMeters(anchorLonLat, convert([p[0], p[1]]));
      return [east, p[2] - minH, -north];
    }
    return [p[0] - centre[0], p[2] - minH, -(p[1] - centre[1])];
  };

  // Triangulate into one position buffer per colour, recording — per colour
  // bucket — the contiguous vertex run each CityObject owns (`ranges`), so a
  // pick on the merged mesh resolves the specific building. Polygons arrive
  // grouped by CityObject (parseCityJSON iterates objects in order), so a
  // bucket's runs stay object-grouped and index-sorted.
  interface Bucket {
    positions: number[];
    ranges: CityObjectRange[];
  }
  const buckets = new Map<number, Bucket>();
  const contributed = new Set<string>();
  let triangleCount = 0;
  for (const poly of polygons) {
    if (triangleCount >= MAX_TRIANGLES) break;
    const tris = triangulate(poly.rings.map((r) => r.map(toLocal)));
    if (!tris.length) continue;
    const color = colorFor(poly.semantic, poly.objectType);
    let bucket = buckets.get(color);
    if (!bucket) buckets.set(color, (bucket = { positions: [], ranges: [] }));
    const startV = bucket.positions.length / 3; // vertex index before this poly
    for (const p of tris) bucket.positions.push(p[0], p[1], p[2]);
    if (poly.objectId) {
      contributed.add(poly.objectId);
      const last = bucket.ranges[bucket.ranges.length - 1];
      // Same object as the previous run in this bucket → extend it; else a new run.
      if (last && last.objectId === poly.objectId && last.start + last.count === startV) {
        last.count += tris.length;
      } else {
        bucket.ranges.push({ start: startV, count: tris.length, objectId: poly.objectId });
      }
    }
    triangleCount += tris.length / 3;
  }
  if (!buckets.size) throw new Error('no triangulatable surfaces');

  const group = new THREE.Group();
  for (const [color, bucket] of buckets) {
    const geom = new THREE.BufferGeometry();
    geom.setAttribute('position', new THREE.Float32BufferAttribute(bucket.positions, 3));
    geom.computeVertexNormals(); // non-indexed → per-face normals (flat look)
    if (bucket.ranges.length) {
      (geom.userData as { objectRanges?: CityObjectRange[] }).objectRanges = bucket.ranges;
    }
    const mat = new THREE.MeshStandardMaterial({
      color,
      roughness: 0.85,
      metalness: 0.0,
      side: THREE.DoubleSide, // ring winding varies per producer
    });
    group.add(new THREE.Mesh(geom, mat));
  }

  const box = new THREE.Box3().setFromObject(group);
  const size = box.getSize(new THREE.Vector3());
  // Only objects that actually produced triangles are selectable.
  const contributors = objects.filter((o) => contributed.has(o.id));
  return {
    group,
    anchorLonLat,
    sizeMeters: { x: size.x, y: size.y, z: size.z },
    objectCount: contributors.length || objects.length,
    triangleCount: Math.round(triangleCount),
    objects: contributors,
  };
}

// ── CityJSON ─────────────────────────────────────────────────────────────────

interface CityJsonGeometry {
  type?: string;
  lod?: string | number;
  boundaries?: unknown[];
  semantics?: { surfaces?: { type?: string }[]; values?: unknown[] };
}

/** Numeric LoD of a geometry ("2.2" → 2.2; absent → -1). */
const lodOf = (g: CityJsonGeometry): number => {
  const n = parseFloat(String(g.lod ?? ''));
  return Number.isFinite(n) ? n : -1;
};

/**
 * Parse a CityJSON document (1.0 – 2.0) into a [CityModel]. Throws with a
 * readable message when the document is not CityJSON or has no usable surfaces.
 *
 * `opts.only` isolates a subset of top-level CityObjects (by root id) — the
 * CityJSON analogue of an IFC `#GlobalId` fragment: one building's element (or a
 * zone's whole block) can be shown without the rest of a shared file.
 */
export function parseCityJSON(doc: unknown, opts: { only?: Set<string> } = {}): CityModel {
  const cj = doc as {
    type?: string;
    transform?: { scale?: number[]; translate?: number[] };
    vertices?: number[][];
    metadata?: { referenceSystem?: string };
    CityObjects?: Record<
      string,
      { type?: string; geometry?: CityJsonGeometry[]; parents?: string[]; attributes?: Record<string, unknown> }
    >;
  };
  if (!cj || cj.type !== 'CityJSON') throw new Error('not a CityJSON document');
  const [sx, sy, sz] = cj.transform?.scale ?? [1, 1, 1];
  const [tx, ty, tz] = cj.transform?.translate ?? [0, 0, 0];
  const vertices: V3[] = (cj.vertices ?? []).map((v) => [
    v[0] * sx + tx,
    v[1] * sy + ty,
    (v[2] ?? 0) * sz + tz,
  ]);

  const epsg = epsgFromReference(cj.metadata?.referenceSystem);
  const convert = epsg != null ? toLonLat(epsg) : null;

  // Resolve a CityObject to its ROOT (a BuildingPart with LoD2.2 geometry folds
  // into its parent Building, which carries the `identificatie`), so a pick on a
  // part selects the whole building. Metadata is taken from the root object.
  const cos = cj.CityObjects ?? {};
  const only = opts.only;
  const rootOf = (key: string): string => {
    let cur = key;
    const seen = new Set<string>([cur]);
    for (;;) {
      const p = cos[cur]?.parents?.[0];
      if (!p || !cos[p] || seen.has(p)) break;
      seen.add(p);
      cur = p;
    }
    return cur;
  };
  const objectMeta = new Map<string, CityObject>();
  const rootMeta = (rootKey: string): CityObject => {
    let m = objectMeta.get(rootKey);
    if (!m) {
      const o = cos[rootKey];
      const attributes = (o?.attributes as Record<string, unknown>) ?? {};
      const ident = attributes.identificatie;
      m = {
        id: rootKey,
        label: typeof ident === 'string' && ident ? ident : rootKey,
        type: o?.type ?? null,
        attributes,
      };
      objectMeta.set(rootKey, m);
    }
    return m;
  };

  const polygons: SemPolygon[] = [];
  const ringPoints = (ring: unknown): V3[] =>
    (Array.isArray(ring) ? ring : [])
      .map((i) => vertices[i as number])
      .filter((p): p is V3 => Array.isArray(p));

  for (const [key, obj] of Object.entries(cos)) {
    const geoms = obj.geometry ?? [];
    if (!geoms.length) continue;
    const root = rootOf(key);
    if (only && !only.has(root)) continue; // isolating a subset — skip the rest
    rootMeta(root);
    // Highest LoD only — LoD1 + LoD2 of the same building would z-fight.
    const best = Math.max(...geoms.map(lodOf));
    for (const geom of geoms.filter((g) => lodOf(g) === best)) {
      // Normalise nesting to a flat list of surfaces (+ aligned semantic values):
      // MultiSurface → boundaries, Solid → shells × surfaces, MultiSolid → +1 level.
      const surfaces: { rings: unknown[]; semantic: string | null }[] = [];
      const semTypes = geom.semantics?.surfaces?.map((s) => s?.type ?? null) ?? [];
      const semOf = (v: unknown): string | null =>
        typeof v === 'number' ? (semTypes[v] ?? null) : null;
      const b = geom.boundaries ?? [];
      const values = geom.semantics?.values as unknown[] | undefined;
      switch (geom.type) {
        case 'MultiSurface':
        case 'CompositeSurface':
          b.forEach((srf, i) => surfaces.push({ rings: srf as unknown[], semantic: semOf(values?.[i]) }));
          break;
        case 'Solid':
          (b as unknown[][]).forEach((shell, si) =>
            shell.forEach((srf, i) =>
              surfaces.push({
                rings: srf as unknown[],
                semantic: semOf((values?.[si] as unknown[] | undefined)?.[i]),
              })
            )
          );
          break;
        case 'MultiSolid':
        case 'CompositeSolid':
          (b as unknown[][][]).forEach((solid, di) =>
            solid.forEach((shell, si) =>
              shell.forEach((srf, i) =>
                surfaces.push({
                  rings: srf as unknown[],
                  semantic: semOf(
                    ((values?.[di] as unknown[] | undefined)?.[si] as unknown[] | undefined)?.[i]
                  ),
                })
              )
            )
          );
          break;
        default:
          break; // points / linestrings / templates: not drawable surfaces
      }
      for (const srf of surfaces) {
        const rings = srf.rings.map(ringPoints).filter((r) => r.length >= 3);
        if (rings.length) {
          polygons.push({ rings, semantic: srf.semantic, objectType: obj.type ?? null, objectId: root });
        }
      }
    }
  }

  return buildCityModel(polygons, convert, [...objectMeta.values()]);
}

// ── CityGML (best effort) ────────────────────────────────────────────────────

const SEMANTIC_TAGS = new Set(Object.keys(SURFACE_COLORS));
const OBJECT_TAGS = new Set(OBJECT_COLORS.map(([prefix]) => prefix));

/**
 * Parse a CityGML / GML document into a [CityModel] by collecting every
 * `gml:PosList` polygon: semantics and LoD come from ancestor element names,
 * the CRS from the first `srsName`. Covers the LoD1/LoD2 building exports that
 * are common in open city data; exotic profiles may parse partially.
 */
export function parseCityGML(xmlText: string, opts: { only?: Set<string> } = {}): CityModel {
  const doc = new DOMParser().parseFromString(xmlText, 'application/xml');
  if (doc.getElementsByTagName('parsererror').length) throw new Error('invalid XML');

  // A stable per-object id: the element's gml:id where present, else a synthetic
  // counter — so a CityGML block still supports per-object picking + isolation.
  let synthCount = 0;
  const objIds = new Map<Element, string>();
  const gmlIdOf = (el: Element): string => {
    let id = objIds.get(el);
    if (id) return id;
    for (const a of Array.from(el.attributes)) {
      if (a.localName === 'id' && a.value) {
        id = a.value;
        break;
      }
    }
    id = id || `object-${synthCount++}`;
    objIds.set(el, id);
    return id;
  };

  const srsEl = doc.querySelector('[srsName]');
  const epsg = epsgFromReference(srsEl?.getAttribute('srsName'));
  let convert = epsg != null ? toLonLat(epsg) : null;
  const geographic = epsg === 4326 || epsg === 4979;

  const polys = Array.from(doc.getElementsByTagNameNS('*', 'Polygon'));
  if (!polys.length) throw new Error('no gml:Polygon elements found');

  /** "x y z x y z …" → V3[], honouring srsDimension (default 3). */
  const parseRing = (ringHolder: Element | null): V3[] => {
    if (!ringHolder) return [];
    const posList = ringHolder.getElementsByTagNameNS('*', 'posList')[0];
    let nums: number[];
    let dim = 3;
    if (posList) {
      dim = Number(posList.getAttribute('srsDimension') || srsEl?.getAttribute('srsDimension')) || 3;
      nums = (posList.textContent ?? '').trim().split(/\s+/).map(Number);
    } else {
      const pos = Array.from(ringHolder.getElementsByTagNameNS('*', 'pos'));
      nums = pos.flatMap((p) => (p.textContent ?? '').trim().split(/\s+/).map(Number));
      dim = pos.length ? Math.round(nums.length / pos.length) : 3;
    }
    const pts: V3[] = [];
    for (let i = 0; i + dim - 1 < nums.length; i += dim) {
      // Geographic CRS lists (lat, lon); projected CRS list (E, N, H).
      const [a, b] = [nums[i], nums[i + 1]];
      const h = dim >= 3 ? nums[i + 2] : 0;
      pts.push(geographic ? [b, a, h] : [a, b, h]);
    }
    // Drop the closing repeat of the first point (triangulateShape dislikes it).
    if (pts.length > 3) {
      const [f, l] = [pts[0], pts[pts.length - 1]];
      if (f[0] === l[0] && f[1] === l[1] && f[2] === l[2]) pts.pop();
    }
    return pts;
  };

  /** Semantics / object type / LoD from the polygon's ancestor chain. */
  const classify = (el: Element) => {
    let semantic: string | null = null;
    let objectType: string | null = null;
    let lod = -1;
    for (let n: Element | null = el; n; n = n.parentElement) {
      const name = n.localName;
      if (!semantic && SEMANTIC_TAGS.has(name)) semantic = name;
      if (OBJECT_TAGS.has(name)) objectType = name;
      const m = /^lod(\d)/i.exec(name);
      if (m) lod = Math.max(lod, Number(m[1]));
    }
    return { semantic, objectType, lod };
  };

  const collected: (SemPolygon & { lod: number })[] = [];
  const objectMeta = new Map<string, CityObject>();
  const only = opts.only;
  for (const poly of polys) {
    const exterior = poly.getElementsByTagNameNS('*', 'exterior')[0] ?? poly;
    const outer = parseRing(exterior);
    if (outer.length < 3) continue;
    const holes = Array.from(poly.getElementsByTagNameNS('*', 'interior'))
      .map(parseRing)
      .filter((r) => r.length >= 3);
    const { semantic, objectType, lod } = classify(poly);
    let n: Element | null = poly;
    while (n && !OBJECT_TAGS.has(n.localName)) n = n.parentElement;
    const objectId = n ? gmlIdOf(n) : null;
    if (objectId) {
      if (only && !only.has(objectId)) continue; // isolating a subset
      if (!objectMeta.has(objectId)) {
        objectMeta.set(objectId, { id: objectId, label: objectId, type: objectType, attributes: {} });
      }
    }
    collected.push({ rings: [outer, ...holes], semantic, objectType, objectId, lod });
  }
  if (!collected.length) throw new Error('no usable polygons');

  // When multiple LoDs are present keep only the highest (avoid z-fighting).
  const maxLod = Math.max(...collected.map((p) => p.lod));
  const filtered = maxLod >= 0 ? collected.filter((p) => p.lod === maxLod || p.lod < 0) : collected;

  // Some producers emit (N, E) despite the EPSG axis convention; detect UTM-ish
  // magnitudes in the first coordinate and swap.
  if (convert && !geographic) {
    const probe = filtered[0].rings[0][0];
    if (probe && probe[0] > 1_500_000 && probe[1] < 1_500_000) {
      for (const poly of filtered) {
        for (const ring of poly.rings) {
          for (const p of ring) {
            const t = p[0];
            p[0] = p[1];
            p[1] = t;
          }
        }
      }
    }
  }
  if (epsg != null && !convert) convert = null; // unsupported EPSG → local metres

  return buildCityModel(filtered, convert, [...objectMeta.values()]);
}

// ── Per-object picking + isolation (the CityJSON counterpart of ifc.ts) ───────

/** Resolve the (root) CityObject id at a raycast hit: the merged mesh's geometry
 *  carries `objectRanges` in vertex order, so the picked triangle (`faceIndex`)
 *  maps to a building via binary search. Null when the hit owns no object range. */
export function cityObjectIdAt(mesh: unknown, faceIndex: number | null | undefined): string | null {
  const ranges: CityObjectRange[] | undefined = (mesh as { geometry?: { userData?: { objectRanges?: CityObjectRange[] } } })
    ?.geometry?.userData?.objectRanges;
  if (!ranges || faceIndex == null) return null;
  const off = faceIndex * 3; // first vertex index of the picked (non-indexed) triangle
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
  return off < r.start + r.count ? r.objectId : null;
}

/** Does a group of (possibly merged) CityJSON meshes own any triangle of `wanted`? */
export function groupHasCityObject(group: THREE.Object3D, wanted: Set<string>): boolean {
  let found = false;
  group.traverse((n) => {
    if (found || !(n as THREE.Mesh).isMesh) return;
    const ranges: CityObjectRange[] | undefined = (n as THREE.Mesh).geometry?.userData?.objectRanges;
    if (ranges) {
      for (const r of ranges) {
        if (wanted.has(r.objectId)) {
          found = true;
          return;
        }
      }
    }
  });
  return found;
}

/**
 * Build a group containing ONLY the triangles of `wanted` CityObjects, extracted
 * from a group of merged (non-indexed) CityJSON meshes — the map's x-ray
 * "selected building" overlay. Positions/normals are copied per matching vertex
 * run; the source materials are reused (the caller re-skins the clone).
 */
export function subCityGeometryForObjects(group: THREE.Object3D, wanted: Set<string>): THREE.Group {
  const out = new THREE.Group();
  group.traverse((node) => {
    const mesh = node as THREE.Mesh;
    if (!mesh.isMesh) return;
    const geom = mesh.geometry as THREE.BufferGeometry;
    const ranges: CityObjectRange[] | undefined = geom?.userData?.objectRanges;
    if (!ranges) return;
    const keep = ranges.filter((r) => wanted.has(r.objectId));
    if (!keep.length) return;
    const srcPos = geom.attributes.position.array as ArrayLike<number>;
    const srcNrm = (geom.attributes.normal?.array as ArrayLike<number>) || null;
    const pos: number[] = [];
    const nrm: number[] = [];
    for (const r of keep) {
      for (let v = r.start; v < r.start + r.count; v++) {
        pos.push(srcPos[v * 3], srcPos[v * 3 + 1], srcPos[v * 3 + 2]);
        if (srcNrm) nrm.push(srcNrm[v * 3], srcNrm[v * 3 + 1], srcNrm[v * 3 + 2]);
      }
    }
    const sg = new THREE.BufferGeometry();
    sg.setAttribute('position', new THREE.Float32BufferAttribute(pos, 3));
    if (nrm.length) sg.setAttribute('normal', new THREE.Float32BufferAttribute(nrm, 3));
    else sg.computeVertexNormals();
    out.add(new THREE.Mesh(sg, mesh.material));
  });
  return out;
}
