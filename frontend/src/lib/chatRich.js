// Spark chat answers are markdown plus a small set of fenced "widget" blocks the
// model may emit (see CHAT_SYSTEM_PROMPT in src/server/llm_sparql.rs):
//
//   ```sparql  → runnable query card            ```chart → JSON chart spec
//   ```api     → runnable GET /api/... call     ```map   → JSON WKT feature map
//   ```csv     → table preview with download    ```card  → entity info card
//
// parseChatBlocks() splits a message into ordered segments: plain markdown runs
// (rendered by the caller through renderMarkdown) interleaved with typed widget
// segments. Unknown / malformed blocks fall back into the markdown flow (or a
// 'broken' segment with the raw text), so a model that gets a spec wrong still
// produces a readable answer. Everything here is pure parsing — no DOM except
// decorateApiLinks(), which post-processes already-sanitized HTML.

/** A fence opener/closer: ``` or ~~~, optionally with a language tag. */
const FENCE_RE = /^\s*(```+|~~~+)\s*([\w+-]*)\s*$/;

/** Hard caps so a hostile/confused answer cannot freeze the tab. */
const MAX_CHART_POINTS = 100;
const MAX_MAP_FEATURES = 200;
const MAX_CARD_FACTS = 24;
const MAX_CSV_ROWS = 500;

/** Fence languages that render as live widgets (see specialSegment). */
const SPECIAL_LANGS = [
  'sparql', 'rq', 'api', 'http', 'endpoint', 'chart',
  'map', 'geo', 'card', 'infocard', 'info-card', 'csv',
];

/** Canonical widget kind for a special fence language (for pending labels). */
function canonicalKind(lang) {
  switch (lang) {
    case 'rq': return 'sparql';
    case 'http': case 'endpoint': return 'api';
    case 'geo': return 'map';
    case 'infocard': case 'info-card': return 'card';
    default: return lang;
  }
}

/**
 * Split an assistant message into renderable segments.
 *
 * With `streaming: true` the source is a partial, still-growing answer: an
 * unclosed widget fence at the end becomes a `{kind:'pending'}` placeholder
 * instead of a half-parsed (and flickering) widget or broken-block error. A
 * fence whose language tag may still be incomplete (it's the very last line,
 * e.g. "```cha…") is held as a generic pending block rather than guessing.
 *
 * @param {string} source - the raw markdown answer
 * @param {{streaming?: boolean}} [opts]
 * @returns {Array<object>} segments: {kind:'md', source} | {kind:'sparql', code}
 *   | {kind:'api', method, path} | {kind:'chart', spec} | {kind:'map', features}
 *   | {kind:'card', card} | {kind:'csv', columns, rows, raw}
 *   | {kind:'broken', label, error, raw} | {kind:'pending', label}
 */
export function parseChatBlocks(source, { streaming = false } = {}) {
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
    if (close === -1 && streaming) {
      // The block is still being generated. Widget fences become a placeholder;
      // ordinary code fences stay in the markdown flow for a live code preview.
      const langFinal = i < lines.length - 1; // a body line exists ⇒ the tag line was completed
      if (langFinal ? SPECIAL_LANGS.includes(lang) : couldBecomeSpecial(lang)) {
        flushMd();
        segments.push({
          kind: 'pending',
          label: SPECIAL_LANGS.includes(lang) ? canonicalKind(lang) : '',
        });
      } else {
        md.push(...lines.slice(i));
        flushMd();
      }
      return segments;
    }
    const body = lines.slice(i + 1, close === -1 ? lines.length : close).join('\n');
    const seg = specialSegment(lang, body);
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

/** Is `lang` a (possibly still-typing) prefix of any widget language? */
function couldBecomeSpecial(lang) {
  return SPECIAL_LANGS.some((s) => s.startsWith(lang));
}

/** Stable identity key for one parsed segment (see reuseSegments). */
function segKey(seg) {
  switch (seg.kind) {
    case 'md': return `md:${seg.source}`;
    case 'sparql': return `sparql:${seg.code}`;
    case 'api': return `api:${seg.method} ${seg.path}`;
    case 'chart': return `chart:${JSON.stringify(seg.spec)}`;
    case 'map': return `map:${JSON.stringify(seg.features)}`;
    case 'card': return `card:${JSON.stringify(seg.card)}`;
    case 'csv': return `csv:${seg.raw}`;
    case 'pending': return `pending:${seg.label}`;
    case 'broken': return `broken:${seg.label}:${seg.raw}`;
    default: return seg.kind;
  }
}

/**
 * Re-parsing a streaming message produces all-new segment objects every token,
 * which would re-render every widget (charts, Leaflet maps) on each delta. Keep
 * the previous object whenever a segment is unchanged, so Svelte sees identical
 * props and leaves settled widgets alone — only the still-growing tail updates.
 */
export function reuseSegments(prev, next) {
  if (!prev || !prev.length) return next;
  return next.map((seg, i) => {
    const p = prev[i];
    return p && p.kind === seg.kind && segKey(p) === segKey(seg) ? p : seg;
  });
}

/** Map one fenced block to a widget segment, or null to leave it as markdown. */
function specialSegment(lang, body) {
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
      const r = parseChartSpec(code);
      return r.error
        ? { kind: 'broken', label: 'chart', error: r.error, raw: code }
        : { kind: 'chart', spec: r.spec };
    }
    case 'map':
    case 'geo': {
      const r = parseMapSpec(code);
      return r.error
        ? { kind: 'broken', label: 'map', error: r.error, raw: code }
        : { kind: 'map', features: r.features };
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
  const m = /^([A-Z]+)\s+(\S+)$/i.exec(t);
  if (m) {
    method = m[1].toUpperCase();
    target = m[2];
  } else if (/\s/.test(t)) {
    return null;
  }
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

/**
 * Parse + validate a ```chart spec.
 * @returns {{spec?: {type, title, xLabel, yLabel, series: Array<{name, data}>}, error?: string}}
 */
export function parseChartSpec(text) {
  let raw;
  try {
    raw = JSON.parse(text);
  } catch {
    return { error: 'invalid JSON' };
  }
  if (!raw || typeof raw !== 'object') return { error: 'not an object' };
  const type = ['bar', 'line', 'pie'].includes(raw.type) ? raw.type : 'bar';
  let series;
  if (Array.isArray(raw.series)) {
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
 * Parse a ```map spec: {"features":[{label?, iri?, wkt}]}, a bare JSON array, or
 * plain newline-separated WKT strings.
 * @returns {{features?: Array<{label, iri, wkt}>, error?: string}}
 */
export function parseMapSpec(text) {
  const t = String(text ?? '').trim();
  let features = [];
  if (t.startsWith('{') || t.startsWith('[')) {
    let raw;
    try {
      raw = JSON.parse(t);
    } catch {
      return { error: 'invalid JSON' };
    }
    const arr = Array.isArray(raw) ? raw : raw?.features;
    if (!Array.isArray(arr)) return { error: 'missing "features" array' };
    features = arr
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
  if (!features.length) return { error: 'no features' };
  return { features };
}

/**
 * Parse a ```card spec into an entity info card.
 * @returns {{card?: {title, subtitle, iri, image, facts}, error?: string}}
 */
export function parseInfoCard(text) {
  let raw;
  try {
    raw = JSON.parse(text);
  } catch {
    return { error: 'invalid JSON' };
  }
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
