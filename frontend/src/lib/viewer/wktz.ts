// Volumetric WKT (POLYHEDRALSURFACE Z / TIN Z) → renderable faces.
//
// The viewer feed delivers `wkt3d`: a Z-carrying WKT body in EPSG:4326 with
// heights in metres (src/geo/viewer_feed.rs reprojects it text-wise, since the
// 2-D geometry pipeline cannot represent polyhedral types at all). This module
// parses it into per-face vertex rings for the map's 3D layer — deliberately
// free of a `three` import so it stays unit-testable and out of the WebGL
// chunk until a volumetric element actually renders.

/** One vertex: [lon, lat, heightMeters]. */
export type LonLatZ = [number, number, number];

/** Parsed volumetric geometry: one OUTER ring per face (holes are rare in
 *  building solids and are dropped rather than mis-triangulated). */
export interface WktZGeometry {
  faces: LonLatZ[][];
}

/**
 * Parse a `POLYHEDRALSURFACE Z` / `TIN Z` body. Returns null when the text is
 * not volumetric or yields no usable face. Tolerates the optional `Z` token
 * and arbitrary whitespace; a face's ring is closed in WKT (first point
 * repeated) — the repeat is dropped here so consumers triangulate cleanly.
 */
export function parseWktZ(wkt: string | null | undefined): WktZGeometry | null {
  const t = String(wkt ?? '').trim();
  if (!/^(POLYHEDRALSURFACE|TIN)\b/i.test(t)) return null;
  const faces: LonLatZ[][] = [];
  // Faces are `((ring[, hole…]))` groups; capture each group's FIRST ring.
  const faceRe = /\(\(([^()]*)\)/g;
  let m: RegExpExecArray | null;
  while ((m = faceRe.exec(t))) {
    const ring: LonLatZ[] = [];
    for (const coord of m[1].split(',')) {
      const parts = coord.trim().split(/\s+/).map(Number);
      if (parts.length < 3 || parts.some((v) => !Number.isFinite(v))) {
        ring.length = 0;
        break;
      }
      ring.push([parts[0], parts[1], parts[2]]);
    }
    if (ring.length >= 4) {
      const first = ring[0];
      const last = ring[ring.length - 1];
      if (first[0] === last[0] && first[1] === last[1] && first[2] === last[2]) ring.pop();
    }
    if (ring.length >= 3) faces.push(ring);
  }
  return faces.length ? { faces } : null;
}

/** Centroid of all face vertices — the anchor the map places the mesh at. */
export function wktZCentroid(geom: WktZGeometry): [number, number] {
  let lon = 0;
  let lat = 0;
  let n = 0;
  for (const face of geom.faces) {
    for (const [x, y] of face) {
      lon += x;
      lat += y;
      n += 1;
    }
  }
  return n ? [lon / n, lat / n] : [0, 0];
}

/**
 * Fan-triangulate every face into a flat position array of LOCAL metres about
 * `anchor`: x = east, y = height, z = south (the scene convention models use).
 * Faces of building solids are convex quads/rings, for which a fan is exact.
 */
export function wktZToLocalTriangles(
  geom: WktZGeometry,
  anchor: [number, number],
  lonLatToLocalMeters: (anchor: [number, number], p: [number, number]) => [number, number],
): Float32Array {
  const out: number[] = [];
  for (const face of geom.faces) {
    const local = face.map(([lon, lat, z]) => {
      const [east, north] = lonLatToLocalMeters(anchor, [lon, lat]);
      return [east, z, -north] as const;
    });
    for (let i = 1; i + 1 < local.length; i++) {
      out.push(...local[0], ...local[i], ...local[i + 1]);
    }
  }
  return new Float32Array(out);
}
