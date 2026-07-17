// Dependency-free detection helpers shared by lightweight surfaces (RdfTerm in
// tables/graphs) — deliberately free of three.js/leaflet imports so they add
// nothing to the main bundle. The heavy viewer modules import from here too.

export type ModelFormat = 'gltf' | 'stl' | 'cityjson' | 'citygml' | 'ifc';

/** Detect a loadable 3D-model format from a file URL (glb/gltf/stl/CityJSON/CityGML). */
export function modelFormatFromUrl(url: string): ModelFormat | null {
  // Absolute http(s) or site-relative (same-origin samples/assets).
  if (!/^https?:\/\//i.test(url) && !url.startsWith('/')) return null;
  const clean = url.split(/[?#]/)[0].toLowerCase();
  if (clean.endsWith('.glb') || clean.endsWith('.gltf')) return 'gltf';
  if (clean.endsWith('.stl')) return 'stl';
  if (clean.endsWith('.cityjson') || clean.endsWith('.city.json')) return 'cityjson';
  if (clean.endsWith('.citygml') || clean.endsWith('.gml')) return 'citygml';
  if (clean.endsWith('.ifc')) return 'ifc';
  return null;
}

/** What a file-like resource string points at — drives the FileViewer's renderer. */
export type FileResourceKind = 'model3d' | 'image' | 'pdf' | 'text' | 'json' | 'binary';

export interface FileResource {
  kind: FileResourceKind;
  /** Present only when `kind === 'model3d'` — the resolved 3D model format. */
  format?: ModelFormat;
}

// Extension → renderer kind. The double-extension `.city.json` is handled by
// modelFormatFromUrl before this map is consulted, so plain `.json` lands here.
const TEXT_EXTS = new Set([
  'txt', 'csv', 'tsv', 'md', 'ttl', 'nt', 'rdf', 'xml', 'obj',
]);
const IMAGE_EXTS = new Set(['png', 'jpg', 'jpeg', 'gif', 'webp', 'svg']);
const JSON_EXTS = new Set(['json', 'geojson']);

/**
 * Classify a resource string as a *file* (vs. an RDF resource IRI) and pick the
 * renderer for it. Returns non-null only when the string looks like a file:
 *
 *   - a site-relative path (starts with `/`, `./` or `../`), or
 *   - an http(s) URL whose path (before `?`/`#`) ends in a known extension.
 *
 * Plain RDF resource IRIs with no file extension — `https://data.3dbag.nl/def/Building`,
 * `http://example.org/Thing` — return null and stay on the normal resource flow.
 *
 * Extensions map: glb/gltf/stl/cityjson/city.json/citygml/gml/ifc → model3d
 * (via {@link modelFormatFromUrl}); png/jpg/jpeg/gif/webp/svg → image; pdf → pdf;
 * json/geojson → json; txt/csv/tsv/md/ttl/nt/rdf/xml/obj → text; anything else
 * with an extension → binary.
 */
export function fileResourceKind(url: string | null | undefined): FileResource | null {
  if (!url) return null;
  const s = String(url).trim();
  if (!s) return null;

  const siteRelative = s.startsWith('/') || s.startsWith('./') || s.startsWith('../');
  const isHttp = /^https?:\/\//i.test(s);
  // Only site-relative paths and http(s) URLs can be files; bare IRIs in other
  // schemes (urn:, mailto:, _:bnode…) are never file resources here.
  if (!siteRelative && !isHttp) return null;

  // 3D models reuse the shared detector so the double-extension `.city.json`
  // and the same site-relative/http rules stay in one place.
  const format = modelFormatFromUrl(s);
  if (format) return { kind: 'model3d', format };

  // Isolate the path component, then its trailing extension.
  let path = s.split(/[?#]/)[0];
  if (isHttp) {
    try {
      path = new URL(s).pathname;
    } catch {
      // Malformed http(s) URL — fall through with the naive split above.
    }
  }
  const dot = path.lastIndexOf('.');
  const slash = path.lastIndexOf('/');
  // No extension (or the dot is in a directory segment) → an RDF IRI, not a file.
  if (dot === -1 || dot < slash || dot === path.length - 1) return null;
  const ext = path.slice(dot + 1).toLowerCase();

  if (IMAGE_EXTS.has(ext)) return { kind: 'image' };
  if (ext === 'pdf') return { kind: 'pdf' };
  if (JSON_EXTS.has(ext)) return { kind: 'json' };
  if (TEXT_EXTS.has(ext)) return { kind: 'text' };
  return { kind: 'binary' };
}

/** FOG format key (the local name after `fog:as`, e.g. `Gltf_v2.0-glb`) → format. */
function formatFromFogKey(key: string): ModelFormat | null {
  const k = key.toLowerCase();
  if (k.startsWith('gltf')) return 'gltf';
  if (k.startsWith('stl')) return 'stl';
  if (k.startsWith('cityjson')) return 'cityjson';
  if (k.startsWith('citygml')) return 'citygml';
  if (k.startsWith('ifc')) return 'ifc';
  return null;
}

export interface ModelRef {
  url: string;
  format: ModelFormat;
  /** Source up-axis from the element's `ots:modelUpAxis` annotation ('Z' rotates into Y-up scenes). */
  upAxis?: string | null;
}

/** Preference when an element offers several formats. */
const FORMAT_ORDER: ModelFormat[] = ['gltf', 'cityjson', 'citygml', 'stl', 'ifc'];

/**
 * The best loadable 3D-model reference of a viewer-feed element: the explicit
 * glTF URL first, then the FOG file list — by FOG format key or URL extension —
 * preferring glTF > CityJSON > CityGML > STL.
 */
export function modelRefOf(el: {
  gltf_url?: string | null;
  ifc_url?: string | null;
  files?: [string, string][];
  up_axis?: string | null;
}): ModelRef | null {
  return modelRefsOf(el)[0] ?? null;
}

/**
 * Every loadable 3D-model reference of an element, one per format, ordered by
 * preference — lets viewers offer a format picker when an element links
 * several representations (e.g. a glTF *and* the source IFC).
 */
export function modelRefsOf(el: {
  gltf_url?: string | null;
  ifc_url?: string | null;
  files?: [string, string][];
  up_axis?: string | null;
}): ModelRef[] {
  const found = new Map<ModelFormat, string>();
  if (el.gltf_url) found.set('gltf', el.gltf_url);
  for (const [key, url] of el.files || []) {
    const format = formatFromFogKey(key) ?? modelFormatFromUrl(url);
    if (format && !found.has(format)) found.set(format, url);
  }
  // The feed's dedicated ifc_url (possibly carrying a `#GlobalId` fragment that
  // isolates this element in the model) backs the FOG list up.
  if (el.ifc_url && !found.has('ifc')) found.set('ifc', el.ifc_url);
  const out: ModelRef[] = [];
  for (const format of FORMAT_ORDER) {
    const url = found.get(format);
    if (url) out.push({ url, format, upAxis: el.up_axis ?? null });
  }
  return out;
}

/** Human-readable display name per model format (file chips, BIM lists). */
export const FORMAT_LABELS: Record<ModelFormat, string> = {
  gltf: 'glTF',
  stl: 'STL',
  cityjson: 'CityJSON',
  citygml: 'CityGML',
  ifc: 'IFC',
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
