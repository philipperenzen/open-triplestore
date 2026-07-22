import { describe, it, expect } from 'vitest';
import {
  buildingIdFilter,
  buildingSuppressionFilter,
  clusterXZ,
  combineFilters,
  convexHullXZ,
  decimateRing,
  footprintPolygon,
  footprintsFromLocalPoints,
  footprintsToMultiPolygon,
  isExpressionFilter,
  latFromMercatorY,
  localToLngLat,
  lngFromMercatorX,
  mercatorXfromLng,
  mercatorYfromLat,
  meterInMercatorUnits,
  pointInPolygon,
  polygonsIntersect,
  segmentsIntersect,
} from '../viewer/footprint';
import type { LocalXZ, LonLat, Ring } from '../viewer/footprint';

/** Big Ben: a ~20 × 20 m tower, 96 m tall, normalised into a 1.6-unit box. */
const BIG_BEN: LonLat = [-0.1246, 51.5007];
const TOWER_BOX = {
  min: { x: -(1.6 * 20) / 96 / 2, y: 0, z: -(1.6 * 20) / 96 / 2 },
  max: { x: (1.6 * 20) / 96 / 2, y: 1.6, z: (1.6 * 20) / 96 / 2 },
};

/** Ground distance in metres between two lon/lat points (cheap-ruler style). */
function metres(a: LonLat, b: LonLat): number {
  const lat = ((a[1] + b[1]) / 2) * (Math.PI / 180);
  const kx = 111319.49 * Math.cos(lat);
  const ky = 111319.49;
  const dx = (a[0] - b[0]) * kx;
  const dy = (a[1] - b[1]) * ky;
  return Math.hypot(dx, dy);
}

describe('mercator helpers', () => {
  it('round-trips lon/lat through mercator', () => {
    expect(lngFromMercatorX(mercatorXfromLng(5.86))).toBeCloseTo(5.86, 9);
    expect(latFromMercatorY(mercatorYfromLat(51.851))).toBeCloseTo(51.851, 9);
  });

  it('scales a metre by the mercator factor at the given latitude', () => {
    // 1 / (2π · 6371008.8) at the equator, secant-scaled towards the poles.
    expect(meterInMercatorUnits(0)).toBeCloseTo(1 / (2 * Math.PI * 6371008.8), 15);
    expect(meterInMercatorUnits(60)).toBeCloseTo(meterInMercatorUnits(0) * 2, 12);
  });
});

describe('footprintPolygon', () => {
  it('spans the model’s real footprint in both ground axes', () => {
    const ring = footprintPolygon(TOWER_BOX, 96, BIG_BEN).coordinates[0];
    // Closed ring: 4 corners + the repeated first point.
    expect(ring).toHaveLength(5);
    const lons = ring.map((p) => p[0]);
    const lats = ring.map((p) => p[1]);
    const west = Math.min(...lons);
    const east = Math.max(...lons);
    const south = Math.min(...lats);
    const north = Math.max(...lats);
    expect(metres([west, south], [east, south])).toBeCloseTo(20, 1);
    expect(metres([west, south], [west, north])).toBeCloseTo(20, 1);
  });

  it('places local +z SOUTH of the anchor (mercator y grows downward)', () => {
    const north = localToLngLat(0, -0.5, 96, BIG_BEN);
    const south = localToLngLat(0, 0.5, 96, BIG_BEN);
    expect(north[1]).toBeGreaterThan(south[1]);
    // ... and local +x east.
    expect(localToLngLat(0.5, 0, 96, BIG_BEN)[0]).toBeGreaterThan(BIG_BEN[0]);
  });

  it('is symmetric about the anchor for a normalised box', () => {
    const ring = footprintPolygon(TOWER_BOX, 96, BIG_BEN).coordinates[0];
    const lons = ring.map((p) => p[0]);
    const lats = ring.map((p) => p[1]);
    expect((Math.min(...lons) + Math.max(...lons)) / 2).toBeCloseTo(BIG_BEN[0], 9);
    expect((Math.min(...lats) + Math.max(...lats)) / 2).toBeCloseTo(BIG_BEN[1], 6);
  });

  it('widens by exactly twice the pad', () => {
    const plain = footprintPolygon(TOWER_BOX, 96, BIG_BEN).coordinates[0];
    const padded = footprintPolygon(TOWER_BOX, 96, BIG_BEN, { padMeters: 3 }).coordinates[0];
    const span = (r: Ring) => Math.max(...r.map((p) => p[0])) - Math.min(...r.map((p) => p[0]));
    const grew = (span(padded) - span(plain)) * 111319.49 * Math.cos((BIG_BEN[1] * Math.PI) / 180);
    expect(grew).toBeCloseTo(6, 1);
  });

  it('winds the ring counter-clockwise', () => {
    const ring = footprintPolygon(TOWER_BOX, 96, BIG_BEN).coordinates[0];
    let a = 0;
    for (let i = 0, j = ring.length - 1; i < ring.length; j = i++) {
      a += (ring[j][0] - ring[i][0]) * (ring[j][1] + ring[i][1]);
    }
    expect(a).toBeGreaterThan(0);
  });
});

describe('convexHullXZ / decimateRing', () => {
  it('keeps only the corners of a square with interior points', () => {
    const pts: LocalXZ[] = [
      [0, 0],
      [1, 0],
      [1, 1],
      [0, 1],
      [0.5, 0.5],
      [0.2, 0.7],
      [0.5, 0],
    ];
    const hull = convexHullXZ(pts);
    expect(hull).toHaveLength(4);
    expect(new Set(hull.map((p) => p.join(',')))).toEqual(
      new Set(['0,0', '1,0', '1,1', '0,1']),
    );
  });

  it('returns the input unchanged when there are fewer than three points', () => {
    expect(convexHullXZ([[0, 0], [1, 1]])).toEqual([[0, 0], [1, 1]]);
  });

  it('decimates to the cap while preserving closure and a minimum triangle', () => {
    const circle: LocalXZ[] = [];
    for (let i = 0; i < 32; i++) {
      circle.push([Math.cos((i / 32) * 2 * Math.PI), Math.sin((i / 32) * 2 * Math.PI)]);
    }
    const closed = [...circle, circle[0]];
    const cut = decimateRing(closed, 8);
    expect(cut).toHaveLength(9); // 8 + the repeated first point
    expect(cut[0]).toEqual(cut[cut.length - 1]);
    expect(decimateRing(closed, 1).length).toBe(4); // never below a triangle
  });
});

describe('clusterXZ / footprintsFromLocalPoints', () => {
  it('separates two point groups that are further apart than the cell', () => {
    const pts: LocalXZ[] = [
      [0, 0],
      [0.1, 0.1],
      [0, 0.1],
      [10, 10],
      [10.1, 10],
      [10, 10.1],
    ];
    expect(clusterXZ(pts, 0.5)).toHaveLength(2);
    expect(clusterXZ(pts, 100)).toHaveLength(1);
  });

  it('emits one footprint per building for a multi-building model', () => {
    // Two 8 m squares 40 m apart inside a 100 m-wide model.
    const pts: LocalXZ[] = [];
    for (const cx of [-0.32, 0.32]) {
      for (const dx of [-0.064, 0.064]) {
        for (const dz of [-0.064, 0.064]) pts.push([cx + dx, dz]);
      }
    }
    const polys = footprintsFromLocalPoints(pts, 100, BIG_BEN);
    expect(polys).toHaveLength(2);
    for (const p of polys) {
      const ring = p.coordinates[0];
      const lons = ring.map((q) => q[0]);
      const width = metres([Math.min(...lons), BIG_BEN[1]], [Math.max(...lons), BIG_BEN[1]]);
      expect(width).toBeCloseTo(8, 0);
    }
  });

  it('keeps the biggest clusters at the ring cap instead of hulling everything', () => {
    // Five separated blobs; the first is much denser than the others.
    const pts: LocalXZ[] = [];
    for (let c = 0; c < 5; c++) {
      const n = c === 0 ? 12 : 3;
      for (let i = 0; i < n; i++) pts.push([c * 0.5 + (i % 3) * 0.01, Math.floor(i / 3) * 0.01]);
    }
    const polys = footprintsFromLocalPoints(pts, 100, BIG_BEN, { maxRings: 2 });
    expect(polys).toHaveLength(2);
    // The emitted rings must stay LOCAL: one hull over all five blobs would
    // span the model's full 2-unit extent and blank the basemap in between.
    const lons = polys.flatMap((p) => p.coordinates[0].map((q) => q[0]));
    const span = metres([Math.min(...lons), BIG_BEN[1]], [Math.max(...lons), BIG_BEN[1]]);
    const full = metres(localToLngLat(0, 0, 100, BIG_BEN), localToLngLat(2, 0, 100, BIG_BEN));
    expect(span).toBeLessThan(full);
    // …and the densest cluster (at local x = 0) is one of the survivors.
    expect(Math.min(...lons)).toBeCloseTo(localToLngLat(0, 0, 100, BIG_BEN)[0], 5);
  });

  it('covers a degenerate (collinear) cluster with a box instead of a zero-area ring', () => {
    const pts: LocalXZ[] = [
      [0, 0],
      [0, 0.1],
      [0, 0.2],
    ];
    const polys = footprintsFromLocalPoints(pts, 100, BIG_BEN);
    expect(polys).toHaveLength(1);
    const ring = polys[0].coordinates[0];
    const lons = ring.map((q) => q[0]);
    expect(metres([Math.min(...lons), BIG_BEN[1]], [Math.max(...lons), BIG_BEN[1]])).toBeGreaterThan(0.5);
  });

  it('returns nothing for an empty sample', () => {
    expect(footprintsFromLocalPoints([], 100, BIG_BEN)).toEqual([]);
  });
});

describe('polygon intersection (id-fallback path)', () => {
  const unit: Array<[number, number]> = [
    [0, 0],
    [2, 0],
    [2, 2],
    [0, 2],
  ];

  it('detects overlap, containment and edge contact', () => {
    expect(polygonsIntersect(unit, [[1, 1], [3, 1], [3, 3], [1, 3]])).toBe(true); // overlap
    expect(polygonsIntersect(unit, [[0.5, 0.5], [1, 0.5], [1, 1], [0.5, 1]])).toBe(true); // inside
    expect(polygonsIntersect([[0.5, 0.5], [1, 0.5], [1, 1], [0.5, 1]], unit)).toBe(true); // contains
    expect(polygonsIntersect(unit, [[2, 0], [4, 0], [4, 2], [2, 2]])).toBe(true); // shared edge
  });

  it('rejects disjoint polygons whose bounding boxes overlap', () => {
    // An L-shaped arrangement: the bboxes overlap but the shapes do not.
    const a: Array<[number, number]> = [[0, 0], [1, 0], [1, 4], [0, 4]];
    const b: Array<[number, number]> = [[2, 0], [4, 0], [4, 1], [2, 1]];
    expect(polygonsIntersect(a, b)).toBe(false);
  });

  it('exposes the primitives it is built from', () => {
    expect(pointInPolygon([1, 1], unit)).toBe(true);
    expect(pointInPolygon([3, 1], unit)).toBe(false);
    expect(segmentsIntersect([0, 0], [2, 2], [0, 2], [2, 0])).toBe(true);
    expect(segmentsIntersect([0, 0], [1, 0], [0, 1], [1, 1])).toBe(false);
  });
});

describe('filter expressions', () => {
  it('emits a fail-open distance filter', () => {
    const geo = footprintPolygon(TOWER_BOX, 96, BIG_BEN);
    const multi = footprintsToMultiPolygon([geo])!;
    expect(buildingSuppressionFilter(multi)).toEqual([
      '!',
      ['<=', ['distance', multi], 0],
    ]);
    // `['>' , distance, b]` would hide every feature whose distance is NaN;
    // the negated `<=` keeps them, so a bad footprint can never blank the map.
    expect(buildingSuppressionFilter(multi, 0.5)![1]).toEqual(['<=', ['distance', multi], 0.5]);
  });

  it('returns null when there is nothing to suppress', () => {
    expect(buildingSuppressionFilter(null)).toBeNull();
    expect(footprintsToMultiPolygon([])).toBeNull();
    expect(buildingIdFilter([])).toBeNull();
    expect(buildingIdFilter([1, 2])).toEqual(['!', ['in', ['id'], ['literal', [1, 2]]]]);
  });

  it('recognises legacy filters so they are never wrapped in `all`', () => {
    expect(isExpressionFilter(['==', 'class', 'x'])).toBe(false);
    expect(isExpressionFilter(['!in', 'class', 'x'])).toBe(false);
    expect(isExpressionFilter(['==', ['get', 'class'], 'x'])).toBe(true);
    expect(isExpressionFilter(['all', ['==', ['get', 'a'], 1]])).toBe(true);
    expect(isExpressionFilter(['all', ['==', 'a', 1]])).toBe(false);
  });

  it('is accepted and correctly evaluated by MapLibre’s own expression engine', async () => {
    // The `distance` expression is the load-bearing part of the suppression, so
    // a shape-only assertion is not enough: a rejected filter, a wrong buffer or
    // a north/south sign error would all pass it. This runs the real filter the
    // tile worker would run. The style-spec package is maplibre-gl's own pinned
    // dependency; if it ever stops resolving the check skips rather than fails.
    let mod: Record<string, unknown>;
    try {
      mod = (await import('@maplibre/maplibre-gl-style-spec')) as Record<string, unknown>;
    } catch {
      return;
    }
    const spec = (mod.featureFilter ? mod : (mod.default as Record<string, unknown>)) ?? mod;
    const featureFilter = spec.featureFilter as (f: unknown) => {
      needGeometry: boolean;
      filter: (g: unknown, f: unknown, c: unknown) => boolean;
    };

    const geo = footprintsToMultiPolygon([footprintPolygon(TOWER_BOX, 96, BIG_BEN)])!;
    const compiled = featureFilter(buildingSuppressionFilter(geo, 0.5));
    expect(compiled.needGeometry).toBe(true); // real geometry reaches the filter

    // Feature geometry arrives in tile-local units; mirror MapLibre's own
    // conversion so the synthetic buildings land exactly where they claim to.
    const EXTENT = 8192;
    const z = 14;
    const n = 2 ** z;
    const canonical = {
      z,
      x: Math.floor(mercatorXfromLng(BIG_BEN[0]) * n),
      y: Math.floor(mercatorYfromLat(BIG_BEN[1]) * n),
    };
    const kx = 111319.49 * Math.cos((BIG_BEN[1] * Math.PI) / 180);
    const ky = 111319.49;
    const squareFeature = (offsetEastM: number, halfM: number, id: number) => {
      const dLon = halfM / kx;
      const dLat = halfM / ky;
      const oLon = offsetEastM / kx;
      const ring: LonLat[] = [
        [BIG_BEN[0] - dLon + oLon, BIG_BEN[1] - dLat],
        [BIG_BEN[0] + dLon + oLon, BIG_BEN[1] - dLat],
        [BIG_BEN[0] + dLon + oLon, BIG_BEN[1] + dLat],
        [BIG_BEN[0] - dLon + oLon, BIG_BEN[1] + dLat],
        [BIG_BEN[0] - dLon + oLon, BIG_BEN[1] - dLat],
      ];
      return {
        type: 3, // Polygon
        id,
        properties: {},
        geometry: [
          ring.map((p) => ({
            x: Math.round((mercatorXfromLng(p[0]) * n - canonical.x) * EXTENT),
            y: Math.round((mercatorYfromLat(p[1]) * n - canonical.y) * EXTENT),
          })),
        ],
      };
    };

    const globals = { zoom: z };
    // The 12 m block the 20 m tower stands on: gone.
    expect(compiled.filter(globals, squareFeature(0, 6, 1), canonical)).toBe(false);
    // Its neighbour 60 m east: untouched (the old anchor-radius pass hid this).
    expect(compiled.filter(globals, squareFeature(60, 6, 2), canonical)).toBe(true);
    // A block just clipping the footprint's east edge is gone too.
    expect(compiled.filter(globals, squareFeature(14, 6, 3), canonical)).toBe(false);
  });

  it('combines only when the original filter is safe to wrap', () => {
    const ours = ['!', ['<=', ['distance', { type: 'Polygon', coordinates: [] }], 0]];
    expect(combineFilters(null, ours)).toBe(ours);
    expect(combineFilters(undefined, null)).toBeNull();
    expect(combineFilters(['==', ['get', 'a'], 1], ours)).toEqual([
      'all',
      ['==', ['get', 'a'], 1],
      ours,
    ]);
    expect(combineFilters(['==', 'a', 1], ours)).toBeUndefined();
  });
});
