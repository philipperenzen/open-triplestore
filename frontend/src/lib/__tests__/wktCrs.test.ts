// CRS-aware WKT parsing for client-side map plotting: projected-CRS literals
// (the Waalbrug demo publishes EPSG:28992 RD New) must land at their true
// WGS84 position, not be plotted raw. Regression for the map-chip/GeoPreview
// path — found by review: the flagship demo plotted at a nonsense location.
import { describe, it, expect } from 'vitest';
import { parseWktAsWgs84 } from '../viewer/crs';

describe('parseWktAsWgs84', () => {
  it('reprojects an EPSG:28992 point to lon/lat near Nijmegen', () => {
    const g = parseWktAsWgs84(
      '<http://www.opengis.net/def/crs/EPSG/0/28992> POINT(187420 428470)'
    );
    expect(g?.kind).toBe('point');
    if (g?.kind !== 'point') return;
    const [lon, lat] = g.coord;
    expect(lon).toBeCloseTo(5.86, 1);
    expect(lat).toBeCloseTo(51.85, 1);
  });

  it('passes plain WGS84 WKT through unchanged', () => {
    const g = parseWktAsWgs84('POINT(108.22666667 16.06111111)');
    expect(g?.kind).toBe('point');
    if (g?.kind !== 'point') return;
    expect(g.coord[0]).toBeCloseTo(108.2266, 3);
  });

  it('reprojects linestring coordinates too', () => {
    const g = parseWktAsWgs84(
      '<http://www.opengis.net/def/crs/EPSG/0/28992> LINESTRING(187320 428330, 187610 428690)'
    );
    expect(g?.kind).toBe('linestring');
    if (g?.kind !== 'linestring') return;
    for (const [lon, lat] of g.coords) {
      expect(lon).toBeGreaterThan(5.8);
      expect(lon).toBeLessThan(5.9);
      expect(lat).toBeGreaterThan(51.8);
      expect(lat).toBeLessThan(51.9);
    }
  });

  it('falls back to raw coordinates for an unknown CRS', () => {
    const g = parseWktAsWgs84('<http://example.org/unknown-crs> POINT(1 2)');
    expect(g?.kind).toBe('point');
    if (g?.kind !== 'point') return;
    expect(g.coord).toEqual([1, 2]);
  });

  it('returns null for garbage', () => {
    expect(parseWktAsWgs84('not wkt at all')).toBeNull();
  });
});
