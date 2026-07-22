// Ground footprint of a georeferenced 3D model, in WGS84, plus the MapLibre
// filter expressions that hide the basemap buildings the model actually covers.
//
// The map layer places a model with ViewerMap's `mercMatrixFor()`, which is
// `T(merc) · S(s, -s, s) · Rx(π/2)` with `s = meterInMercatorUnits(lat) ·
// meters / NORMALISED_DIM`. Expanding that product: local +x → mercator +x
// (east), local +z → mercator +y (which grows SOUTHWARD) and local +y is
// altitude. Everything below is the exact inverse of that placement, so a
// footprint computed from model-local vertices lands precisely where the model
// renders — the old suppression instead guessed a screen-space square around
// the anchor point and missed most of the blocks a model overlaps.
//
// Deliberately free of a `three` import: it keeps the module unit-testable
// without a WebGL context and keeps it out of the lazily-loaded three chunk.
// Boxes are taken structurally (`{min,max}`), so a THREE.Box3 fits as-is, and
// `normalisedDim` is a parameter rather than an import from models.ts (which
// pulls in three at module scope).

/** `[lon, lat]`, WGS84 — the order GeoJSON and MapLibre use. */
export type LonLat = [number, number];
/** A polygon ring in WGS84; closed rings repeat the first point at the end. */
export type Ring = LonLat[];
/** `[x, z]` in model-local (normalised, y-up) units. */
export type LocalXZ = [number, number];

export interface PolygonGeometry {
  type: 'Polygon';
  coordinates: Ring[];
}
export interface MultiPolygonGeometry {
  type: 'MultiPolygon';
  coordinates: Ring[][];
}
export type FootprintGeometry = PolygonGeometry | MultiPolygonGeometry;

/** Structural stand-in for THREE.Box3 (so this module needs no three import). */
export interface BoxLike {
  min: { x: number; y: number; z: number };
  max: { x: number; y: number; z: number };
}

export interface FootprintOptions {
  /** Outward pad in metres — absorbs the mismatch between a generalised OSM
   *  footprint and a survey-accurate IFC/CityJSON one. */
  padMeters?: number;
  /** The box size models.ts#normalise() scales a model's largest dimension to. */
  normalisedDim?: number;
}

// MapLibre's own mercator constants (src/geo/mercator_coordinate.ts). Copied
// verbatim rather than derived, so the footprint cannot drift from the matrix
// the custom layer renders with.
const EARTH_RADIUS = 6371008.8;
const EARTH_CIRCUMFERENCE = 2 * Math.PI * EARTH_RADIUS;

/** The box size models.ts#normalise() scales a model's largest dimension to. */
export const DEFAULT_NORMALISED_DIM = 1.6;

export const mercatorXfromLng = (lng: number): number => (180 + lng) / 360;

export const mercatorYfromLat = (lat: number): number =>
  (180 - (180 / Math.PI) * Math.log(Math.tan(Math.PI / 4 + (lat * Math.PI) / 360))) / 360;

export const lngFromMercatorX = (x: number): number => x * 360 - 180;

export const latFromMercatorY = (y: number): number =>
  (360 / Math.PI) * Math.atan(Math.exp(((180 - y * 360) * Math.PI) / 180)) - 90;

/** Size of one metre in mercator units at `lat` (MapLibre's
 *  `MercatorCoordinate.meterInMercatorCoordinateUnits`). */
export const meterInMercatorUnits = (lat: number): number =>
  (1 / EARTH_CIRCUMFERENCE) * (1 / Math.cos((lat * Math.PI) / 180));

/** Mercator unit scale for one model-local unit of a model rendered `meters`
 *  wide at `anchor` — the `s` in `mercMatrixFor()`. */
function localScale(meters: number, anchor: LonLat, normalisedDim: number): number {
  return meterInMercatorUnits(anchor[1]) * (meters / (normalisedDim || DEFAULT_NORMALISED_DIM));
}

/**
 * Model-local `[x, z]` → WGS84 `[lon, lat]`, inverting `mercMatrixFor()`.
 * Note the sign of z: mercator y grows southward, so a vertex at +z sits SOUTH
 * of the anchor — the single easiest thing to get backwards here.
 */
export function localToLngLat(
  x: number,
  z: number,
  meters: number,
  anchor: LonLat,
  normalisedDim: number = DEFAULT_NORMALISED_DIM,
): LonLat {
  const s = localScale(meters, anchor, normalisedDim);
  return [
    lngFromMercatorX(mercatorXfromLng(anchor[0]) + s * x),
    latFromMercatorY(mercatorYfromLat(anchor[1]) + s * z),
  ];
}

/** Signed area of a ring in lon/lat space; positive = counter-clockwise. */
function signedArea(ring: Ring): number {
  let a = 0;
  for (let i = 0, j = ring.length - 1; i < ring.length; j = i++) {
    a += (ring[j][0] - ring[i][0]) * (ring[j][1] + ring[i][1]);
  }
  return a / 2;
}

/** Close a ring (repeat the first point) and orient it counter-clockwise —
 *  the winding GeoJSON prescribes for exterior rings. */
function finishRing(ring: Ring): Ring {
  const out = signedArea(ring) < 0 ? [...ring].reverse() : [...ring];
  const first = out[0];
  const last = out[out.length - 1];
  if (!last || first[0] !== last[0] || first[1] !== last[1]) out.push([first[0], first[1]]);
  return out;
}

/**
 * Axis-aligned ground footprint of a model's bounding box. This is the coarse
 * fallback used when a model's vertices can't be sampled; prefer
 * [footprintsFromLocalPoints] which follows the model's real outline.
 */
export function footprintPolygon(
  box: BoxLike,
  meters: number,
  anchor: LonLat,
  opts: FootprintOptions = {},
): PolygonGeometry {
  const nd = opts.normalisedDim ?? DEFAULT_NORMALISED_DIM;
  const s = localScale(meters, anchor, nd);
  const pad = (opts.padMeters ?? 0) * meterInMercatorUnits(anchor[1]);
  const cx = mercatorXfromLng(anchor[0]);
  const cy = mercatorYfromLat(anchor[1]);
  const x0 = cx + s * box.min.x - pad;
  const x1 = cx + s * box.max.x + pad;
  // Mercator y grows southward, so max.z is the SOUTH edge and min.z the NORTH.
  const yS = cy + s * box.max.z + pad;
  const yN = cy + s * box.min.z - pad;
  const corner = (mx: number, my: number): LonLat => [lngFromMercatorX(mx), latFromMercatorY(my)];
  return {
    type: 'Polygon',
    coordinates: [finishRing([corner(x0, yS), corner(x1, yS), corner(x1, yN), corner(x0, yN)])],
  };
}

/**
 * Convex hull of model-local `[x, z]` points (Andrew's monotone chain), returned
 * counter-clockwise in x/z space with duplicate and collinear points dropped.
 * Fewer than three distinct points yields the input unchanged — callers treat
 * that as degenerate and fall back to a box.
 */
export function convexHullXZ(points: LocalXZ[]): LocalXZ[] {
  const seen = new Set<string>();
  const pts: LocalXZ[] = [];
  for (const p of points) {
    if (!Number.isFinite(p[0]) || !Number.isFinite(p[1])) continue;
    const k = `${p[0]},${p[1]}`;
    if (seen.has(k)) continue;
    seen.add(k);
    pts.push([p[0], p[1]]);
  }
  if (pts.length < 3) return pts;
  pts.sort((a, b) => a[0] - b[0] || a[1] - b[1]);
  const cross = (o: LocalXZ, a: LocalXZ, b: LocalXZ) =>
    (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0]);
  const half = (src: LocalXZ[]): LocalXZ[] => {
    const out: LocalXZ[] = [];
    for (const p of src) {
      while (out.length >= 2 && cross(out[out.length - 2], out[out.length - 1], p) <= 0) out.pop();
      out.push(p);
    }
    out.pop(); // the shared endpoint belongs to the other half
    return out;
  };
  const hull = [...half(pts), ...half([...pts].reverse())];
  return hull.length >= 3 ? hull : pts;
}

/**
 * Drop the least significant vertices of a ring until it has at most
 * `maxPoints`, choosing each time the vertex whose removal changes the area
 * least (the triangle it forms with its neighbours). Ring size is the dominant
 * cost of the `distance` filter — every basemap building feature is tested
 * against every footprint vertex — so this cap is a hard performance budget,
 * not cosmetic. A closed ring stays closed; the result never drops below a
 * triangle.
 */
export function decimateRing<T extends [number, number]>(ring: T[], maxPoints: number): T[] {
  if (ring.length < 2) return [...ring];
  const closed =
    ring[0][0] === ring[ring.length - 1][0] && ring[0][1] === ring[ring.length - 1][1];
  const pts = closed ? ring.slice(0, -1) : [...ring];
  const limit = Math.max(3, Math.floor(maxPoints));
  while (pts.length > limit) {
    let worst = -1;
    let worstArea = Infinity;
    for (let i = 0; i < pts.length; i++) {
      const a = pts[(i - 1 + pts.length) % pts.length];
      const b = pts[i];
      const c = pts[(i + 1) % pts.length];
      const area = Math.abs((b[0] - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (b[1] - a[1])) / 2;
      if (area < worstArea) {
        worstArea = area;
        worst = i;
      }
    }
    if (worst < 0) break;
    pts.splice(worst, 1);
  }
  if (closed) pts.push(pts[0]);
  return pts;
}

/**
 * Split model-local `[x, z]` points into connected components on a coarse grid
 * (8-neighbour). A self-georeferenced CityJSON excerpt is one model but many
 * buildings with streets in between; one convex hull over all of them would
 * blank the OSM blocks in those gaps, so each cluster gets its own footprint.
 */
export function clusterXZ(points: LocalXZ[], cell: number): LocalXZ[][] {
  const size = cell > 0 ? cell : 1;
  const buckets = new Map<string, LocalXZ[]>();
  for (const p of points) {
    if (!Number.isFinite(p[0]) || !Number.isFinite(p[1])) continue;
    const key = `${Math.floor(p[0] / size)},${Math.floor(p[1] / size)}`;
    const b = buckets.get(key);
    if (b) b.push(p);
    else buckets.set(key, [p]);
  }
  const out: LocalXZ[][] = [];
  const seen = new Set<string>();
  for (const key of buckets.keys()) {
    if (seen.has(key)) continue;
    seen.add(key);
    const group: LocalXZ[] = [];
    const stack = [key];
    while (stack.length) {
      const k = stack.pop() as string;
      const pts = buckets.get(k);
      if (pts) group.push(...pts);
      const [gx, gy] = k.split(',').map(Number);
      for (let dx = -1; dx <= 1; dx++) {
        for (let dy = -1; dy <= 1; dy++) {
          if (!dx && !dy) continue;
          const nk = `${gx + dx},${gy + dy}`;
          if (buckets.has(nk) && !seen.has(nk)) {
            seen.add(nk);
            stack.push(nk);
          }
        }
      }
    }
    out.push(group);
  }
  return out;
}

/** Twice the signed area of a local x/z ring — used only to spot degenerate
 *  (collinear) hulls, so the sign is irrelevant here. */
function ringArea2(ring: LocalXZ[]): number {
  let a = 0;
  for (let i = 0, j = ring.length - 1; i < ring.length; j = i++) {
    a += ring[j][0] * ring[i][1] - ring[i][0] * ring[j][1];
  }
  return a;
}

/** Axis-aligned local ring around `pts`, at least `minSize` across — the
 *  stand-in for a cluster too small or too collinear to hull. */
function boxRing(pts: LocalXZ[], minSize: number): LocalXZ[] {
  let x0 = Infinity;
  let x1 = -Infinity;
  let z0 = Infinity;
  let z1 = -Infinity;
  for (const p of pts) {
    if (p[0] < x0) x0 = p[0];
    if (p[0] > x1) x1 = p[0];
    if (p[1] < z0) z0 = p[1];
    if (p[1] > z1) z1 = p[1];
  }
  if (!Number.isFinite(x0)) return [];
  const gx = Math.max(0, minSize - (x1 - x0)) / 2;
  const gz = Math.max(0, minSize - (z1 - z0)) / 2;
  return [
    [x0 - gx, z0 - gz],
    [x1 + gx, z0 - gz],
    [x1 + gx, z1 + gz],
    [x0 - gx, z1 + gz],
  ];
}

/** Map a local x/z ring to a closed, CCW WGS84 polygon. */
function ringToPolygon(
  ring: LocalXZ[],
  meters: number,
  anchor: LonLat,
  normalisedDim: number,
): PolygonGeometry | null {
  if (ring.length < 3) return null;
  const lonlat = ring.map(([x, z]) => localToLngLat(x, z, meters, anchor, normalisedDim));
  return { type: 'Polygon', coordinates: [finishRing(lonlat)] };
}

export interface LocalFootprintOptions extends FootprintOptions {
  /** Maximum vertices per emitted ring (the `distance` cost budget). */
  maxPoints?: number;
  /** Maximum separate footprints; the smallest clusters beyond it are dropped. */
  maxRings?: number;
  /** Grid cell for the clustering pass, in metres. */
  cellMeters?: number;
}

/**
 * Real ground footprint(s) of a model from a sample of its vertices: cluster →
 * convex hull → decimate → project. This is what turns a diagonal bridge, an
 * L-shaped IFC building or a multi-building 3DBAG excerpt from an over-covering
 * bounding box into the shape the model actually occupies.
 *
 * Vertices are used at every height, not just near the ground: a cantilevered
 * roof or a bridge deck visually covers the block underneath it, and that block
 * is exactly what the user wants gone.
 */
export function footprintsFromLocalPoints(
  points: LocalXZ[],
  meters: number,
  anchor: LonLat,
  opts: LocalFootprintOptions = {},
): PolygonGeometry[] {
  if (!points.length) return [];
  const nd = opts.normalisedDim ?? DEFAULT_NORMALISED_DIM;
  // Ring count × ring size is the `distance` filter's cost budget: every basemap
  // building feature in every tile is tested against every footprint vertex.
  const maxPoints = opts.maxPoints ?? 8;
  const maxRings = opts.maxRings ?? 4;
  // Local units → metres: a model rendered `meters` across spans `normalisedDim`
  // local units, so one local unit is meters/normalisedDim metres.
  const metresPerUnit = (meters || 1) / (nd || DEFAULT_NORMALISED_DIM);
  const cellLocal = Math.max(0.001, (opts.cellMeters ?? 8) / Math.max(metresPerUnit, 1e-9));
  // A hull that came out degenerate (a single storey column of vertices, a
  // perfectly flat wall) still has to cover something, so it falls back to its
  // own box at ~1 m minimum rather than emitting a zero-area ring — `distance`
  // returns NaN for those, and our fail-open filter would then hide nothing.
  const minLocal = 1 / Math.max(metresPerUnit, 1e-9);
  const clusters = clusterXZ(points, cellLocal);
  // Over the ring budget, keep the BIGGEST clusters and drop the rest. Hulling
  // every point instead (what this used to do) is the worst possible answer:
  // one polygon over the model's whole extent covers strictly more than the
  // clusters' union, so the basemap blocks in the gaps — the very thing
  // clusterXZ exists to preserve — would go with it. Dropping small clusters
  // fails the safe way round: a little of the model stays un-suppressed.
  const chosen =
    clusters.length === 0
      ? [points]
      : clusters.length > maxRings
        ? [...clusters].sort((a, b) => b.length - a.length).slice(0, maxRings)
        : clusters;
  const out: PolygonGeometry[] = [];
  for (const cluster of chosen) {
    const hull = convexHullXZ(cluster);
    // Anything thinner than ~0.01 m² (a single wall's vertices, one column of a
    // point cloud) is treated as degenerate and covered by its box instead.
    const solid = hull.length >= 3 && Math.abs(ringArea2(hull)) / 2 > minLocal * minLocal * 0.01;
    const ring = solid ? decimateRing(hull, maxPoints) : boxRing(cluster, minLocal);
    const poly = ringToPolygon(ring, meters, anchor, nd);
    if (poly) out.push(poly);
  }
  return out;
}

/** Merge single-ring polygons into one MultiPolygon (null when there are none). */
export function footprintsToMultiPolygon(polys: PolygonGeometry[]): MultiPolygonGeometry | null {
  const coordinates = polys.filter(Boolean).map((p) => p.coordinates);
  return coordinates.length ? { type: 'MultiPolygon', coordinates } : null;
}

/**
 * MapLibre filter keeping only the basemap buildings that are further than
 * `bufferMeters` from the model footprint.
 *
 * Written as `!(distance <= b)` and NOT as `distance > b` on purpose: MapLibre's
 * `distance` expression returns NaN for degenerate geometry, and `NaN > b` is
 * false — which would HIDE the feature. One bad footprint would then blank the
 * entire basemap building layer. Negating `<=` fails open instead: `NaN <= b` is
 * false, so `!` keeps the building visible.
 */
export function buildingSuppressionFilter(
  geo: FootprintGeometry | null,
  bufferMeters = 0,
): unknown[] | null {
  if (!geo || !geo.coordinates.length) return null;
  return ['!', ['<=', ['distance', geo], bufferMeters]];
}

/** Fallback filter for engines/styles where `distance` is unavailable: hide the
 *  listed feature ids. Features that carry no id cannot be addressed this way
 *  and therefore stay visible — see `polygonsIntersect` and its caller. */
export function buildingIdFilter(ids: Array<string | number>): unknown[] | null {
  return ids.length ? ['!', ['in', ['id'], ['literal', [...ids]]]] : null;
}

/** Ray-casting point-in-polygon; the ring may be open or closed. */
export function pointInPolygon(pt: [number, number], ring: Array<[number, number]>): boolean {
  const n = ring.length;
  if (n < 3) return false;
  let inside = false;
  for (let i = 0, j = n - 1; i < n; j = i++) {
    const [xi, yi] = ring[i];
    const [xj, yj] = ring[j];
    if (yi > pt[1] !== yj > pt[1] && pt[0] < ((xj - xi) * (pt[1] - yi)) / (yj - yi || 1e-15) + xi) {
      inside = !inside;
    }
  }
  return inside;
}

const orient = (a: [number, number], b: [number, number], c: [number, number]) =>
  (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);

const onSegment = (a: [number, number], b: [number, number], p: [number, number]) =>
  Math.min(a[0], b[0]) <= p[0] &&
  p[0] <= Math.max(a[0], b[0]) &&
  Math.min(a[1], b[1]) <= p[1] &&
  p[1] <= Math.max(a[1], b[1]);

/** Do segments p1→p2 and q1→q2 touch or cross? (Collinear overlap included.) */
export function segmentsIntersect(
  p1: [number, number],
  p2: [number, number],
  q1: [number, number],
  q2: [number, number],
): boolean {
  const d1 = orient(p1, p2, q1);
  const d2 = orient(p1, p2, q2);
  const d3 = orient(q1, q2, p1);
  const d4 = orient(q1, q2, p2);
  if (d1 * d2 < 0 && d3 * d4 < 0) return true;
  if (d1 === 0 && onSegment(p1, p2, q1)) return true;
  if (d2 === 0 && onSegment(p1, p2, q2)) return true;
  if (d3 === 0 && onSegment(q1, q2, p1)) return true;
  if (d4 === 0 && onSegment(q1, q2, p2)) return true;
  return false;
}

/** Do two simple rings overlap (share area) or touch? Used by the id fallback,
 *  where the intersection test has to run in JS instead of in the tile worker. */
export function polygonsIntersect(
  a: Array<[number, number]>,
  b: Array<[number, number]>,
): boolean {
  if (a.length < 3 || b.length < 3) return false;
  for (const p of a) if (pointInPolygon(p, b)) return true;
  for (const p of b) if (pointInPolygon(p, a)) return true;
  for (let i = 0, j = a.length - 1; i < a.length; j = i++) {
    for (let k = 0, l = b.length - 1; k < b.length; l = k++) {
      if (segmentsIntersect(a[j], a[i], b[l], b[k])) return true;
    }
  }
  return false;
}

/**
 * Is `filter` an *expression* filter (as opposed to the legacy array form)?
 * Mirrors @maplibre/maplibre-gl-style-spec's `isExpressionFilter`, because
 * wrapping a legacy filter in `['all', legacy, expr]` makes the whole style
 * fail validation — every child of `all` must itself be an expression.
 */
export function isExpressionFilter(filter: unknown): boolean {
  if (filter === true || filter === false) return true;
  if (!Array.isArray(filter) || filter.length === 0) return false;
  switch (filter[0]) {
    case 'has':
      return filter.length >= 2 && filter[1] !== '$id' && filter[1] !== '$type';
    case 'in':
      return filter.length >= 3 && (typeof filter[1] !== 'string' || Array.isArray(filter[2]));
    case '!in':
    case '!has':
    case 'none':
      return false;
    case '==':
    case '!=':
    case '>':
    case '>=':
    case '<':
    case '<=':
      return filter.length !== 3 || Array.isArray(filter[1]) || Array.isArray(filter[2]);
    case 'any':
    case 'all':
      for (const f of filter.slice(1)) {
        if (!isExpressionFilter(f) && typeof f !== 'boolean') return false;
      }
      return true;
    default:
      return true;
  }
}

/**
 * Combine a layer's own filter with ours. Returns `undefined` when the layer's
 * filter is a legacy one that cannot be safely wrapped — the caller must then
 * leave that layer alone rather than break the style.
 */
export function combineFilters(orig: unknown, ours: unknown[] | null): unknown | undefined {
  if (!ours) return orig ?? null;
  if (orig === null || orig === undefined) return ours;
  if (!isExpressionFilter(orig)) return undefined;
  return ['all', orig, ours];
}
