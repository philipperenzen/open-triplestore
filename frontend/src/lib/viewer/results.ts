// Map-view gating + conversion for the table/graph result explorers (SPARQL
// editor + Triple Browser). A SELECT result set (or a list of triples adapted
// to the same shape) is scanned for mappable geometry; when present, the
// explorer can offer a Map tab that plots the rows on the shared ViewerMap.
//
// Kept Leaflet/three-free (only valueType + crs helpers) so it unit-tests in
// jsdom and adds nothing to the main bundle until the Map tab is opened.
import type { ViewerElement } from './geometry';
import { epsgFromReference } from './crs';
import { parseWktGeometry } from '../ontology/valueType';

// SPARQL-results-JSON binding shape (the same one rdf-utils uses everywhere).
interface RdfTerm {
  type: string;
  value?: string;
  datatype?: string;
  language?: string;
  'xml:lang'?: string;
}
type Binding = Record<string, RdfTerm>;
export interface SelectResults {
  head?: { vars?: string[] };
  results?: { bindings?: Binding[] };
}

const GEO = 'http://www.opengis.net/ont/geosparql#';

/** A GeoSPARQL WKT literal datatype (`…#wktLiteral`). */
function isWktTerm(term: RdfTerm | undefined | null): boolean {
  return !!term && term.type === 'literal' && typeof term.datatype === 'string' && term.datatype.endsWith('wktLiteral');
}

/** Column names that plausibly carry a latitude / longitude number. */
const LATLON_COL = /(?:^|[_\s])(latitude|longitude|lat|long|lng|lon)(?:$|[_\s])/i;

function isNumericTerm(term: RdfTerm | undefined | null): boolean {
  if (!term || term.type !== 'literal') return false;
  const v = (term.value ?? '').trim();
  if (!v) return false;
  return Number.isFinite(Number(v));
}

/**
 * True when a SELECT result set contains mappable geometry: any binding is a
 * `geo:wktLiteral`, or there is at least one plausible lat AND one plausible
 * lon numeric column. Conservative by design — without geometry the Map tab
 * stays hidden, which is exactly the requested scope-aware gating.
 */
export function detectGeoBindings(results: SelectResults | null | undefined): boolean {
  const bindings = results?.results?.bindings;
  if (!Array.isArray(bindings) || bindings.length === 0) return false;

  // Any WKT literal anywhere in the rows → mappable.
  for (const row of bindings) {
    if (!row) continue;
    for (const k of Object.keys(row)) {
      if (isWktTerm(row[k])) return true;
    }
  }

  // Otherwise: paired lat/lon numeric columns (by variable name).
  const vars = results?.head?.vars ?? Object.keys(bindings[0] ?? {});
  let hasLat = false;
  let hasLon = false;
  for (const v of vars) {
    const m = LATLON_COL.exec(v);
    if (!m) continue;
    const which = m[1].toLowerCase();
    const isLat = which === 'lat' || which === 'latitude';
    // Confirm at least one row carries a numeric value for this column.
    const numeric = bindings.some((row) => isNumericTerm(row?.[v]));
    if (!numeric) continue;
    if (isLat) hasLat = true;
    else hasLon = true;
  }
  return hasLat && hasLon;
}

/** A row's WKT literal kept only when it is WGS84/CRS84/unprefixed. */
function wgs84Wkt(literal: string | undefined | null): string | null {
  if (!literal) return null;
  const m = literal.match(/^\s*<([^>]+)>\s*/);
  if (m) {
    const epsg = epsgFromReference(m[1]);
    // A recognised projected CRS that isn't WGS84 (4326) / WGS84-3D (4979) is
    // skipped — we do NOT reproject client-side here, only pass WGS84 through.
    // An unrecognised CRS ref (epsg === null) is treated as plottable-as-is,
    // mirroring the feed's permissive fallback.
    if (epsg !== null && epsg !== 4326 && epsg !== 4979) return null;
  }
  // Strip the optional CRS prefix the same way parseWktGeometry does, then
  // require it to parse as a geometry before keeping it.
  const stripped = (m ? literal.slice(m[0].length) : literal).trim();
  return parseWktGeometry(stripped) ? stripped : null;
}

/** A row's IRI/bnode value usable as the element id (first IRI, else first bnode). */
function rowId(row: Binding, vars: string[]): string | null {
  for (const v of vars) {
    const t = row[v];
    if (t && (t.type === 'uri' || t.type === 'iri') && t.value) return t.value;
  }
  for (const v of vars) {
    const t = row[v];
    if (t && t.type === 'bnode' && t.value) return `_:${t.value}`;
  }
  return null;
}

/** Local name of an IRI (segment after the last `/` or `#`). */
function localName(iri: string): string {
  return iri.split(/[/#]/).filter(Boolean).pop() || iri;
}

/** A human-ish label for a row: an rdfs:label-ish string literal, else the id's local name. */
function rowLabel(row: Binding, vars: string[], id: string): string {
  // Prefer a plain/lang string literal in a label-ish column.
  for (const v of vars) {
    const t = row[v];
    if (!t || t.type !== 'literal' || !t.value) continue;
    if (/label|name|title|prefLabel/i.test(v) && !isWktTerm(t)) return t.value;
  }
  return localName(id);
}

/**
 * Convert SELECT result rows into ViewerElements for the map: every row that
 * carries a WGS84 WKT literal becomes an element keyed on a row IRI (or a
 * synthetic id), labelled from a label-ish column or the IRI's local name.
 * Rows without plottable WGS84 geometry are dropped.
 */
export function resultsToViewerElements(results: SelectResults | null | undefined): ViewerElement[] {
  const bindings = results?.results?.bindings;
  if (!Array.isArray(bindings)) return [];
  const vars = results?.head?.vars ?? Object.keys(bindings[0] ?? {});
  const out: ViewerElement[] = [];
  let synthetic = 0;
  for (const row of bindings) {
    if (!row) continue;
    // The first WKT literal in the row defines this element's geometry.
    let wkt: string | null = null;
    for (const v of vars) {
      if (isWktTerm(row[v])) {
        const kept = wgs84Wkt(row[v]?.value);
        if (kept) { wkt = kept; break; }
      }
    }
    if (!wkt) continue;
    const id = rowId(row, vars) ?? `row:${synthetic++}`;
    out.push({ id, label: rowLabel(row, vars, id), wkt4326: wkt });
  }
  return out;
}

/**
 * Adapt a Triple Browser triple list (`[{subject,predicate,object,graph}]`) to
 * the SELECT-results shape so the same detection/conversion path applies. Each
 * triple becomes one row with `subject`/`predicate`/`object` bindings.
 */
export function triplesToResults(
  triples: Array<{ subject?: RdfTerm; predicate?: RdfTerm; object?: RdfTerm }> | null | undefined
): SelectResults {
  const bindings: Binding[] = [];
  for (const t of triples ?? []) {
    if (!t?.subject || !t?.predicate || !t?.object) continue;
    bindings.push({ subject: t.subject, predicate: t.predicate, object: t.object });
  }
  return { head: { vars: ['subject', 'predicate', 'object'] }, results: { bindings } };
}
