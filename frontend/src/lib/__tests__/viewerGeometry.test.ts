import { describe, it, expect } from 'vitest';
import { toMapFeature, featureBounds, modelRefs } from '../viewer/geometry';
import type { ViewerElement } from '../viewer/geometry';

const boog: ViewerElement = {
  id: 'https://data.example.nl/id/waalbrug/Boog-Noord',
  label: 'Noordelijke boog',
  wkt4326: 'POINT(5.860 51.851)',
  gltf_url: 'https://files.example/boog-noord.glb',
  files: [['Gltf_v2.0-glb', 'https://files.example/boog-noord.glb']],
};

const trace: ViewerElement = {
  id: 'https://data.example.nl/id/waalbrug/Waalbrug',
  label: 'Waalbrug',
  wkt4326: 'LINESTRING(5.858 51.850, 5.862 51.853)',
};

const landmark: ViewerElement = {
  id: 'https://data.example.nl/id/landmarks/DragonBridge',
  label: 'Dragon Bridge',
  wkt4326: 'POINT(108.22666667 16.06111111)',
  files: [['Stl', 'http://commons.wikimedia.org/wiki/Special:FilePath/Dragon%20Bridge%20in%20Da%20Nang.stl']],
};

describe('viewer geometry helpers', () => {
  it('converts a WKT point to a Leaflet [lat, lng] feature', () => {
    const f = toMapFeature(boog)!;
    expect(f.kind).toBe('point');
    // WKT is (lon lat); Leaflet wants [lat, lng].
    expect(f.latlngs[0]).toEqual([51.851, 5.86]);
    expect(f.label).toBe('Noordelijke boog');
  });

  it('converts a linestring and computes bounds over all features', () => {
    const fs = [boog, trace].map(toMapFeature).map((f) => f!);
    expect(fs[1].kind).toBe('line');
    const b = featureBounds(fs)!;
    expect(b[0][0]).toBeCloseTo(51.85); // min lat
    expect(b[1][1]).toBeCloseTo(5.862); // max lng
  });

  it('returns null for elements without geometry', () => {
    expect(toMapFeature({ id: 'x' })).toBeNull();
    expect(featureBounds([])).toBeNull();
  });

  it('flattens a GEOMETRYCOLLECTION so bounds cover all members', () => {
    const f = toMapFeature({
      id: 'gc',
      wkt4326: 'GEOMETRYCOLLECTION(POINT(4 52), LINESTRING(5 53, 6 54))',
    })!;
    expect(f).not.toBeNull();
    expect(f.kind).toBe('geometrycollection');
    expect(f.latlngs).toEqual([
      [52, 4],
      [53, 5],
      [54, 6],
    ]);
    const b = featureBounds([f])!;
    expect(b).toEqual([
      [52, 4], // min lat / min lng
      [54, 6], // max lat / max lng
    ]);
  });

  it('ignores out-of-range coordinates when computing bounds', () => {
    // A geometry whose CRS was mishandled upstream (metres, not degrees) must
    // not produce bounds that would make the map's fitBounds throw.
    const bad = { id: 'bad', label: 'bad', kind: 'point' as const, latlngs: [[5338000, 167000]] as [number, number][] };
    expect(featureBounds([bad])).toBeNull();
    const good = toMapFeature(boog)!;
    const b = featureBounds([bad, good])!;
    expect(b[0]).toEqual([51.851, 5.86]);
    expect(b[1]).toEqual([51.851, 5.86]);
  });

  it('collects model refs preferring glTF, falling back to STL, on a grid', () => {
    const refs = modelRefs([boog, trace, landmark]);
    expect(refs).toHaveLength(2); // trace has no model
    expect(refs[0].format).toBe('gltf');
    expect(refs[1].format).toBe('stl');
    expect(refs[1].url).toContain('Dragon%20Bridge');
    // Distinct grid slots.
    expect(refs[0].slot).not.toEqual(refs[1].slot);
  });
});
