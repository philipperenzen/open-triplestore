// Spark chat answers are markdown plus a small set of fenced "widget" blocks the
// model may emit (see CHAT_SYSTEM_PROMPT in src/server/llm_sparql.rs):
//
//   ```sparql  → runnable query card            ```chart   → JSON chart spec
//   ```api     → runnable GET /api/... call     ```map     → JSON WKT feature map
//   ```csv     → table preview with download    ```card    → entity info card
//   ```file    → downloadable file card         ```model3d → 3D model orbit viewer
//
// A ```map spec may also carry "models": URL-referenced 3D models anchored at a
// WKT POINT, rendered on the georeferenced MapLibre viewer instead of Leaflet.
//
// parseChatBlocks() splits a message into ordered segments: plain markdown runs
// (rendered by the caller through renderMarkdown) interleaved with typed widget
// segments. Unknown / malformed blocks fall back into the markdown flow (or a
// 'broken' segment with the raw text), so a model that gets a spec wrong still
// produces a readable answer. Everything here is pure parsing — no DOM except
// decorateApiLinks(), which post-processes already-sanitized HTML, and the
// safeUrl scheme gate (which resolves model/file URLs against window.location).

import { modelFormatFromUrl } from './viewer/detect';
import { parseWktGeometry } from './ontology/valueType';
import { safeExternalUrl } from './safeUrl';

/** A fence opener/closer: ``` or ~~~, optionally with a language tag. */
const FENCE_RE = /^\s*(```+|~~~+)\s*([\w+-]*)\s*$/;

/** Hard caps so a hostile/confused answer cannot freeze the tab. */
const MAX_CHART_POINTS = 100;
const MAX_MAP_FEATURES = 200;
const MAX_CARD_FACTS = 24;
const MAX_CSV_ROWS = 500;
const MAX_3D_MODELS = 8;

/**
 * Split an assistant message into renderable segments.
 * @param {string} source - the raw markdown answer
 * @returns {Array<object>} segments: {kind:'md', source} | {kind:'sparql', code}
 *   | {kind:'api', method, path} | {kind:'chart', spec} | {kind:'map', features, models}
 *   | {kind:'card', card} | {kind:'csv', columns, rows, raw}
 *   | {kind:'model3d', models} | {kind:'file', file}
 *   | {kind:'broken', label, error, raw}
 */
export function parseChatBlocks(source, opts = {}) {
  const queryRows = opts.queryRows ?? null;
  const lines = String(source ?? '').split('\n');
  const segments = [];
  let md = [];
  const flushMd = () => {
    const s = md.join('\n');
    if (s.trim()) segments.push({ kind: 'md', source: s });
    md = [];
  };

  for (let i = 0; i < lines.length; i++) {
    const open = FENCE_RE.exec(lines[i]);
    if (!open) {
      md.push(lines[i]);
      continue;
    }
    const marker = open[1];
    const lang = (open[2] || '').toLowerCase();
    // Find the matching closing fence (same character, at least as long).
    let close = -1;
    for (let j = i + 1; j < lines.length; j++) {
      const m = FENCE_RE.exec(lines[j]);
      if (m && m[1][0] === marker[0] && m[1].length >= marker.length && !m[2]) {
        close = j;
        break;
      }
    }
    const body = lines.slice(i + 1, close === -1 ? lines.length : close).join('\n');
    const seg = specialSegment(lang, body, queryRows);
    if (seg) {
      flushMd();
      segments.push(seg);
    } else {
      // Not one of ours — keep the fence verbatim in the markdown flow, where
      // renderMarkdown() will syntax-highlight known languages.
      md.push(...lines.slice(i, close === -1 ? lines.length : close + 1));
    }
    i = close === -1 ? lines.length : close;
  }
  flushMd();
  return segments;
}

/** Map one fenced block to a widget segment, or null to leave it as markdown. */
function specialSegment(lang, body, queryRows = null) {
  const code = body.trim();
  if (!code) return null;
  const kind = lang || sniffLang(code);
  switch (kind) {
    case 'sparql':
    case 'rq':
      return { kind: 'sparql', code };
    case 'api':
    case 'http':
    case 'endpoint': {
      const ep = parseApiEndpoint(code.split('\n')[0]);
      return ep ? { kind: 'api', ...ep } : null;
    }
    case 'chart': {
      const r = parseChartSpec(code, queryRows);
      return r.error
        ? { kind: 'broken', label: 'chart', error: r.error, raw: code }
        : { kind: 'chart', spec: r.spec };
    }
    case 'map':
    case 'geo': {
      const r = parseMapSpec(code, queryRows);
      return r.error
        ? { kind: 'broken', label: 'map', error: r.error, raw: code }
        : { kind: 'map', features: r.features, models: r.models };
    }
    // model3d / file fire on the explicit fence tag only — sniffLang never
    // returns them, so untagged JSON stays in the markdown flow.
    case 'model3d': {
      const r = parseModel3dSpec(code);
      return r.error
        ? { kind: 'broken', label: 'model3d', error: r.error, raw: code }
        : { kind: 'model3d', models: r.models };
    }
    case 'file': {
      const r = parseFileSpec(code);
      return r.error
        ? { kind: 'broken', label: 'file', error: r.error, raw: code }
        : { kind: 'file', file: r.file };
    }
    case 'card':
    case 'infocard':
    case 'info-card': {
      const r = parseInfoCard(code);
      return r.error
        ? { kind: 'broken', label: 'card', error: r.error, raw: code }
        : { kind: 'card', card: r.card };
    }
    case 'csv': {
      const t = parseCsv(code);
      return t.columns.length ? { kind: 'csv', ...t, raw: code } : null;
    }
    default:
      return null;
  }
}

/** Best-effort language sniff for fences the model forgot to tag. */
export function sniffLang(code) {
  const t = String(code ?? '').trim();
  if (!t) return '';
  // A one-or-two-line `GET /api/...` is an API call, not a code sample.
  if (t.split('\n').length <= 2 && parseApiEndpoint(t.split('\n')[0])) return 'api';
  if (/^\s*@prefix/im.test(t)) return ''; // Turtle — leave to the highlighter
  if (
    /\b(SELECT|ASK|CONSTRUCT|DESCRIBE)\b/i.test(t) &&
    /[{}]/.test(t) &&
    /^\s*(PREFIX|BASE|SELECT|ASK|CONSTRUCT|DESCRIBE|#)/im.test(t)
  ) {
    return 'sparql';
  }
  return '';
}

/**
 * Parse a runnable API endpoint from a line like `GET /api/.../run?x=1`, a bare
 * `/api/...` path, or a full same-app URL. Only same-origin `/api/` GETs are
 * runnable from chat — reads under the caller's own session, never mutations.
 * @returns {{method: 'GET', path: string} | null}
 */
export function parseApiEndpoint(line) {
  const t = String(line ?? '').trim().replace(/^`+|`+$/g, '');
  if (!t) return null;
  let method = 'GET';
  let target = t;
  // `METHOD <target>`, where the target may itself contain spaces: a GeoSPARQL
  // query string carries WKT — ?from=POLYGON((3.5 51, 4.5 51, …)) — and the old
  // \S+ target stopped at the first space, so the SAME endpoint rendered as a
  // runnable block without parameters and as inert text with them. Take the
  // rest of the line as the target and let the checks below judge it.
  const m = /^([A-Z]+)\s+(\S.*)$/i.exec(t);
  if (m) {
    method = m[1].toUpperCase();
    target = m[2].trim();
  } else if (/^\s|\s{2,}/.test(t)) {
    return null;
  }
  // A bare target (no method) is only an endpoint if it is a single token or a
  // path whose spaces are inside its query string — prose is not an endpoint.
  if (!m && /\s/.test(target) && !/^\/api\/[^\s?]*\?/.test(target)) return null;
  let path = target;
  if (/^https?:\/\//i.test(target)) {
    try {
      const u = new URL(target);
      path = u.pathname + u.search;
    } catch {
      return null;
    }
  }
  if (method !== 'GET' || !path.startsWith('/api/')) return null;
  return { method, path };
}

/**
 * Recognise an API-service run path so the runner can use the saved-query client
 * (parameter metadata, version header) instead of a plain fetch.
 * @returns {{scope: 'datasets'|'organisations'|'groups', ownerId, slug, params} | null}
 */
export function describeApiService(path) {
  const m = /^\/api\/(datasets|organisations|groups)\/([^/]+)\/api-services\/([^/?]+)\/run(?:\?(.*))?$/.exec(
    String(path ?? '')
  );
  if (!m) return null;
  const params = {};
  if (m[4]) {
    for (const [k, v] of new URLSearchParams(m[4])) params[k] = v;
  }
  return {
    scope: m[1],
    ownerId: decodeURIComponent(m[2]),
    slug: decodeURIComponent(m[3]),
    params,
  };
}

const str = (v) => (typeof v === 'string' ? v.trim() : typeof v === 'number' ? String(v) : '');

/**
 * JSON.parse with a lenient retry for model output: smaller models sneak `//`
 * or  `/* *\/` comments and trailing commas into widget specs. The stripper is
 * string-aware, so `https://…` inside a value survives. Returns undefined when
 * even the lenient pass fails.
 */
export function lenientJsonParse(text) {
  const s = String(text ?? '');
  try {
    return JSON.parse(s);
  } catch {
    /* lenient pass below */
  }
  let out = '';
  let inStr = false;
  for (let i = 0; i < s.length; i++) {
    const c = s[i];
    if (inStr) {
      out += c;
      if (c === '\\') {
        out += s[i + 1] ?? '';
        i++;
      } else if (c === '"') {
        inStr = false;
      }
      continue;
    }
    if (c === '"') {
      inStr = true;
      out += c;
      continue;
    }
    if (c === '/' && s[i + 1] === '/') {
      while (i < s.length && s[i] !== '\n') i++;
      out += '\n';
      continue;
    }
    if (c === '/' && s[i + 1] === '*') {
      i += 2;
      while (i < s.length && !(s[i] === '*' && s[i + 1] === '/')) i++;
      i++;
      continue;
    }
    out += c;
  }
  out = out.replace(/,\s*([}\]])/g, '$1');
  try {
    return JSON.parse(out);
  } catch {
    return undefined;
  }
}

/** Normalise one chart data array into [{label, value}] with finite values. */
function normPoints(arr) {
  if (!Array.isArray(arr)) return [];
  return arr
    .map((p) => {
      if (Array.isArray(p)) return { label: str(p[0]) || String(p[0] ?? ''), value: Number(p[1]) };
      if (p && typeof p === 'object') {
        return {
          label: str(p.label ?? p.x ?? p.name ?? ''),
          value: Number(p.value ?? p.y ?? p.count),
        };
      }
      return null;
    })
    .filter((p) => p && Number.isFinite(p.value))
    .slice(0, MAX_CHART_POINTS);
}

/** Column index by (case-insensitive) name, tolerating a leading '?'. */
function colIndex(columns, name) {
  if (!name) return -1;
  const want = String(name).replace(/^\?/, '').toLowerCase();
  return (columns || []).findIndex((c) => String(c).replace(/^\?/, '').toLowerCase() === want);
}

/** First column whose cells are predominantly numeric. */
function firstNumericColumn(columns, rows) {
  for (let i = 0; i < (columns || []).length; i++) {
    let num = 0;
    let seen = 0;
    for (const r of rows.slice(0, 20)) {
      const v = r[i];
      if (v == null || v === '') continue;
      seen += 1;
      if (Number.isFinite(Number(v))) num += 1;
    }
    if (seen && num === seen) return i;
  }
  return -1;
}

/**
 * Bind a `"source":"query"` chart to the executed rows: x = the named label
 * column (default: first non-numeric), y = the named value column (default:
 * first numeric). Sorted descending and capped so a 40-graph result stays a
 * readable chart.
 */
function chartFromQueryRows(raw, queryRows) {
  const columns = queryRows?.columns || [];
  const rows = queryRows?.rows || [];
  if (!columns.length || !rows.length) {
    return { error: 'source:"query" but no query rows were produced this turn' };
  }
  let yi = colIndex(columns, raw.y ?? raw.value);
  if (yi < 0) yi = firstNumericColumn(columns, rows);
  if (yi < 0) return { error: 'source:"query": no numeric column for y' };
  let xi = colIndex(columns, raw.x ?? raw.label);
  if (xi < 0) xi = columns.findIndex((_, i) => i !== yi);
  if (xi < 0) return { error: 'source:"query": no label column for x' };
  const data = rows
    .map((r) => ({ label: String(r[xi] ?? ''), value: Number(r[yi]) }))
    .filter((d) => d.label && Number.isFinite(d.value))
    .sort((a, b) => b.value - a.value)
    .slice(0, MAX_CHART_POINTS);
  if (!data.length) return { error: 'source:"query": rows yielded no plottable points' };
  return { series: [{ name: '', data }] };
}

/**
 * Parse + validate a ```chart spec.
 * @returns {{spec?: {type, title, xLabel, yLabel, series: Array<{name, data}>}, error?: string}}
 */
export function parseChartSpec(text, queryRows = null) {
  const raw = lenientJsonParse(text);
  if (raw === undefined) return { error: 'invalid JSON' };
  if (!raw || typeof raw !== 'object') return { error: 'not an object' };
  const type = ['bar', 'line', 'pie'].includes(raw.type) ? raw.type : 'bar';
  let series;
  if (raw.source === 'query' || raw.data === 'query') {
    // Data comes from the turn's executed SPARQL rows — the numbers users see
    // are the numbers the store returned, not the model's transcription of
    // them (a 7B model reliably corrupts digits when copying 30+ values).
    const bound = chartFromQueryRows(raw, queryRows);
    if (bound.error) return bound;
    series = bound.series;
  } else if (Array.isArray(raw.series)) {
    series = raw.series
      .map((s) => ({ name: str(s?.name), data: normPoints(s?.data) }))
      .filter((s) => s.data.length);
  } else {
    const data = normPoints(raw.data);
    series = data.length ? [{ name: '', data }] : [];
  }
  if (!series.length) return { error: 'no data points' };
  return {
    spec: {
      type,
      title: str(raw.title),
      xLabel: str(raw.xLabel ?? raw.x_label),
      yLabel: str(raw.yLabel ?? raw.y_label),
      series,
    },
  };
}

/**
 * A model/file URL is loadable when its scheme passes the shared allowlist
 * (http/https or a same-origin-safe relative reference) AND its extension maps
 * to a viewer format. Returns the detected format, or null.
 */
function safeModelFormat(url) {
  if (!url || safeExternalUrl(url) === undefined) return null;
  return modelFormatFromUrl(url);
}

/**
 * Validate the optional "models" of a ```map spec: each needs a scheme-safe URL
 * with a detectable 3D format and a parseable WKT POINT anchor (a line/area
 * doesn't tell the viewer where the model stands). Invalid entries drop.
 */
function normMapModels(arr) {
  if (!Array.isArray(arr)) return [];
  return arr
    .map((m) => {
      if (!m || typeof m !== 'object') return null;
      const url = str(m.url ?? m.href);
      const wkt = str(m.wkt ?? m.geometry);
      const format = safeModelFormat(url);
      if (!format || parseWktGeometry(wkt)?.kind !== 'point') return null;
      return { label: str(m.label ?? m.name), url, format, wkt };
    })
    .filter(Boolean)
    .slice(0, MAX_3D_MODELS);
}

/**
 * Parse a ```map spec: {"features":[{label?, iri?, wkt}], "models":[{label?,
 * url, wkt}]?}, a bare JSON array, or plain newline-separated WKT strings.
 * When every model is invalid but features remain, the map renders features
 * only (models: []).
 * @returns {{features?: Array<{label, iri, wkt}>, models?: Array<{label, url, format, wkt}>, error?: string}}
 */
export function parseMapSpec(text, queryRows = null) {
  const t = String(text ?? '').trim();
  let features = [];
  let models = [];
  if (t.startsWith('{') || t.startsWith('[')) {
    const raw = lenientJsonParse(t);
    if (raw && typeof raw === 'object' && (raw.source === 'query' || raw.features === 'query')) {
      const columns = queryRows?.columns || [];
      const rows = queryRows?.rows || [];
      if (!columns.length || !rows.length) {
        return { error: 'source:"query" but no query rows were produced this turn' };
      }
      let wi = colIndex(columns, raw.wkt);
      if (wi < 0) {
        wi = columns.findIndex((_, i) =>
          rows.slice(0, 10).some((r) => parseWktGeometry(String(r[i] ?? ''))),
        );
      }
      if (wi < 0) return { error: 'source:"query": no WKT column found' };
      let li = colIndex(columns, raw.label);
      if (li < 0) li = columns.findIndex((_, i) => i !== wi);
      const ii = colIndex(columns, raw.iri);
      const feats = rows
        .map((r) => ({
          label: String((li >= 0 ? r[li] : '') ?? ''),
          iri: String((ii >= 0 ? r[ii] : '') ?? ''),
          wkt: String(r[wi] ?? ''),
        }))
        .filter((f) => parseWktGeometry(f.wkt))
        .slice(0, MAX_MAP_FEATURES);
      if (!feats.length) return { error: 'source:"query": rows yielded no mappable WKT' };
      return { features: feats, models: [] };
    }
    if (raw === undefined) return { error: 'invalid JSON' };
    const arr = Array.isArray(raw) ? raw : raw?.features;
    models = Array.isArray(raw) ? [] : normMapModels(raw?.models);
    if (!Array.isArray(arr) && !models.length) return { error: 'missing "features" array' };
    features = (Array.isArray(arr) ? arr : [])
      .map((f) =>
        typeof f === 'string'
          ? { wkt: f.trim(), label: '', iri: '' }
          : f && typeof f === 'object'
            ? { wkt: str(f.wkt ?? f.geometry), label: str(f.label ?? f.name), iri: str(f.iri ?? f.uri) }
            : null
      )
      .filter((f) => f && f.wkt);
  } else {
    features = t
      .split('\n')
      .map((l) => l.trim())
      .filter(Boolean)
      .map((wkt) => ({ wkt, label: '', iri: '' }));
  }
  features = features.slice(0, MAX_MAP_FEATURES);
  if (!features.length && !models.length) return { error: 'no features' };
  return { features, models };
}

/**
 * Parse a ```model3d spec: {"models":[{label?, url}]} — a single {"url": …}
 * object or a bare array are tolerated. A model is kept only when its URL is
 * scheme-safe and its extension maps to a loadable format (glb/gltf/stl/ifc/
 * CityJSON/CityGML — see viewer/detect.ts); when none survive the block is
 * reported broken.
 * @returns {{models?: Array<{id, label, url, format}>, error?: string}}
 */
export function parseModel3dSpec(text) {
  const raw = lenientJsonParse(text);
  if (raw === undefined) return { error: 'invalid JSON' };
  if (!raw || typeof raw !== 'object') return { error: 'not an object' };
  const arr = Array.isArray(raw) ? raw : Array.isArray(raw.models) ? raw.models : [raw];
  const models = arr
    .map((m) => {
      if (!m || typeof m !== 'object') return null;
      const url = str(m.url ?? m.href);
      // Asset download routes carry no file extension, so the spec may state
      // the format explicitly; the URL still passes the scheme allowlist.
      const KNOWN = ['gltf', 'stl', 'cityjson', 'citygml', 'ifc'];
      const declared = KNOWN.includes(str(m.format).toLowerCase()) ? str(m.format).toLowerCase() : null;
      const format =
        safeModelFormat(url) ?? (declared && safeExternalUrl(url) !== undefined ? declared : null);
      return format ? { label: str(m.label ?? m.name), url, format } : null;
    })
    .filter(Boolean)
    .slice(0, MAX_3D_MODELS)
    .map((m, i) => ({ id: `m${i}`, ...m }));
  if (!models.length) return { error: 'no loadable model URLs' };
  return { models };
}

/**
 * Parse a ```file spec: {"label?", "url", "filename?"}. The URL goes through
 * the shared scheme allowlist; an unsafe scheme (javascript:, data:, …) yields
 * a *blocked* card — the segment keeps the name for display but carries an
 * empty url, so no renderer can turn it into a clickable link.
 * @returns {{file?: {label, url, filename, blocked}, error?: string}}
 */
export function parseFileSpec(text) {
  const raw = lenientJsonParse(text);
  if (raw === undefined) return { error: 'invalid JSON' };
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) return { error: 'not an object' };
  const url = str(raw.url ?? raw.href);
  if (!url) return { error: 'missing url' };
  const safe = safeExternalUrl(url);
  let filename = str(raw.filename ?? raw.name);
  if (!filename && safe) {
    const last = safe.split(/[?#]/)[0].split('/').pop() || '';
    try {
      filename = decodeURIComponent(last);
    } catch {
      filename = last;
    }
  }
  return {
    file: {
      label: str(raw.label ?? raw.title),
      filename,
      url: safe ?? '',
      blocked: safe === undefined,
    },
  };
}

/**
 * Parse a ```card spec into an entity info card.
 * @returns {{card?: {title, subtitle, iri, image, facts}, error?: string}}
 */
export function parseInfoCard(text) {
  const raw = lenientJsonParse(text);
  if (raw === undefined) return { error: 'invalid JSON' };
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) return { error: 'not an object' };
  const facts = Array.isArray(raw.facts)
    ? raw.facts
        .map((f) => ({
          label: str(f?.label ?? f?.name),
          value: str(f?.value),
          iri: str(f?.iri ?? f?.uri),
        }))
        .filter((f) => f.label && (f.value || f.iri))
        .slice(0, MAX_CARD_FACTS)
    : [];
  const card = {
    title: str(raw.title) || str(raw.label),
    subtitle: str(raw.subtitle) || str(raw.description),
    iri: str(raw.iri) || str(raw.uri),
    image: str(raw.image),
    facts,
  };
  if (!card.title) return { error: 'missing title' };
  return { card };
}

/**
 * Small RFC 4180-ish CSV parser (quoted fields, "" escapes, CR/LF). The first
 * record is the header. Rows are padded/truncated to the header width.
 * @returns {{columns: string[], rows: string[][]}}
 */
export function parseCsv(text, { maxRows = MAX_CSV_ROWS } = {}) {
  const s = String(text ?? '');
  const records = [];
  let field = '';
  let record = [];
  let inQuotes = false;
  const endField = () => {
    record.push(field);
    field = '';
  };
  const endRecord = () => {
    endField();
    if (record.length > 1 || record[0].trim() !== '') records.push(record);
    record = [];
  };
  for (let i = 0; i < s.length; i++) {
    const c = s[i];
    if (inQuotes) {
      if (c === '"') {
        if (s[i + 1] === '"') {
          field += '"';
          i++;
        } else {
          inQuotes = false;
        }
      } else {
        field += c;
      }
    } else if (c === '"' && field === '') {
      inQuotes = true;
    } else if (c === ',') {
      endField();
    } else if (c === '\n') {
      endRecord();
    } else if (c !== '\r') {
      field += c;
    }
  }
  if (field !== '' || record.length) endRecord();
  if (!records.length) return { columns: [], rows: [] };
  const columns = records[0].map((c) => c.trim());
  const rows = records
    .slice(1, 1 + maxRows)
    .map((r) => columns.map((_, i) => r[i] ?? ''));
  return { columns, rows };
}

/**
 * Normalise a SPARQL endpoint response (lib/api.js sparqlQuery/runSavedQuery
 * JSON) into one of three renderable shapes.
 * @returns {{kind:'graph', ntriples} | {kind:'boolean', value} | {kind:'bindings', vars, bindings}}
 */
export function normalizeSparqlResult(resp) {
  if (resp && resp._graphResult) return { kind: 'graph', ntriples: String(resp.ntriples || '') };
  if (typeof resp?.boolean === 'boolean') return { kind: 'boolean', value: resp.boolean };
  return {
    kind: 'bindings',
    vars: resp?.head?.vars || [],
    bindings: resp?.results?.bindings || [],
  };
}

/**
 * Post-process sanitized markdown HTML: inline `<code>GET /api/...</code>` spans
 * become keyboard-accessible run buttons (the chat component handles the click
 * via delegation), and anchors whose href is an API-service run URL become run
 * chips too — following such a link would just dump raw JSON in the browser.
 * Only attributes are added to existing sanitized nodes, so this cannot
 * introduce markup from the (model-controlled) message.
 */
/**
 * Turn IRIs in an answer into links to their resource page.
 *
 * An answer names things by IRI constantly — that IS the identifier in a
 * knowledge graph — but they rendered as dead monospace text, so the one thing
 * a reader wants next ("what IS that thing?") meant copying the IRI into the
 * browser. Decorated the same way as the API chips: attributes only, on
 * already-sanitized HTML.
 *
 * Only absolute http(s)/urn IRIs qualify, and only outside code BLOCKS — a
 * query or a Turtle sample is source, not a set of links.
 */
export function decorateIriLinks(html) {
  if (typeof DOMParser === 'undefined') return html;
  const doc = new DOMParser().parseFromString(String(html ?? ''), 'text/html');
  const isIri = (t) => /^(https?:\/\/|urn:)[^\s<>"']+$/i.test(t);
  const decorate = (el, iri) => {
    el.classList.add('chat-iri-link');
    el.setAttribute('data-iri', iri);
    el.setAttribute('role', 'button');
    el.setAttribute('tabindex', '0');
    el.setAttribute('title', iri);
  };
  doc.querySelectorAll('code').forEach((code) => {
    if (code.closest('pre') || code.closest('a')) return;
    if (code.classList.contains('chat-api-link')) return; // already an endpoint
    const text = (code.textContent || '').trim().replace(/^[<]|[>]$/g, '');
    if (isIri(text)) decorate(code, text);
  });
  // marked auto-links bare URLs; an IRI is not a page to browse away to, so
  // swap those for the same in-app chip.
  doc.querySelectorAll('a[href]').forEach((a) => {
    const href = (a.getAttribute('href') || '').trim();
    if (!isIri(href) || a.closest('pre')) return;
    const code = doc.createElement('code');
    code.textContent = a.textContent || href;
    decorate(code, href);
    a.replaceWith(code);
  });
  return doc.body.innerHTML;
}

export function decorateApiLinks(html) {
  if (typeof DOMParser === 'undefined') return html;
  const doc = new DOMParser().parseFromString(String(html ?? ''), 'text/html');
  const decorate = (el, ep) => {
    el.classList.add('chat-api-link');
    el.setAttribute('data-method', ep.method);
    el.setAttribute('data-path', ep.path);
    el.setAttribute('role', 'button');
    el.setAttribute('tabindex', '0');
    el.setAttribute('title', `${ep.method} ${ep.path}`);
  };
  doc.querySelectorAll('code').forEach((code) => {
    if (code.closest('pre') || code.closest('a')) return;
    const ep = parseApiEndpoint(code.textContent || '');
    if (ep) decorate(code, ep);
  });
  doc.querySelectorAll('a[href]').forEach((a) => {
    const ep = parseApiEndpoint(a.getAttribute('href') || '');
    if (!ep || !describeApiService(ep.path)) return;
    const code = doc.createElement('code');
    code.textContent = a.textContent || `${ep.method} ${ep.path}`;
    decorate(code, ep);
    a.replaceWith(code);
  });
  return doc.body.innerHTML;
}
