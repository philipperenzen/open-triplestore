// Result-shape inference for Spark's visual result widgets.
//
// A query result in the chat (a retrieval-trail run, a Run-block result, or the
// rows a ```view block binds to) arrives as a plain table: column names plus
// string cells. This module looks at the SHAPE of those cells and says which
// visuals the rows can honestly support — a bar chart needs a label and a
// number, a line chart a date and a number, a map a WKT geometry, a node-link
// graph subject/predicate/object triples, a SHACL summary validation results —
// and builds each visual's input from the rows alone.
//
// Grounding is the point: every builder reads only the cells it was given and
// never fills gaps, so a picture can never show something the query did not
// return. When nothing fits, the answer is the table.
//
// Pure (no DOM, no Leaflet/cytoscape imports) so it unit-tests in jsdom and
// adds nothing heavy to the chat bundle.

import { parseWktGeometry } from './ontology/valueType';
import { graphResultsToElements } from './rdf-utils';
import { resultsToViewerElements, type SelectResults } from './viewer/results';
import type { ViewerElement } from './viewer/geometry';
import type { ValidationReport } from './validationReport';

/** A visual the chat can draw for a result. `table` always applies. */
export type VizKind = 'table' | 'bar' | 'line' | 'stat' | 'map' | 'graph' | 'shacl';

/** Preference order when picking the default visual: most specific first. */
export const VIZ_ORDER: VizKind[] = ['shacl', 'graph', 'map', 'line', 'bar', 'stat', 'table'];

/** A query result as the chat carries it: column names plus string cells. */
export interface VizTable {
  columns: string[];
  rows: string[][];
}

/** Optional column hints (the model's ```view "x"/"y"), matched by name. */
export interface VizHints {
  x?: string;
  y?: string;
}

/** Hard caps so a large result stays a readable picture. */
export const MAX_BARS = 15;
export const MAX_LINE_POINTS = 100;
export const MAX_LINE_SERIES = 6;
export const MAX_STATS = 6;
export const MAX_GRAPH_NODES = 150;
export const MAX_SHACL_GROUPS = 5;
export const MAX_SHACL_SAMPLES = 5;

const RDF_TYPE = 'http://www.w3.org/1999/02/22-rdf-syntax-ns#type';
const SH = 'http://www.w3.org/ns/shacl#';
const GEO_WKT = 'http://www.opengis.net/ont/geosparql#wktLiteral';

// ── Cell classification ──────────────────────────────────────────────────────

/** `<http://…>` → `http://…`; anything else unchanged. */
export function unwrapIri(cell: string): string {
  const s = String(cell ?? '').trim();
  return /^<[^<>\s]+>$/.test(s) ? s.slice(1, -1) : s;
}

function isIri(cell: string): boolean {
  return /^(https?:\/\/|urn:)\S+$/i.test(unwrapIri(cell));
}

function isBnode(cell: string): boolean {
  return /^_:\S+$/.test(String(cell ?? '').trim());
}

function isNumber(cell: string): boolean {
  const s = String(cell ?? '').trim();
  return s !== '' && Number.isFinite(Number(s));
}

/** Column names that make a bare 4-digit number a year rather than a count. */
const TEMPORAL_NAME = /year|jaar|date|datum|time|tijd|period|month|maand|day|dag|when|created|modified|issued/i;
const ISO_DATE = /^-?\d{4}-\d{2}(-\d{2}([T ]\d{2}:\d{2}(:\d{2}(\.\d+)?)?(Z|[+-]\d{2}:?\d{2})?)?)?$/;

/**
 * A cell's point in time (ms since epoch) when it is an ISO date/dateTime,
 * a gYearMonth, or — in a date-ish column — a bare year. Otherwise null.
 */
export function temporalValue(cell: string, columnName = ''): number | null {
  const s = String(cell ?? '').trim();
  if (!s) return null;
  if (/^-?\d{4}$/.test(s)) {
    if (!TEMPORAL_NAME.test(columnName)) return null;
    return Date.UTC(Number(s), 0, 1);
  }
  if (!ISO_DATE.test(s)) return null;
  const ym = /^(-?\d{4})-(\d{2})$/.exec(s);
  if (ym) return Date.UTC(Number(ym[1]), Number(ym[2]) - 1, 1);
  const t = Date.parse(s.replace(' ', 'T'));
  return Number.isFinite(t) ? t : null;
}

/** Strip an optional `<crs-iri>` prefix, then parse; null when not WKT. */
function wktOf(cell: string): string | null {
  const s = String(cell ?? '').trim();
  if (!s || s.length > 200_000) return null;
  const bare = s.replace(/^<[^>]+>\s*/, '');
  return parseWktGeometry(bare) ? s : null;
}

interface ColumnProfile {
  name: string;
  filled: number;
  numeric: boolean;
  temporal: boolean;
  wkt: boolean;
}

function profileColumns(table: VizTable): ColumnProfile[] {
  return table.columns.map((name, i) => {
    let filled = 0;
    let numeric = 0;
    let temporal = 0;
    let wkt = 0;
    for (const row of table.rows) {
      const v = String(row[i] ?? '').trim();
      if (!v) continue;
      filled += 1;
      if (isNumber(v)) numeric += 1;
      if (temporalValue(v, name) !== null) temporal += 1;
      if (wktOf(v)) wkt += 1;
    }
    return {
      name,
      filled,
      numeric: filled > 0 && numeric === filled,
      temporal: filled > 0 && temporal === filled,
      wkt: wkt > 0,
    };
  });
}

function colIndex(columns: string[], name?: string): number {
  if (!name) return -1;
  const want = String(name).replace(/^\?/, '').toLowerCase();
  return columns.findIndex((c) => String(c).replace(/^\?/, '').toLowerCase() === want);
}

// ── Shape inference ──────────────────────────────────────────────────────────

const TRIPLE_COLS = [
  ['s', 'p', 'o'],
  ['subject', 'predicate', 'object'],
];

/** Index of the s/p/o columns when the table is a triple list, else null. */
function tripleColumns(table: VizTable): [number, number, number] | null {
  const lower = table.columns.map((c) => String(c).replace(/^\?/, '').toLowerCase());
  for (const names of TRIPLE_COLS) {
    const idx = names.map((n) => lower.indexOf(n));
    if (idx.every((i) => i >= 0)) return idx as [number, number, number];
  }
  return null;
}

/** True when the table is a list of triples whose predicates are all IRIs. */
export function isTripleTable(table: VizTable): boolean {
  const cols = tripleColumns(table);
  if (!cols || !table.rows.length) return false;
  return table.rows.every((r) => isIri(String(r[cols[1]] ?? '')));
}

interface ChartColumns {
  x: number;
  y: number;
  /** Optional series-splitting column (line charts only). */
  series: number;
}

/** Pick the label/value columns for a bar chart, or null. */
function barColumns(table: VizTable, prof: ColumnProfile[], hints: VizHints = {}): ChartColumns | null {
  if (table.rows.length < 2) return null;
  let y = colIndex(table.columns, hints.y);
  if (y < 0 || !prof[y]?.numeric) {
    // Rightmost numeric column: `SELECT ?class (COUNT(*) AS ?n)` puts the
    // aggregate last.
    y = -1;
    for (let i = prof.length - 1; i >= 0; i--) {
      if (prof[i].numeric) { y = i; break; }
    }
  }
  if (y < 0) return null;
  let x = colIndex(table.columns, hints.x);
  if (x < 0 || x === y) x = prof.findIndex((p, i) => i !== y && !p.numeric && !p.wkt && p.filled > 0);
  if (x < 0) x = prof.findIndex((p, i) => i !== y && p.filled > 0);
  return x < 0 ? null : { x, y, series: -1 };
}

/** Pick the time/value (and optional series) columns for a line chart, or null. */
function lineColumns(table: VizTable, prof: ColumnProfile[], hints: VizHints = {}): ChartColumns | null {
  if (table.rows.length < 2) return null;
  let x = colIndex(table.columns, hints.x);
  if (x < 0 || !prof[x]?.temporal) x = prof.findIndex((p) => p.temporal);
  if (x < 0) return null;
  let y = colIndex(table.columns, hints.y);
  if (y < 0 || y === x || !prof[y]?.numeric) {
    y = -1;
    for (let i = prof.length - 1; i >= 0; i--) {
      if (i !== x && prof[i].numeric) { y = i; break; }
    }
  }
  if (y < 0) return null;
  // A remaining categorical column with a handful of values splits the line
  // into series (`?year ?type (COUNT(*) AS ?n)`).
  let series = -1;
  for (let i = 0; i < prof.length; i++) {
    if (i === x || i === y || prof[i].numeric || prof[i].wkt) continue;
    const distinct = new Set(table.rows.map((r) => String(r[i] ?? ''))).size;
    if (distinct >= 2 && distinct <= MAX_LINE_SERIES) { series = i; break; }
  }
  return { x, y, series };
}

/** Cells shown as KPI tiles: only a single-row result with a number in it. */
function statColumns(table: VizTable, prof: ColumnProfile[]): number[] {
  if (table.rows.length !== 1 || !prof.some((p) => p.numeric)) return [];
  return prof
    .map((p, i) => (p.filled > 0 && !p.wkt ? i : -1))
    .filter((i) => i >= 0)
    .slice(0, MAX_STATS);
}

/**
 * The visuals a result can support, in preference order, plus the default.
 * `table` is always present and is the default when nothing else fits.
 */
export function inferViews(table: VizTable | null | undefined, hints: VizHints = {}): { views: VizKind[]; best: VizKind } {
  if (!table || !table.columns?.length || !table.rows?.length) return { views: ['table'], best: 'table' };
  const prof = profileColumns(table);
  const found = new Set<VizKind>(['table']);
  if (shaclFromTable(table)) found.add('shacl');
  if (isTripleTable(table)) found.add('graph');
  if ((prof.some((p) => p.wkt) || latLonColumns(table)) && mapElementsFromTable(table).length) found.add('map');
  if (lineColumns(table, prof, hints)) found.add('line');
  if (barColumns(table, prof, hints)) found.add('bar');
  if (statColumns(table, prof).length) found.add('stat');
  const views = VIZ_ORDER.filter((v) => found.has(v));
  return { views, best: views[0] };
}

/**
 * Resolve the view to show: the requested one when the rows support it,
 * otherwise the best fit (`auto`/missing) — or the table when a specific
 * request does not fit, so a wrong guess never becomes a misleading picture.
 */
export function resolveView(
  table: VizTable | null | undefined,
  requested: string | null | undefined,
  hints: VizHints = {},
): { view: VizKind; views: VizKind[]; fellBack: VizKind | null } {
  const { views, best } = inferViews(table, hints);
  const want = String(requested ?? 'auto').toLowerCase();
  const alias: Record<string, VizKind> = { chart: best === 'line' ? 'line' : 'bar', report: 'shacl', kpi: 'stat', network: 'graph' };
  const kind = (alias[want] ?? want) as VizKind;
  if (!requested || want === 'auto' || !VIZ_ORDER.includes(kind)) return { view: best, views, fellBack: null };
  if (views.includes(kind)) return { view: kind, views, fellBack: null };
  return { view: 'table', views, fellBack: kind };
}

// ── Builders ─────────────────────────────────────────────────────────────────

export interface ChartPoint { label: string; value: number }
export interface ChartSpec {
  type: 'bar' | 'line';
  title: string;
  xLabel: string;
  yLabel: string;
  /** Explicit category order (chronological for line charts). */
  labels?: string[];
  series: Array<{ name: string; data: ChartPoint[] }>;
}

/** Chart spec + how many rows it shows of how many the result had. */
export interface BuiltChart { spec: ChartSpec; shown: number; total: number }

/**
 * Build a bar (top-N by value) or line (chronological) chart from the rows.
 * Rows whose value cell is not a number are skipped, never imputed.
 */
export function buildChart(table: VizTable, type: 'bar' | 'line', hints: VizHints & { title?: string } = {}): BuiltChart | null {
  const prof = profileColumns(table);
  const cols = type === 'line' ? lineColumns(table, prof, hints) : barColumns(table, prof, hints);
  if (!cols) return null;
  const { x, y, series } = cols;
  const points = table.rows
    .map((r) => ({ label: String(r[x] ?? '').trim(), value: Number(String(r[y] ?? '').trim()), group: series >= 0 ? String(r[series] ?? '') : '', row: r }))
    .filter((p) => p.label && String(p.row[y] ?? '').trim() !== '' && Number.isFinite(p.value));
  if (!points.length) return null;
  const base = { title: hints.title ?? '', xLabel: table.columns[x], yLabel: table.columns[y] };
  if (type === 'bar') {
    const data = points
      .map(({ label, value }) => ({ label, value }))
      .sort((a, b) => b.value - a.value)
      .slice(0, MAX_BARS);
    return { spec: { type, ...base, series: [{ name: '', data }] }, shown: data.length, total: points.length };
  }
  const timed = points
    .map((p) => ({ ...p, t: temporalValue(p.label, table.columns[x]) as number }))
    .sort((a, b) => a.t - b.t)
    .slice(0, MAX_LINE_POINTS);
  const labels = [...new Set(timed.map((p) => p.label))];
  const groups = new Map<string, ChartPoint[]>();
  for (const p of timed) {
    if (!groups.has(p.group)) groups.set(p.group, []);
    groups.get(p.group)!.push({ label: p.label, value: p.value });
  }
  return {
    spec: { type, ...base, labels, series: [...groups].map(([name, data]) => ({ name, data })) },
    shown: timed.length,
    total: points.length,
  };
}

export interface StatTile { label: string; value: string; numeric: boolean }

/** KPI tiles for a single-row aggregate result — the cells exactly as returned. */
export function buildStats(table: VizTable): StatTile[] {
  const prof = profileColumns(table);
  return statColumns(table, prof).map((i) => ({
    label: table.columns[i],
    value: String(table.rows[0][i] ?? ''),
    numeric: prof[i].numeric,
  }));
}

const LAT = /(?:^|[_\s-])(lat|latitude|breedte)(?:$|[_\s-])/i;
const LON = /(?:^|[_\s-])(lon|lng|long|longitude|lengte)(?:$|[_\s-])/i;

function latLonColumns(table: VizTable): [number, number] | null {
  const lat = table.columns.findIndex((c) => LAT.test(String(c).replace(/^\?/, '')));
  const lon = table.columns.findIndex((c) => LON.test(String(c).replace(/^\?/, '')));
  return lat >= 0 && lon >= 0 && lat !== lon ? [lat, lon] : null;
}

/** Turn a string cell into a SPARQL-JSON term (WKT gets its datatype back). */
function cellTerm(cell: string): { type: string; value: string; datatype?: string } {
  const s = String(cell ?? '').trim();
  if (isIri(s)) return { type: 'uri', value: unwrapIri(s) };
  if (isBnode(s)) return { type: 'bnode', value: s.slice(2) };
  if (wktOf(s)) return { type: 'literal', value: s, datatype: GEO_WKT };
  return { type: 'literal', value: s };
}

/**
 * Map elements (WGS84 only — projected CRSs are skipped, never reprojected)
 * from WKT cells, or from a lat/lon column pair when there is no WKT.
 */
export function mapElementsFromTable(table: VizTable): ViewerElement[] {
  const latLon = latLonColumns(table);
  const results: SelectResults = {
    head: { vars: latLon ? [...table.columns, '__point'] : table.columns },
    results: {
      bindings: table.rows.map((r) => {
        const b: Record<string, ReturnType<typeof cellTerm>> = {};
        table.columns.forEach((c, i) => {
          if (String(r[i] ?? '').trim()) b[c] = cellTerm(String(r[i]));
        });
        // An empty cell is no coordinate — Number('') would put it at 0.
        if (latLon && isNumber(String(r[latLon[0]] ?? '')) && isNumber(String(r[latLon[1]] ?? ''))) {
          const lat = Number(r[latLon[0]]);
          const lon = Number(r[latLon[1]]);
          if (Math.abs(lat) <= 90 && Math.abs(lon) <= 180) {
            b.__point = { type: 'literal', value: `POINT(${lon} ${lat})`, datatype: GEO_WKT };
          }
        }
        return b;
      }),
    },
  };
  return resultsToViewerElements(results);
}

type Term = { type: string; value: string; datatype?: string; language?: string };

/** Node-link elements from SPARQL-JSON s/p/o bindings (typed terms). */
export function graphFromBindings(bindings: Array<Record<string, Term>>) {
  return graphResultsToElements(bindings, 's', 'p', 'o', MAX_GRAPH_NODES);
}

/** Node-link elements from a triple table (CONSTRUCT/DESCRIBE trail rows). */
export function graphFromTable(table: VizTable) {
  const cols = tripleColumns(table);
  if (!cols) return { nodes: [], edges: [] };
  const bindings = table.rows.map((r) => ({
    s: cellTerm(String(r[cols[0]] ?? '')),
    p: { type: 'uri', value: unwrapIri(String(r[cols[1]] ?? '')) },
    o: cellTerm(String(r[cols[2]] ?? '')),
  }));
  return graphFromBindings(bindings);
}

/** SPARQL-JSON s/p/o bindings → the chat's string table shape. */
export function tableFromTripleBindings(bindings: Array<Record<string, Term>>): VizTable {
  const cell = (t?: Term) => (!t ? '' : t.type === 'bnode' ? `_:${t.value}` : String(t.value ?? ''));
  return {
    columns: ['subject', 'predicate', 'object'],
    rows: bindings.map((b) => [cell(b.s), cell(b.p), cell(b.o)]),
  };
}

/** SPARQL-JSON SELECT bindings → the chat's string table shape. */
export function tableFromBindings(vars: string[], bindings: Array<Record<string, Term>>): VizTable {
  return {
    columns: [...vars],
    rows: bindings.map((b) =>
      vars.map((v) => {
        const t = b?.[v];
        if (!t || typeof t.value !== 'string') return '';
        return t.type === 'bnode' ? `_:${t.value}` : t.value;
      }),
    ),
  };
}

// ── SHACL ────────────────────────────────────────────────────────────────────

export interface ShaclIssue {
  focus: string;
  path: string;
  severity: string;
  message: string;
  shape: string;
}

export interface ShaclFindings { conforms: boolean | null; issues: ShaclIssue[] }

/** `sh:Violation` / `http://www.w3.org/ns/shacl#Violation` / `Violation` → `Violation`. */
export function severityName(s: string): string {
  const v = unwrapIri(String(s ?? '')).trim();
  if (!v) return '';
  return v.split(/[#/:]/).filter(Boolean).pop() || v;
}

function localSh(pred: string): string | null {
  const p = unwrapIri(pred);
  if (p.startsWith(SH)) return p.slice(SH.length);
  if (p.startsWith('sh:')) return p.slice(3);
  return null;
}

/**
 * Validation findings carried by a result table: either a SHACL report as
 * triples (CONSTRUCT/DESCRIBE over a report graph) or a SELECT whose columns
 * name the result's focus node and severity/message. Null when neither.
 */
export function shaclFromTable(table: VizTable | null | undefined): ShaclFindings | null {
  if (!table?.rows?.length) return null;
  const cols = tripleColumns(table);
  if (cols) {
    const bySubject = new Map<string, Partial<ShaclIssue> & { isResult?: boolean }>();
    let conforms: boolean | null = null;
    let sawReport = false;
    for (const r of table.rows) {
      const s = String(r[cols[0]] ?? '');
      const p = localSh(String(r[cols[1]] ?? ''));
      const o = String(r[cols[2]] ?? '');
      const pred = unwrapIri(String(r[cols[1]] ?? ''));
      if (pred === RDF_TYPE) {
        const cls = localSh(o);
        if (cls === 'ValidationReport') sawReport = true;
        if (cls === 'ValidationResult') bySubject.set(s, { ...(bySubject.get(s) || {}), isResult: true });
        continue;
      }
      if (!p) continue;
      if (p === 'conforms') {
        sawReport = true;
        conforms = /^(true|1)$/i.test(o.trim());
        continue;
      }
      const field = { focusNode: 'focus', resultPath: 'path', resultSeverity: 'severity', resultMessage: 'message', sourceShape: 'shape' }[p] as keyof ShaclIssue | undefined;
      if (!field) continue;
      const cur = bySubject.get(s) || {};
      // A result can carry several messages (one per language); keep the first.
      if (!cur[field]) cur[field] = field === 'severity' ? severityName(o) : unwrapIri(o);
      cur.isResult = true;
      bySubject.set(s, cur);
    }
    const issues = [...bySubject.values()]
      .filter((i) => i.isResult && (i.focus || i.severity))
      .map((i) => ({ focus: i.focus || '', path: i.path || '', severity: i.severity || '', message: i.message || '', shape: i.shape || '' }));
    if (!issues.length && !sawReport) return null;
    return { conforms: conforms ?? (sawReport ? issues.length === 0 : null), issues };
  }
  const find = (re: RegExp) => table.columns.findIndex((c) => re.test(String(c)));
  const focus = find(/focus/i);
  const severity = find(/severity/i);
  const message = find(/message|msg/i);
  if (focus < 0 || (severity < 0 && message < 0)) return null;
  const path = find(/path/i);
  const shape = find(/shape/i);
  const at = (r: string[], i: number) => (i >= 0 ? unwrapIri(String(r[i] ?? '')) : '');
  const issues = table.rows.map((r) => ({
    focus: at(r, focus),
    path: at(r, path),
    severity: severity >= 0 ? severityName(String(r[severity] ?? '')) : '',
    message: at(r, message),
    shape: at(r, shape),
  }));
  return { conforms: null, issues };
}

/** Findings from the platform's JSON validation report (validate / latest run). */
export function shaclFromReport(report: ValidationReport | null | undefined): ShaclFindings | null {
  if (!report) return null;
  const issues = (report.results || []).map((r) => ({
    focus: String(r.focus_node ?? ''),
    path: String(r.path ?? ''),
    severity: severityName(String(r.severity ?? '')),
    message: String(r.message ?? ''),
    shape: String((r as Record<string, unknown>).source_shape ?? (r as Record<string, unknown>).shape ?? ''),
  }));
  return { conforms: !!report.conforms, issues };
}

export interface ShaclSummary {
  conforms: boolean | null;
  total: number;
  bySeverity: Array<{ key: string; count: number }>;
  byPath: Array<{ key: string; count: number }>;
  byShape: Array<{ key: string; count: number }>;
  byFocus: Array<{ key: string; count: number }>;
  samples: ShaclIssue[];
}

const SEVERITY_RANK: Record<string, number> = { Violation: 0, Warning: 1, Info: 2 };

function countBy(issues: ShaclIssue[], key: keyof ShaclIssue, limit = MAX_SHACL_GROUPS) {
  const m = new Map<string, number>();
  for (const i of issues) if (i[key]) m.set(i[key], (m.get(i[key]) || 0) + 1);
  return [...m].map(([k, count]) => ({ key: k, count })).sort((a, b) => b.count - a.count || a.key.localeCompare(b.key)).slice(0, limit);
}

/** Counts by severity / path / shape / focus node plus the worst few results. */
export function summarizeShacl(findings: ShaclFindings): ShaclSummary {
  const issues = findings.issues;
  const bySeverity = countBy(issues, 'severity', 10).sort(
    (a, b) => (SEVERITY_RANK[a.key] ?? 9) - (SEVERITY_RANK[b.key] ?? 9) || b.count - a.count,
  );
  const samples = [...issues]
    .sort((a, b) => (SEVERITY_RANK[a.severity] ?? 9) - (SEVERITY_RANK[b.severity] ?? 9))
    .slice(0, MAX_SHACL_SAMPLES);
  return {
    conforms: findings.conforms,
    total: issues.length,
    bySeverity,
    byPath: countBy(issues, 'path'),
    byShape: countBy(issues, 'shape'),
    byFocus: countBy(issues, 'focus'),
    samples,
  };
}
