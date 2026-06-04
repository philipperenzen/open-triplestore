// Detect the best ValueRenderer variant for an RDF term.
// Input: { type: 'uri'|'bnode'|'literal', value, datatype?, lang? } (SPARQL JSON binding shape)
// Output: { kind, hints? }   kind ∈ 'iri'|'image'|'geo'|'sparql'|'turtle'|'date'|'number'|
//                                   'url'|'lang'|'html'|'markdown'|'bool'|'color'|'text'

const XSD = 'http://www.w3.org/2001/XMLSchema#';
const GEO = 'http://www.opengis.net/ont/geosparql#';
const SH = 'http://www.w3.org/ns/shacl#';
const RDF_LANG_STRING = 'http://www.w3.org/1999/02/22-rdf-syntax-ns#langString';

const IMAGE_EXT = /\.(png|jpe?g|gif|svg|webp|avif|bmp)(\?.*)?$/i;
const IMAGE_PREDICATES = new Set([
  'http://xmlns.com/foaf/0.1/img',
  'http://xmlns.com/foaf/0.1/depiction',
  'http://schema.org/image',
  'http://www.w3.org/ns/dcat#thumbnail',
]);

// Any WKT geometry type, optionally prefixed by a CRS URI (`<http://…/CRS84> POINT(…)`)
// and carrying an optional Z/M dimensionality flag, including the `… EMPTY` form.
const WKT_HEAD = /^\s*(?:<[^>]*>\s*)?(?:POINT|MULTIPOINT|LINESTRING|MULTILINESTRING|POLYGON|MULTIPOLYGON|GEOMETRYCOLLECTION|TRIANGLE|TIN|POLYHEDRALSURFACE|CIRCULARSTRING|CURVEPOLYGON)\s*(?:Z|M|ZM)?\s*(?:\(|EMPTY)/i;

const NUMERIC_DATATYPES = new Set([
  'integer', 'decimal', 'double', 'float', 'long', 'int', 'short', 'byte',
  'nonNegativeInteger', 'nonPositiveInteger', 'negativeInteger', 'positiveInteger',
  'unsignedLong', 'unsignedInt', 'unsignedShort', 'unsignedByte',
].map(t => XSD + t));

const TEMPORAL_DATATYPES = new Set([
  'date', 'dateTime', 'dateTimeStamp', 'time',
  'gYear', 'gYearMonth', 'gMonth', 'gMonthDay', 'gDay',
].map(t => XSD + t));

interface RdfTerm {
  type: string;
  value?: string;
  datatype?: string;
  lang?: string;
  'xml:lang'?: string;
}

export function detectValueKind(term: RdfTerm | null | undefined, predicateIri = ''): { kind: string; [key: string]: unknown } {
  if (!term) return { kind: 'text' };
  if (term.type === 'bnode') return { kind: 'bnode' };
  if (term.type === 'uri' || term.type === 'iri') {
    if (IMAGE_PREDICATES.has(predicateIri) || IMAGE_EXT.test(term.value)) {
      return { kind: 'image' };
    }
    if (/^https?:\/\//i.test(term.value)) return { kind: 'url' };
    return { kind: 'iri' };
  }
  // literal
  const dt = term.datatype || '';
  const lang = term.lang || term['xml:lang'] || (term as any).language || '';
  const v = String(term.value ?? '');
  // Attach the datatype to every literal verdict so the renderer can surface it.
  const dtag = dt || undefined;

  if (dt === GEO + 'wktLiteral' || WKT_HEAD.test(v)) {
    return { kind: 'geo', format: 'wkt', datatype: dtag, geometry: wktKind(v) };
  }
  if (dt === GEO + 'gmlLiteral' || /^\s*<gml:/i.test(v)) {
    return { kind: 'geo', format: 'gml', datatype: dtag };
  }
  if (predicateIri === SH + 'select' || predicateIri === SH + 'ask' ||
      (/\bSELECT\s|\bCONSTRUCT\s|\bASK\s|\bDESCRIBE\s/i.test(v) && v.includes('{'))) {
    return { kind: 'sparql', datatype: dtag };
  }
  if (dt === XSD + 'boolean' || (!dt && (v === 'true' || v === 'false'))) return { kind: 'bool', datatype: dtag };
  if (TEMPORAL_DATATYPES.has(dt)) return { kind: 'date', datatype: dtag };
  if (dt === XSD + 'duration' || dt === XSD + 'dayTimeDuration' || dt === XSD + 'yearMonthDuration') {
    return { kind: 'duration', datatype: dtag };
  }
  if (NUMERIC_DATATYPES.has(dt)) return { kind: 'number', datatype: dtag };
  if (dt === RDF_HTML) return { kind: 'html', datatype: dtag };
  if (dt === XSD + 'base64Binary' || dt === XSD + 'hexBinary') return { kind: 'binary', datatype: dtag };
  if (/^#[0-9a-f]{3,8}$/i.test(v)) return { kind: 'color', datatype: dtag };
  if (lang) return { kind: 'lang', lang, datatype: dt && dt !== RDF_LANG_STRING ? dt : undefined };
  if (dt === XSD + 'anyURI' || /^https?:\/\//i.test(v)) return { kind: 'url', datatype: dtag };
  if (v.length > 200 || /\n/.test(v)) return { kind: 'text', long: true, datatype: dtag };
  return { kind: 'text', datatype: dtag };
}

/** Lightweight WKT type sniff for labelling (returns 'point', 'polygon', …). */
export function wktKind(wkt: string): string {
  const m = /^\s*(?:<[^>]*>\s*)?([A-Z]+)/i.exec(wkt || '');
  return m ? m[1].toLowerCase() : '';
}

const XSD_NS = 'http://www.w3.org/2001/XMLSchema#';
/** Friendly short label for a datatype IRI, e.g. `xsd:integer`, `geo:wktLiteral`. */
export function datatypeLabel(dt: string | undefined | null): string {
  if (!dt) return '';
  if (dt.startsWith(XSD_NS)) return 'xsd:' + dt.slice(XSD_NS.length);
  if (dt.startsWith(GEO)) return 'geo:' + dt.slice(GEO.length);
  if (dt === RDF_LANG_STRING) return 'rdf:langString';
  if (dt === RDF_HTML) return 'rdf:HTML';
  const i = Math.max(dt.lastIndexOf('#'), dt.lastIndexOf('/'));
  return i >= 0 ? dt.slice(i + 1) : dt;
}

/**
 * Saturated mid-tone fills that read on both light and dark backgrounds when
 * paired with white text. Shared by the graph canvas literal badge.
 */
export const LITERAL_BADGE_COLORS = {
  lang: '#6366f1', // indigo
  string: '#64748b', // slate
  number: '#10b981', // emerald
  boolean: '#f59e0b', // amber
  date: '#0ea5e9', // sky
  uri: '#8b5cf6', // violet
  geo: '#14b8a6', // teal
  other: '#94a3b8', // slate (light)
} as const;

export interface LiteralBadge {
  /** Short label, e.g. '@en', 'num', 'date'. */
  text: string;
  kind: 'lang' | 'datatype';
  /** Fill color (hex); pair with white text. */
  color: string;
}

/**
 * A short, color-coded descriptor of a literal's language tag or datatype, for
 * the corner indicator on graph literal nodes. Language tag takes precedence.
 * Returns null when there is nothing meaningful to show (plain string).
 */
export function literalBadge(datatype?: string | null, language?: string | null): LiteralBadge | null {
  const C = LITERAL_BADGE_COLORS;
  const lang = (language || '').trim();
  if (lang) return { text: `@${lang.toLowerCase()}`, kind: 'lang', color: C.lang };

  const dt = (datatype || '').trim();
  if (!dt || dt === XSD + 'string') return { text: 'str', kind: 'datatype', color: C.string };
  if (dt === RDF_LANG_STRING) return { text: '@', kind: 'lang', color: C.lang };
  if (NUMERIC_DATATYPES.has(dt)) return { text: 'num', kind: 'datatype', color: C.number };
  if (dt === XSD + 'boolean') return { text: 'bool', kind: 'datatype', color: C.boolean };
  if (
    TEMPORAL_DATATYPES.has(dt) ||
    dt === XSD + 'duration' || dt === XSD + 'dayTimeDuration' || dt === XSD + 'yearMonthDuration'
  ) {
    return { text: 'date', kind: 'datatype', color: C.date };
  }
  if (dt === XSD + 'anyURI') return { text: 'uri', kind: 'datatype', color: C.uri };
  if (dt === GEO + 'wktLiteral' || dt === GEO + 'gmlLiteral') return { text: 'geo', kind: 'datatype', color: C.geo };

  const local = (dt.split('#').pop() || dt.split('/').pop() || dt).toLowerCase();
  return { text: local.slice(0, 3) || '?', kind: 'datatype', color: C.other };
}

const RDF_HTML = 'http://www.w3.org/1999/02/22-rdf-syntax-ns#HTML';

export function shortenIri(iri: string): string {
  if (!iri) return '';
  const i = Math.max(iri.lastIndexOf('#'), iri.lastIndexOf('/'));
  return i >= 0 ? iri.slice(i + 1) || iri : iri;
}

export function parseWkt(wkt: string): [number, number] | null {
  // Minimal POINT parser — returns [lng, lat] or null.
  const m = /^\s*(?:<[^>]*>\s*)?POINT\s*\(\s*([-\d.]+)\s+([-\d.]+)/i.exec(wkt);
  if (!m) return null;
  return [parseFloat(m[1]), parseFloat(m[2])];
}

export type WktGeometry =
  | { kind: 'point'; coord: [number, number] }
  | { kind: 'linestring'; coords: [number, number][] }
  | { kind: 'polygon'; rings: [number, number][][] }
  | { kind: 'multipoint'; coords: [number, number][] }
  | { kind: 'multilinestring'; lines: [number, number][][] }
  | { kind: 'multipolygon'; polygons: [number, number][][][] }
  | { kind: 'geometrycollection'; geometries: WktGeometry[] };

function parseCoordList(s: string): [number, number][] {
  return s.split(',').map(pair => {
    const m = pair.trim().match(/^([-\d.eE+]+)\s+([-\d.eE+]+)/);
    if (!m) return null;
    return [parseFloat(m[1]), parseFloat(m[2])] as [number, number];
  }).filter(Boolean) as [number, number][];
}

// Split `s` on the commas that sit at parenthesis depth 0.
function splitTopLevelCommas(s: string): string[] {
  const out: string[] = [];
  let depth = 0, start = 0;
  for (let i = 0; i < s.length; i++) {
    const c = s[i];
    if (c === '(') depth++;
    else if (c === ')') depth--;
    else if (c === ',' && depth === 0) { out.push(s.slice(start, i)); start = i + 1; }
  }
  out.push(s.slice(start));
  return out;
}

// Return the contents of each top-level `(...)` group, in order.
function topLevelGroups(s: string): string[] {
  const out: string[] = [];
  let depth = 0, start = -1;
  for (let i = 0; i < s.length; i++) {
    const c = s[i];
    if (c === '(') { if (depth === 0) start = i + 1; depth++; }
    else if (c === ')') { depth--; if (depth === 0 && start >= 0) { out.push(s.slice(start, i)); start = -1; } }
  }
  return out;
}

export function parseWktGeometry(wkt: string): WktGeometry | null {
  if (!wkt) return null;
  // Strip optional CRS URI prefix (`<http://.../CRS84> POINT(...)`) and Z/M flags.
  const stripped = wkt.replace(/^\s*<[^>]*>\s*/, '').trim();
  const head = stripped.match(/^([A-Z]+)\s*(?:Z|M|ZM)?\s*\((.*)\)\s*$/is);
  if (!head) return null;
  const kind = head[1].toUpperCase();
  const body = head[2];
  if (kind === 'POINT') {
    const m = body.match(/^\s*([-\d.eE+]+)\s+([-\d.eE+]+)/);
    if (!m) return null;
    return { kind: 'point', coord: [parseFloat(m[1]), parseFloat(m[2])] };
  }
  if (kind === 'LINESTRING' || kind === 'CIRCULARSTRING') {
    return { kind: 'linestring', coords: parseCoordList(body) };
  }
  if (kind === 'MULTIPOINT') {
    // MULTIPOINT((x y), (x y)) or MULTIPOINT(x y, x y)
    const clean = body.replace(/[()]/g, '');
    return { kind: 'multipoint', coords: parseCoordList(clean) };
  }
  if (kind === 'POLYGON' || kind === 'TRIANGLE' || kind === 'CURVEPOLYGON') {
    return { kind: 'polygon', rings: topLevelGroups(body).map(parseCoordList) };
  }
  if (kind === 'MULTILINESTRING') {
    return { kind: 'multilinestring', lines: topLevelGroups(body).map(parseCoordList) };
  }
  if (kind === 'MULTIPOLYGON' || kind === 'POLYHEDRALSURFACE' || kind === 'TIN') {
    const polygons = topLevelGroups(body).map(poly => topLevelGroups(poly).map(parseCoordList));
    return { kind: 'multipolygon', polygons };
  }
  if (kind === 'GEOMETRYCOLLECTION') {
    const geometries = splitTopLevelCommas(body)
      .map(part => parseWktGeometry(part.trim()))
      .filter(Boolean) as WktGeometry[];
    return { kind: 'geometrycollection', geometries };
  }
  return null;
}

/** Every [lng, lat] coordinate in a geometry, flattened (for fitting map bounds). */
export function geometryCoords(g: WktGeometry | null): [number, number][] {
  if (!g) return [];
  switch (g.kind) {
    case 'point': return [g.coord];
    case 'linestring':
    case 'multipoint': return g.coords;
    case 'polygon': return g.rings.flat();
    case 'multilinestring': return g.lines.flat();
    case 'multipolygon': return g.polygons.flat(2);
    case 'geometrycollection': return g.geometries.flatMap(geometryCoords);
  }
}
