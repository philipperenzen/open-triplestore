// Client-side CRS handling for georeferenced 3D city models (CityJSON/CityGML).
// The backend reprojects WKT/GML *feature* geometry server-side (src/geo/crs.rs);
// model files however are fetched directly by the browser, so the vertices they
// carry must be reprojected here. proj4 with a small registry of the projected
// CRS that city models are commonly published in — all metric, so local offsets
// double as scene metres.

import proj4 from 'proj4';
import { parseWktGeometry, type WktGeometry } from '../ontology/valueType';

// EPSG definitions (proj4 strings, epsg.io). Compound 3D codes (horizontal +
// height) are aliased to their horizontal member — heights are metres in all of
// them, which is what the scene needs.
const EPSG_DEFS: Record<number, string> = {
  // Netherlands — Amersfoort / RD New (CityJSON's most common CRS; 7415 = +NAP).
  28992:
    '+proj=sterea +lat_0=52.15616055555555 +lon_0=5.38763888888889 +k=0.9999079 ' +
    '+x_0=155000 +y_0=463000 +ellps=bessel ' +
    '+towgs84=565.417,50.3319,465.552,-0.398957,0.343988,-1.8774,4.0725 +units=m +no_defs',
  // France — RGF93 / Lambert-93.
  2154:
    '+proj=lcc +lat_1=49 +lat_2=44 +lat_0=46.5 +lon_0=3 +x_0=700000 +y_0=6600000 ' +
    '+ellps=GRS80 +towgs84=0,0,0,0,0,0,0 +units=m +no_defs',
  // Great Britain — OSGB36 / British National Grid.
  27700:
    '+proj=tmerc +lat_0=49 +lon_0=-2 +k=0.9996012717 +x_0=400000 +y_0=-100000 ' +
    '+ellps=airy +towgs84=446.448,-125.157,542.06,0.15,0.247,0.842,-20.489 +units=m +no_defs',
  // Belgium — BD72 / Belgian Lambert 72.
  31370:
    '+proj=lcc +lat_1=51.16666723333333 +lat_2=49.8333339 +lat_0=90 ' +
    '+lon_0=4.367486666666666 +x_0=150000.013 +y_0=5400088.438 +ellps=intl ' +
    '+towgs84=-106.869,52.2978,-103.724,0.3366,-0.457,1.8422,-1.2747 +units=m +no_defs',
  // Switzerland — CH1903+ / LV95.
  2056:
    '+proj=somerc +lat_0=46.95240555555556 +lon_0=7.439583333333333 +k_0=1 ' +
    '+x_0=2600000 +y_0=1200000 +ellps=bessel +towgs84=674.374,15.056,405.346,0,0,0,0 ' +
    '+units=m +no_defs',
};

// Compound (3D) codes → their horizontal CRS.
const COMPOUND_ALIASES: Record<number, number> = {
  7415: 28992, // RD New + NAP
  7416: 28992,
  5554: 25831, // ETRS89 UTM + DHHN92 (DE)
  5555: 25832,
  5556: 25833,
  9286: 25832, // ETRS89 UTM + EVRF2019
};

/** proj4 definition for an EPSG code, deriving parametric families (UTM). */
function defFor(epsg: number): string | null {
  if (EPSG_DEFS[epsg]) return EPSG_DEFS[epsg];
  // ETRS89 / UTM zones 25828-25838 (Europe).
  if (epsg >= 25828 && epsg <= 25838) {
    return `+proj=utm +zone=${epsg - 25800} +ellps=GRS80 +towgs84=0,0,0,0,0,0,0 +units=m +no_defs`;
  }
  // WGS 84 / UTM north (326xx) and south (327xx).
  if (epsg >= 32601 && epsg <= 32660) {
    return `+proj=utm +zone=${epsg - 32600} +datum=WGS84 +units=m +no_defs`;
  }
  if (epsg >= 32701 && epsg <= 32760) {
    return `+proj=utm +zone=${epsg - 32700} +south +datum=WGS84 +units=m +no_defs`;
  }
  // NAD83 / UTM zones (North America).
  if (epsg >= 26901 && epsg <= 26923) {
    return `+proj=utm +zone=${epsg - 26900} +ellps=GRS80 +towgs84=0,0,0,0,0,0,0 +units=m +no_defs`;
  }
  return null;
}

/**
 * Extract an EPSG code from any of the common CRS reference spellings:
 * `EPSG:28992`, `urn:ogc:def:crs:EPSG::28992`,
 * `https://www.opengis.net/def/crs/EPSG/0/7415`.
 */
export function epsgFromReference(ref: string | null | undefined): number | null {
  if (!ref) return null;
  const m = /EPSG[/:](?:[\d.]*[/:])?(\d+)\s*$/i.exec(String(ref).trim());
  if (!m) return null;
  const code = Number(m[1]);
  return Number.isFinite(code) ? code : null;
}

/**
 * A converter from `epsg` coordinates to `[lon, lat]` (WGS84), or null when the
 * code is unsupported. `4326`/CRS84 and `3857` work out of the box.
 */
export function toLonLat(epsg: number): ((xy: [number, number]) => [number, number]) | null {
  const code = COMPOUND_ALIASES[epsg] ?? epsg;
  if (code === 4326 || code === 4979) return (xy) => xy; // already lon/lat
  let name = `EPSG:${code}`;
  if (code !== 3857 && code !== 900913) {
    const def = defFor(code);
    if (!def) return null;
    if (!proj4.defs(name)) proj4.defs(name, def);
  }
  const transform = proj4(name, 'EPSG:4326');
  return (xy) => transform.forward(xy) as [number, number];
}

const EARTH_RADIUS = 6378137;

/**
 * Local east/north offsets (metres) of `[lon, lat]` from `origin` — a flat-earth
 * approximation that is centimetre-accurate over the few km a city model spans.
 */
/** Recursively map every [x, y] coordinate of a parsed WKT geometry. */
function mapGeometryCoords(g: WktGeometry, fn: (xy: [number, number]) => [number, number]): WktGeometry {
  switch (g.kind) {
    case 'point':
      return { kind: 'point', coord: fn(g.coord) };
    case 'multipoint':
      return { kind: 'multipoint', coords: g.coords.map(fn) };
    case 'linestring':
      return { kind: 'linestring', coords: g.coords.map(fn) };
    case 'multilinestring':
      return { kind: 'multilinestring', lines: g.lines.map((l) => l.map(fn)) };
    case 'polygon':
      return { kind: 'polygon', rings: g.rings.map((r) => r.map(fn)) };
    case 'multipolygon':
      return { kind: 'multipolygon', polygons: g.polygons.map((p) => p.map((r) => r.map(fn))) };
    case 'geometrycollection':
      return { kind: 'geometrycollection', geometries: g.geometries.map((sub) => mapGeometryCoords(sub, fn)) };
  }
}

/**
 * Parse a GeoSPARQL WKT literal into a WGS84 geometry, honouring its optional
 * `<crs-uri>` prefix: projected-CRS coordinates (e.g. the Waalbrug demo's
 * EPSG:28992) are reprojected client-side, so map previews that receive raw
 * literals (RdfTerm chips, resource pages, chat maps) plot correctly. Unknown
 * CRS fall back to plotting as-is (the old behaviour).
 */
export function parseWktAsWgs84(literal: string): WktGeometry | null {
  const g = parseWktGeometry(literal); // strips any CRS prefix itself
  if (!g) return null;
  const m = literal.match(/^\s*<([^>]+)>/);
  if (!m) return g;
  const epsg = epsgFromReference(m[1]);
  if (!epsg || epsg === 4326) return g;
  const tf = toLonLat(epsg);
  return tf ? mapGeometryCoords(g, tf) : g;
}

export function lonLatToLocalMeters(
  origin: [number, number],
  lonLat: [number, number]
): [number, number] {
  const rad = Math.PI / 180;
  const east = (lonLat[0] - origin[0]) * rad * EARTH_RADIUS * Math.cos(origin[1] * rad);
  const north = (lonLat[1] - origin[1]) * rad * EARTH_RADIUS;
  return [east, north];
}
