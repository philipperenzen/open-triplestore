import { describe, it, expect } from 'vitest';
import { parseWktZ, wktZCentroid, wktZToLocalTriangles } from '../viewer/wktz';
import { lonLatToLocalMeters } from '../viewer/crs';

// The demo's Block A shape, as the feed's wkt3d delivers it: 4326 lon/lat with
// metre heights, one bottom face + one top face + one wall.
const BLOCK =
  'POLYHEDRALSURFACE Z (' +
  '((5.8320 51.8392 0,5.8322 51.8392 0,5.8322 51.8394 0,5.8320 51.8394 0,5.8320 51.8392 0)),' +
  '((5.8320 51.8392 9,5.8320 51.8394 9,5.8322 51.8394 9,5.8322 51.8392 9,5.8320 51.8392 9)),' +
  '((5.8320 51.8392 0,5.8320 51.8392 9,5.8322 51.8392 9,5.8322 51.8392 0,5.8320 51.8392 0)))';

describe('volumetric WKT parsing', () => {
  it('parses faces, drops the WKT ring-closing repeat', () => {
    const g = parseWktZ(BLOCK);
    expect(g).toBeTruthy();
    expect(g!.faces).toHaveLength(3);
    // 5 coords in the source ring, closing repeat dropped → 4.
    for (const face of g!.faces) expect(face).toHaveLength(4);
    expect(g!.faces[1][0][2]).toBe(9); // heights preserved
  });

  it('rejects non-volumetric and malformed input', () => {
    expect(parseWktZ('POINT(5.83 51.84)')).toBeNull();
    expect(parseWktZ(null)).toBeNull();
    expect(parseWktZ('POLYHEDRALSURFACE Z (((1 2,3 4)))')).toBeNull(); // 2-component coords
  });

  it('centroid sits inside the footprint', () => {
    const [lon, lat] = wktZCentroid(parseWktZ(BLOCK)!);
    expect(lon).toBeGreaterThan(5.8319);
    expect(lon).toBeLessThan(5.8323);
    expect(lat).toBeGreaterThan(51.8391);
    expect(lat).toBeLessThan(51.8395);
  });

  it('fan-triangulates into scene-space metres (x east, y up, z south)', () => {
    const g = parseWktZ(BLOCK)!;
    const anchor = wktZCentroid(g);
    const pos = wktZToLocalTriangles(g, anchor, lonLatToLocalMeters);
    // 3 faces × (4-vertex ring → 2 triangles) × 3 vertices × 3 components.
    expect(pos.length).toBe(3 * 2 * 3 * 3);
    // Heights land on Y.
    const ys = [];
    for (let i = 1; i < pos.length; i += 3) ys.push(pos[i]);
    expect(Math.min(...ys)).toBe(0);
    expect(Math.max(...ys)).toBe(9);
    // The ~14 m × ~22 m footprint stays metre-scaled (no degree-sized values).
    const xs = [];
    for (let i = 0; i < pos.length; i += 3) xs.push(pos[i]);
    const spanX = Math.max(...xs) - Math.min(...xs);
    expect(spanX).toBeGreaterThan(5);
    expect(spanX).toBeLessThan(50);
  });
});
