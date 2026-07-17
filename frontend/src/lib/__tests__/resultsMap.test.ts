// Scope-aware Map tab gating for the result explorers: detectGeoBindings decides
// whether the SPARQL editor / Triple Browser may offer a Map view, and
// resultsToViewerElements converts the spatial rows into ViewerElements for the
// shared map. Exercised through the public rdf-utils re-export, the same import
// path the Svelte pages use.
import { describe, it, expect } from 'vitest';
import { detectGeoBindings, resultsToViewerElements, triplesToResults } from '../rdf-utils.js';

const WKT = 'http://www.opengis.net/ont/geosparql#wktLiteral';

// Helper: build a minimal SELECT-results object.
const sel = (vars: string[], rows: Record<string, any>[]) => ({
  head: { vars },
  results: { bindings: rows },
});

const wkt = (value: string) => ({ type: 'literal', value, datatype: WKT });
const uri = (value: string) => ({ type: 'uri', value });
const lit = (value: string, datatype?: string) => ({ type: 'literal', value, ...(datatype ? { datatype } : {}) });

describe('detectGeoBindings', () => {
  it('returns true when a binding is a geo:wktLiteral', () => {
    const r = sel(['s', 'geom'], [
      { s: uri('http://ex/a'), geom: wkt('POINT(5.86 51.85)') },
    ]);
    expect(detectGeoBindings(r)).toBe(true);
  });

  it('returns true for plausible lat + lon numeric columns', () => {
    const r = sel(['city', 'lat', 'lon'], [
      { city: uri('http://ex/nijmegen'), lat: lit('51.85'), lon: lit('5.86') },
    ]);
    expect(detectGeoBindings(r)).toBe(true);
  });

  it('also recognises latitude/longitude spelled out', () => {
    const r = sel(['latitude', 'longitude'], [
      { latitude: lit('51.85'), longitude: lit('5.86') },
    ]);
    expect(detectGeoBindings(r)).toBe(true);
  });

  it('returns false when only one of lat/lon is present', () => {
    const r = sel(['name', 'lat'], [
      { name: lit('Nijmegen'), lat: lit('51.85') },
    ]);
    expect(detectGeoBindings(r)).toBe(false);
  });

  it('returns false for a lat/lon-named column with non-numeric values', () => {
    const r = sel(['lat', 'lon'], [
      { lat: lit('north'), lon: lit('east') },
    ]);
    expect(detectGeoBindings(r)).toBe(false);
  });

  it('returns false for ordinary non-spatial results', () => {
    const r = sel(['s', 'p', 'o'], [
      { s: uri('http://ex/a'), p: uri('http://ex/name'), o: lit('Alice') },
    ]);
    expect(detectGeoBindings(r)).toBe(false);
  });

  it('returns false for empty / missing result sets', () => {
    expect(detectGeoBindings(null)).toBe(false);
    expect(detectGeoBindings({})).toBe(false);
    expect(detectGeoBindings(sel(['geom'], []))).toBe(false);
  });
});

describe('resultsToViewerElements', () => {
  it('builds an element per WKT row, keyed on the row IRI and labelled', () => {
    const r = sel(['s', 'label', 'geom'], [
      { s: uri('http://ex/id/Bridge'), label: lit('Waalbrug'), geom: wkt('POINT(5.86 51.85)') },
    ]);
    const els = resultsToViewerElements(r);
    expect(els).toHaveLength(1);
    expect(els[0].id).toBe('http://ex/id/Bridge');
    expect(els[0].label).toBe('Waalbrug');
    expect(els[0].wkt4326).toBe('POINT(5.86 51.85)');
  });

  it('falls back to the IRI local name when no label column is present', () => {
    const r = sel(['s', 'geom'], [
      { s: uri('http://ex/id/Tower'), geom: wkt('POINT(1 2)') },
    ]);
    expect(resultsToViewerElements(r)[0].label).toBe('Tower');
  });

  it('synthesises an id when the row has no IRI', () => {
    const r = sel(['geom'], [
      { geom: wkt('POINT(1 2)') },
    ]);
    const els = resultsToViewerElements(r);
    expect(els).toHaveLength(1);
    expect(els[0].id).toMatch(/^row:/);
  });

  it('strips a WGS84 CRS prefix and keeps the bare WKT', () => {
    const r = sel(['s', 'geom'], [
      { s: uri('http://ex/id/P'), geom: wkt('<http://www.opengis.net/def/crs/EPSG/0/4326> POINT(5.86 51.85)') },
    ]);
    expect(resultsToViewerElements(r)[0].wkt4326).toBe('POINT(5.86 51.85)');
  });

  it('skips a projected (non-WGS84) CRS rather than mis-plotting it', () => {
    // EPSG:28992 (RD New) — recognised projected CRS, not reprojected here.
    const r = sel(['s', 'geom'], [
      { s: uri('http://ex/id/RD'), geom: wkt('<http://www.opengis.net/def/crs/EPSG/0/28992> POINT(187420 428470)') },
    ]);
    expect(resultsToViewerElements(r)).toHaveLength(0);
  });

  it('drops rows without any WKT geometry', () => {
    const r = sel(['s', 'name'], [
      { s: uri('http://ex/a'), name: lit('Alice') },
    ]);
    expect(resultsToViewerElements(r)).toHaveLength(0);
  });
});

describe('triplesToResults (Triple Browser adapter)', () => {
  it('adapts a triple list to the SELECT-results shape and gates correctly', () => {
    const triples = [
      { subject: uri('http://ex/id/Bridge'), predicate: uri('http://www.opengis.net/ont/geosparql#asWKT'), object: wkt('POINT(5.86 51.85)') },
      { subject: uri('http://ex/id/Bridge'), predicate: uri('http://ex/name'), object: lit('Waalbrug') },
    ];
    const r = triplesToResults(triples);
    expect(r.head?.vars).toEqual(['subject', 'predicate', 'object']);
    expect(detectGeoBindings(r)).toBe(true);
    const els = resultsToViewerElements(r);
    // One row carries the WKT object → one element; the other triple has no geometry.
    expect(els).toHaveLength(1);
    expect(els[0].wkt4326).toBe('POINT(5.86 51.85)');
  });

  it('produces no mappable results for non-spatial triples', () => {
    const r = triplesToResults([
      { subject: uri('http://ex/a'), predicate: uri('http://ex/p'), object: lit('x') },
    ]);
    expect(detectGeoBindings(r)).toBe(false);
  });
});
