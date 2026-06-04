import { describe, it, expect } from 'vitest';
import {
  detectValueKind,
  parseWktGeometry,
  geometryCoords,
  datatypeLabel,
  wktKind,
  literalBadge,
} from '../ontology/valueType.js';

const GEO = 'http://www.opengis.net/ont/geosparql#';
const XSD = 'http://www.w3.org/2001/XMLSchema#';

describe('detectValueKind', () => {
  it('classifies a blank node as its own kind (so it can expand inline)', () => {
    expect(detectValueKind({ type: 'bnode', value: 'b0' }).kind).toBe('bnode');
  });

  it('detects WKT geometry by datatype and by leading keyword', () => {
    expect(detectValueKind({ type: 'literal', value: 'POINT(4.9 52.3)', datatype: GEO + 'wktLiteral' }).kind).toBe('geo');
    expect(detectValueKind({ type: 'literal', value: 'POLYGON((0 0, 1 0, 1 1, 0 0))' }).kind).toBe('geo');
    expect(detectValueKind({ type: 'literal', value: 'MULTIPOLYGON(((0 0,1 0,1 1,0 0)))' }).geometry).toBe('multipolygon');
  });

  it('detects a CRS-prefixed WKT literal', () => {
    const d = detectValueKind({ type: 'literal', value: '<http://www.opengis.net/def/crs/OGC/1.3/CRS84> POINT(4.9 52.3)' });
    expect(d.kind).toBe('geo');
  });

  it('detects GML geometry', () => {
    expect(detectValueKind({ type: 'literal', value: 'x', datatype: GEO + 'gmlLiteral' }).format).toBe('gml');
  });

  it('surfaces the datatype on numeric and temporal literals', () => {
    const num = detectValueKind({ type: 'literal', value: '42', datatype: XSD + 'integer' });
    expect(num.kind).toBe('number');
    expect(num.datatype).toBe(XSD + 'integer');

    expect(detectValueKind({ type: 'literal', value: '2020', datatype: XSD + 'gYear' }).kind).toBe('date');
    expect(detectValueKind({ type: 'literal', value: 'P3Y', datatype: XSD + 'duration' }).kind).toBe('duration');
    expect(detectValueKind({ type: 'literal', value: 'AQID', datatype: XSD + 'base64Binary' }).kind).toBe('binary');
  });

  it('treats anyURI and http(s) strings as URLs, and keeps language tags', () => {
    expect(detectValueKind({ type: 'literal', value: 'https://x.test', datatype: XSD + 'anyURI' }).kind).toBe('url');
    const lang = detectValueKind({ type: 'literal', value: 'Amsterdam', 'xml:lang': 'nl' });
    expect(lang.kind).toBe('lang');
    expect(lang.lang).toBe('nl');
  });
});

describe('parseWktGeometry', () => {
  it('parses a POINT (returns [lng, lat])', () => {
    expect(parseWktGeometry('POINT(4.9041 52.3676)')).toEqual({ kind: 'point', coord: [4.9041, 52.3676] });
  });

  it('strips a CRS prefix and a Z dimensionality flag', () => {
    expect(parseWktGeometry('<http://crs> POINT Z (1 2 3)')).toEqual({ kind: 'point', coord: [1, 2] });
  });

  it('parses a POLYGON with a hole', () => {
    const g = parseWktGeometry('POLYGON((0 0, 4 0, 4 4, 0 4, 0 0), (1 1, 2 1, 2 2, 1 1))');
    expect(g?.kind).toBe('polygon');
    expect(g && g.kind === 'polygon' && g.rings.length).toBe(2);
  });

  it('parses a MULTIPOLYGON with nested rings', () => {
    const g = parseWktGeometry('MULTIPOLYGON(((0 0,1 0,1 1,0 0)), ((2 2,3 2,3 3,2 2)))');
    expect(g?.kind).toBe('multipolygon');
    expect(g && g.kind === 'multipolygon' && g.polygons.length).toBe(2);
  });

  it('parses a MULTILINESTRING', () => {
    const g = parseWktGeometry('MULTILINESTRING((0 0, 1 1), (2 2, 3 3))');
    expect(g?.kind).toBe('multilinestring');
    expect(g && g.kind === 'multilinestring' && g.lines.length).toBe(2);
  });

  it('parses a GEOMETRYCOLLECTION recursively', () => {
    const g = parseWktGeometry('GEOMETRYCOLLECTION(POINT(1 2), POLYGON((0 0,1 0,1 1,0 0)))');
    expect(g?.kind).toBe('geometrycollection');
    expect(g && g.kind === 'geometrycollection' && g.geometries.map(x => x.kind)).toEqual(['point', 'polygon']);
  });

  it('returns null for non-geometry text', () => {
    expect(parseWktGeometry('not a geometry')).toBeNull();
  });
});

describe('geometryCoords', () => {
  it('flattens coordinates across every geometry kind', () => {
    expect(geometryCoords(parseWktGeometry('POINT(1 2)')).length).toBe(1);
    expect(geometryCoords(parseWktGeometry('MULTIPOLYGON(((0 0,1 0,1 1,0 0)))')).length).toBe(4);
    expect(geometryCoords(parseWktGeometry('GEOMETRYCOLLECTION(POINT(1 2), POINT(3 4))')).length).toBe(2);
  });
});

describe('datatypeLabel', () => {
  it('shortens well-known datatype IRIs', () => {
    expect(datatypeLabel(XSD + 'integer')).toBe('xsd:integer');
    expect(datatypeLabel(GEO + 'wktLiteral')).toBe('geo:wktLiteral');
    expect(datatypeLabel('http://www.w3.org/1999/02/22-rdf-syntax-ns#langString')).toBe('rdf:langString');
    expect(datatypeLabel('')).toBe('');
  });
});

describe('wktKind', () => {
  it('extracts the leading geometry keyword, lowercased', () => {
    expect(wktKind('POINT(1 2)')).toBe('point');
    expect(wktKind('<http://crs> MULTIPOLYGON(((0 0)))')).toBe('multipolygon');
  });
});

describe('literalBadge', () => {
  it('prefers the language tag over the datatype', () => {
    const b = literalBadge(XSD + 'string', 'EN');
    expect(b?.kind).toBe('lang');
    expect(b?.text).toBe('@en');
  });

  it('abbreviates common datatypes', () => {
    expect(literalBadge(XSD + 'integer')?.text).toBe('num');
    expect(literalBadge(XSD + 'decimal')?.text).toBe('num');
    expect(literalBadge(XSD + 'boolean')?.text).toBe('bool');
    expect(literalBadge(XSD + 'date')?.text).toBe('date');
    expect(literalBadge(XSD + 'dateTime')?.text).toBe('date');
    expect(literalBadge(XSD + 'anyURI')?.text).toBe('uri');
    expect(literalBadge(GEO + 'wktLiteral')?.text).toBe('geo');
  });

  it('treats a missing or xsd:string datatype as a plain string', () => {
    expect(literalBadge(null, null)?.text).toBe('str');
    expect(literalBadge(XSD + 'string')?.text).toBe('str');
  });

  it('falls back to a 3-char abbreviation for unknown datatypes', () => {
    expect(literalBadge('http://example.org/ns#SpecialThing')?.text).toBe('spe');
  });

  it('always carries a color', () => {
    expect(literalBadge(XSD + 'integer')?.color).toMatch(/^#[0-9a-f]{6}$/i);
    expect(literalBadge(null, 'nl')?.color).toMatch(/^#[0-9a-f]{6}$/i);
  });
});
