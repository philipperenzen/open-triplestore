// Dependency-free detection helpers shared by lightweight surfaces (RdfTerm in
// tables/graphs) — deliberately free of three.js/leaflet imports so they add
// nothing to the main bundle. The heavy viewer modules import from here too.

export type ModelFormat = 'gltf' | 'stl' | 'cityjson' | 'citygml';

/** Detect a loadable 3D-model format from a file URL (glb/gltf/stl/CityJSON/CityGML). */
export function modelFormatFromUrl(url: string): ModelFormat | null {
  // Absolute http(s) or site-relative (same-origin samples/assets).
  if (!/^https?:\/\//i.test(url) && !url.startsWith('/')) return null;
  const clean = url.split(/[?#]/)[0].toLowerCase();
  if (clean.endsWith('.glb') || clean.endsWith('.gltf')) return 'gltf';
  if (clean.endsWith('.stl')) return 'stl';
  if (clean.endsWith('.cityjson') || clean.endsWith('.city.json')) return 'cityjson';
  if (clean.endsWith('.citygml') || clean.endsWith('.gml')) return 'citygml';
  return null;
}

/** FOG format key (the local name after `fog:as`, e.g. `Gltf_v2.0-glb`) → format. */
function formatFromFogKey(key: string): ModelFormat | null {
  const k = key.toLowerCase();
  if (k.startsWith('gltf')) return 'gltf';
  if (k.startsWith('stl')) return 'stl';
  if (k.startsWith('cityjson')) return 'cityjson';
  if (k.startsWith('citygml')) return 'citygml';
  return null;
}

export interface ModelRef {
  url: string;
  format: ModelFormat;
}

/** Preference when an element offers several formats. */
const FORMAT_ORDER: ModelFormat[] = ['gltf', 'cityjson', 'citygml', 'stl'];

/**
 * Every loadable 3D-model reference of a viewer-feed element — the explicit
 * glTF URL plus the FOG file list (by FOG format key or URL extension), one per
 * format, ordered by preference (glTF > CityJSON > CityGML > STL). The inspector
 * offers these as switchable representations of the same element.
 */
export function modelRefsOf(el: {
  gltf_url?: string | null;
  files?: [string, string][];
}): ModelRef[] {
  const found = new Map<ModelFormat, string>();
  if (el.gltf_url) found.set('gltf', el.gltf_url);
  for (const [key, url] of el.files || []) {
    const format = formatFromFogKey(key) ?? modelFormatFromUrl(url);
    if (format && !found.has(format)) found.set(format, url);
  }
  return FORMAT_ORDER.flatMap((format) => {
    const url = found.get(format);
    return url ? [{ url, format }] : [];
  });
}

/**
 * The best loadable 3D-model reference of a viewer-feed element: the explicit
 * glTF URL first, then the FOG file list — by FOG format key or URL extension —
 * preferring glTF > CityJSON > CityGML > STL.
 */
export function modelRefOf(el: {
  gltf_url?: string | null;
  files?: [string, string][];
}): ModelRef | null {
  return modelRefsOf(el)[0] ?? null;
}

/** Human-readable display name per model format (file chips, BIM lists). */
export const FORMAT_LABELS: Record<ModelFormat, string> = {
  gltf: 'glTF',
  stl: 'STL',
  cityjson: 'CityJSON',
  citygml: 'CityGML',
};

/** Is this literal datatype a GeoSPARQL WKT literal? */
export function isWktDatatype(datatype: string | undefined | null): boolean {
  return !!datatype && datatype.endsWith('wktLiteral');
}

// Predicate matching mirrors the server viewer feed's resolution
// (src/geo/viewer_feed.rs) so the resource page and the feed agree on which
// triples carry geometry / BIM identity.

/** Exactly geo:hasGeometry or omg:hasGeometry — the two predicates the feed follows. */
export function isGeometryPredicate(iri: string | undefined | null): boolean {
  return (
    iri === 'http://www.opengis.net/ont/geosparql#hasGeometry' ||
    iri === 'https://w3id.org/omg#hasGeometry'
  );
}

/** IFC GlobalId predicate — case-sensitive, like the feed's `STRENDS(STR(?guidp), "ifcGuid")`. */
export function isIfcGuidPredicate(iri: string | undefined | null): boolean {
  return !!iri && iri.endsWith('ifcGuid');
}
