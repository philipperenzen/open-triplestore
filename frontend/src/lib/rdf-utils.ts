// In-memory store: namespace → prefix label (no longer fetched from prefix.cc
// whose certificate has expired; COMMON_PREFIXES covers all well-known prefixes)
const _prefixCcStore: Record<string, string> = {};

/** No-op kept for call-site compatibility. */
export function loadPrefixCcPrefixes(): void {}

// Common RDF namespace prefixes
export const COMMON_PREFIXES = {
  'http://www.w3.org/1999/02/22-rdf-syntax-ns#': 'rdf',
  'http://www.w3.org/2000/01/rdf-schema#': 'rdfs',
  'http://www.w3.org/2002/07/owl#': 'owl',
  'http://www.w3.org/ns/shacl#': 'sh',
  'http://xmlns.com/foaf/0.1/': 'foaf',
  'http://schema.org/': 'schema',
  'http://www.w3.org/2004/02/skos/core#': 'skos',
  'http://purl.org/dc/terms/': 'dct',
  'http://purl.org/dc/elements/1.1/': 'dc',
  'http://www.w3.org/ns/dcat#': 'dcat',
  'http://www.w3.org/2001/XMLSchema#': 'xsd',
  'http://www.opengis.net/ont/geosparql#': 'geo',
  'http://www.w3.org/ns/prov#': 'prov',
  'http://www.w3.org/ns/void#': 'void',
  'http://www.w3.org/2006/time#': 'time',
  'http://purl.org/linked-data/cube#': 'qb',
  'http://www.w3.org/ns/org#': 'org',
  'http://www.w3.org/ns/adms#': 'adms',
};

// Build inverse map: prefix label → namespace URI
export const PREFIX_LABEL_MAP = Object.fromEntries(
  Object.entries(COMMON_PREFIXES).map(([ns, label]) => [label, ns])
);

/**
 * Shorten a full IRI using known prefixes.
 * @param {string} iri
 * @param {Object} [extraPrefixes] - additional {namespace: label} pairs
 * @returns {string} shortened form like "foaf:Person" or "…/Person"
 */
// Memoization cache for shortenIRI. The same IRI (especially predicates like
// rdf:type) is shortened hundreds of times when rendering a graph or table page.
// Bounded to keep memory in check on long sessions.
const _shortenCache = new Map<string, string>();
const _SHORTEN_CACHE_MAX = 4096;

export function shortenIRI(iri: string, extraPrefixes: Record<string, string> = {}): string {
  // Defensive: some callers (e.g. graph rendering of RDF-star quoted triples) may
  // pass a non-string term value (the SPARQL-JSON shape for an embedded triple is
  // an object). Never throw on `.startsWith` — coerce to a safe string instead.
  if (typeof iri !== 'string') return iri == null ? '' : String(iri);
  if (!iri) return '';
  // Only memoize the common (no-extras) call shape \u2014 that's the hot path.
  const useCache = !extraPrefixes || Object.keys(extraPrefixes).length === 0;
  if (useCache) {
    const hit = _shortenCache.get(iri);
    if (hit !== undefined) return hit;
  }
  // COMMON_PREFIXES wins over prefix.cc to keep stable well-known abbreviations
  const allPrefixes = { ..._prefixCcStore, ...COMMON_PREFIXES, ...extraPrefixes };
  let result = iri;
  let matched = false;
  for (const [ns, label] of Object.entries(allPrefixes)) {
    if (iri.startsWith(ns)) {
      const local = iri.slice(ns.length);
      if (local && !local.includes('/') && !local.includes('#')) {
        result = `${label}:${local}`;
        matched = true;
        break;
      }
    }
  }
  if (!matched) {
    // Fallback: derive a short prefix label from the last namespace segment so
    // unknown IRIs render as  showcase:BridgeDataset  instead of  …/BridgeDataset
    const idx = Math.max(iri.lastIndexOf('#'), iri.lastIndexOf('/'));
    if (idx > 0 && idx < iri.length - 1) {
      const local = iri.slice(idx + 1);
      const ns = iri.slice(0, idx);
      const nsLabel = ns.replace(/[/#]+$/, '').split('/').filter(Boolean).pop() || 'ns';
      result = `${nsLabel}:${local}`;
    }
  }
  if (useCache) {
    if (_shortenCache.size >= _SHORTEN_CACHE_MAX) {
      // Drop oldest (Map preserves insertion order) \u2014 O(1) eviction.
      const oldest = _shortenCache.keys().next().value;
      if (oldest !== undefined) _shortenCache.delete(oldest);
    }
    _shortenCache.set(iri, result);
  }
  return result;
}

/**
 * Expand a prefixed name to a full IRI.
 * @param {string} prefixed - e.g. "foaf:Person"
 * @param {Object} [extraPrefixes] - additional {label: namespace} pairs
 * @returns {string|null}
 */
export function expandPrefix(prefixed: string, extraPrefixes: Record<string, string> = {}): string | null {
  if (!prefixed || !prefixed.includes(':')) return null;
  const colon = prefixed.indexOf(':');
  const label = prefixed.slice(0, colon);
  const local = prefixed.slice(colon + 1);
  const allMap = { ...PREFIX_LABEL_MAP, ...extraPrefixes };
  if (allMap[label]) return allMap[label] + local;
  return null;
}

/**
 * Get a display label for an RDF term.
 * @param {{type: string, value: string, datatype?: string, language?: string}} term
 * @returns {string}
 */
interface RdfTerm {
  type: string;
  value?: string;
  language?: string;
  datatype?: string;
}

export function termLabel(term: RdfTerm | null | undefined): string {
  if (!term) return '';
  if (term.type === 'uri' || term.type === 'iri') return shortenIRI(term.value);
  if (term.type === 'literal') {
    const lang = term.language ? `@${term.language}` : '';
    return `"${term.value}"${lang}`;
  }
  if (term.type === 'bnode') return `_:${term.value}`;
  return term.value || '';
}

/**
 * Get a CSS color string for an RDF term type.
 * @param {{type: string}} term
 * @returns {string}
 */
export function termColor(term: RdfTerm | null | undefined): string {
  if (!term) return '#888';
  switch (term.type) {
    case 'uri':
    case 'iri': return '#4a90d9';
    case 'literal': return '#2e8b57';
    case 'bnode': return '#888';
    default: return '#333';
  }
}

/**
 * Convert SPARQL SELECT results bindings to Cytoscape elements.
 * Expects bindings with ?s ?p ?o variables.
 * @param {Array} bindings - SPARQL result bindings
 * @param {string} [sVar='s'] - subject variable name
 * @param {string} [pVar='p'] - predicate variable name
 * @param {string} [oVar='o'] - object variable name
 * @param {number} [maxNodes=200] - cap to avoid browser freeze
 * @returns {{ nodes: Array, edges: Array }}
 */
interface CytoscapeNode {
  data: {
    id: string;
    label: string;
    fullIri: string | null;
    nodeType: string;
    isLiteral?: boolean;
    literalValue?: string | null;
    datatype?: string | null;
    language?: string | null;
    degree: number;
    rdfType: string | null;
  };
}

interface CytoscapeEdge {
  data: {
    id: string;
    source: string;
    target: string;
    label: string;
    predicate: string;
  };
}

export function graphResultsToElements(
  bindings: Record<string, RdfTerm>[],
  sVar = 's',
  pVar = 'p',
  oVar = 'o',
  maxNodes = 200
): { nodes: CytoscapeNode[]; edges: CytoscapeEdge[] } {
  const nodeMap = new Map();
  const edges = [];
  // Dedupe edges within one call: the same triple surfaced from multiple named
  // graphs would otherwise emit duplicate-id edges and make cytoscape's batch add throw.
  const edgeIds = new Set();
  // Track degree (connection count) per node id
  const degreeMap = new Map();
  // Track rdf:type assertions for ontology-aware coloring
  const typeMap = new Map();

  const RDF_TYPE = 'http://www.w3.org/1999/02/22-rdf-syntax-ns#type';

  for (const row of bindings) {
    const s = row[sVar];
    const p = row[pVar];
    const o = row[oVar];
    if (!s || !p || !o) continue;
    // RDF-star quoted triples (type 'triple', whose `value` is an object rather
    // than a string IRI/literal) can't be represented as a simple graph node or
    // edge. Skip them so the graph view degrades gracefully instead of throwing
    // when a non-string value reaches shortenIRI. The table view still renders
    // them via the RdfTerm component, which understands quoted triples.
    if (typeof s.value !== 'string' || typeof p.value !== 'string' || typeof o.value !== 'string') continue;

    // Capture rdf:type assertions for later node decoration
    if (p.value === RDF_TYPE && (o.type === 'uri' || o.type === 'iri')) {
      typeMap.set(s.value, shortenIRI(o.value));
    }

    // Add subject node
    if (!nodeMap.has(s.value)) {
      nodeMap.set(s.value, {
        data: {
          id: s.value,
          label: shortenIRI(s.value),
          fullIri: s.value,
          nodeType: s.type === 'bnode' ? 'bnode' : 'uri',
          degree: 0,
          rdfType: null,
        }
      });
    }

    // Add object node. URIs/bnodes use their own value as id. Literals are keyed
    // by their connecting triple (subject, predicate, value, datatype, language)
    // so the same literal yields the same id across calls — letting incremental
    // expansion dedupe it instead of re-adding a duplicate literal node. Keying on
    // subject+predicate keeps distinct occurrences separate (two entities sharing a
    // value stay separate nodes rather than collapsing into one hub).
    const objId = o.type === 'literal'
      ? `literal::${s.value}::${p.value}::${o.value}::${o.datatype || ''}::${o.language || ''}`
      : o.value;

    if (!nodeMap.has(objId)) {
      nodeMap.set(objId, {
        data: {
          id: objId,
          label: termLabel(o),
          fullIri: (o.type === 'uri' || o.type === 'iri') ? o.value : null,
          nodeType: o.type === 'literal' ? 'literal' : o.type === 'bnode' ? 'bnode' : 'uri',
          isLiteral: o.type === 'literal' || undefined,
          literalValue: o.type === 'literal' ? o.value : null,
          datatype: o.type === 'literal' ? (o.datatype || null) : null,
          language: o.type === 'literal' ? (o.language || null) : null,
          degree: 0,
          rdfType: null,
        }
      });
    }

    // Add edge (id stable + unique across calls). Skip identical edges already
    // emitted in this batch, and count degree only for edges we keep.
    const edgeId = `${s.value}::${p.value}::${objId}`;
    if (!edgeIds.has(edgeId)) {
      edgeIds.add(edgeId);
      degreeMap.set(s.value, (degreeMap.get(s.value) || 0) + 1);
      degreeMap.set(objId, (degreeMap.get(objId) || 0) + 1);
      edges.push({
        data: {
          id: edgeId,
          source: s.value,
          target: objId,
          label: shortenIRI(p.value),
          predicate: p.value,
        }
      });
    }

    if (nodeMap.size >= maxNodes) break;
  }

  // Annotate nodes with degree and rdfType
  const nodes = Array.from(nodeMap.values());
  for (const node of nodes) {
    node.data.degree = degreeMap.get(node.data.id) || 1;
    if (typeMap.has(node.data.id)) {
      node.data.rdfType = typeMap.get(node.data.id);
    }
  }

  return { nodes, edges };
}

/**
 * Parse N-Triples text into SPARQL-results-style bindings with ?s ?p ?o.
 * Each line: <subject> <predicate> <object> .
 * @param {string} ntriples
 * @returns {{ head: { vars: string[] }, results: { bindings: Object[] } }}
 */
export function parseNTriplesToBindings(ntriples: string): { head: { vars: string[] }; results: { bindings: Record<string, RdfTerm>[] } } {
  const bindings = [];
  for (const line of ntriples.split('\n')) {
    const trimmed = line.trim();
    if (!trimmed || trimmed.startsWith('#')) continue;

    const terms = [];
    let i = 0;
    while (i < trimmed.length && terms.length < 3) {
      if (trimmed[i] === '<') {
        // IRI
        const end = trimmed.indexOf('>', i);
        if (end === -1) break;
        terms.push({ type: 'uri', value: trimmed.slice(i + 1, end) });
        i = end + 1;
      } else if (trimmed[i] === '_' && trimmed[i + 1] === ':') {
        // Blank node
        const start = i + 2;
        let end = start;
        while (end < trimmed.length && /\S/.test(trimmed[end]) && trimmed[end] !== '.') end++;
        terms.push({ type: 'bnode', value: trimmed.slice(start, end).trim() });
        i = end;
      } else if (trimmed[i] === '"') {
        // Literal — find the closing quote (handling escaped quotes)
        let j = i + 1;
        while (j < trimmed.length) {
          if (trimmed[j] === '\\') { j += 2; continue; }
          if (trimmed[j] === '"') break;
          j++;
        }
        const lexical = trimmed.slice(i + 1, j).replace(/\\"/g, '"').replace(/\\\\/g, '\\');
        j++; // skip closing quote
        let lang = '';
        let datatype = '';
        if (trimmed[j] === '@') {
          const start = j + 1;
          let end = start;
          while (end < trimmed.length && /[a-zA-Z0-9-]/.test(trimmed[end])) end++;
          lang = trimmed.slice(start, end);
          j = end;
        } else if (trimmed[j] === '^' && trimmed[j + 1] === '^') {
          j += 2;
          if (trimmed[j] === '<') {
            const end = trimmed.indexOf('>', j);
            if (end !== -1) {
              datatype = trimmed.slice(j + 1, end);
              j = end + 1;
            }
          }
        }
        const term: { type: string; value: string; language?: string; datatype?: string } = { type: 'literal', value: lexical };
        if (lang) term.language = lang;
        if (datatype && datatype !== 'http://www.w3.org/2001/XMLSchema#string') term.datatype = datatype;
        terms.push(term);
        i = j;
      } else {
        i++;
      }
    }

    if (terms.length === 3) {
      bindings.push({ s: terms[0], p: terms[1], o: terms[2] });
    }
  }
  return {
    head: { vars: ['s', 'p', 'o'] },
    results: { bindings },
  };
}

/**
 * Detect RDF format from file name or MIME type.
 * @param {string} filename
 * @returns {{ contentType: string, label: string }}
 */
export function detectRdfFormat(filename: string): { contentType: string; label: string } {
  const lower = filename.toLowerCase();
  if (lower.endsWith('.ttl') || lower.endsWith('.n3')) {
    return { contentType: 'text/turtle', label: 'Turtle' };
  }
  if (lower.endsWith('.nt')) {
    return { contentType: 'application/n-triples', label: 'N-Triples' };
  }
  if (lower.endsWith('.nq')) {
    return { contentType: 'application/n-quads', label: 'N-Quads' };
  }
  if (lower.endsWith('.trig')) {
    return { contentType: 'application/trig', label: 'TriG' };
  }
  if (lower.endsWith('.rdf') || lower.endsWith('.owl')) {
    return { contentType: 'application/rdf+xml', label: 'RDF/XML' };
  }
  if (lower.endsWith('.jsonld') || lower.endsWith('.json')) {
    return { contentType: 'application/ld+json', label: 'JSON-LD' };
  }
  return { contentType: 'text/turtle', label: 'Turtle (default)' };
}

/**
 * Validate that a string is an absolute IRI.
 * @param {string} iri
 * @returns {boolean}
 */
/**
 * Parse a SPARQL UPDATE string and extract INSERT/DELETE triple bindings for preview.
 * Handles INSERT DATA, DELETE DATA, and GRAPH { } wrappers.
 * Returns { inserts, deletes, isPatternBased } where inserts/deletes are binding arrays.
 * @param {string} text - SPARQL UPDATE text
 * @returns {{ inserts: Object[], deletes: Object[], isPatternBased: boolean }}
 */
export function parseSparqlUpdatePreview(text: string): { inserts: Record<string, RdfTerm>[]; deletes: Record<string, RdfTerm>[]; isPatternBased: boolean } {
  // Build prefix map from PREFIX and @prefix declarations
  const prefixes = {};
  for (const m of text.matchAll(/PREFIX\s+([a-zA-Z0-9_-]*):\s*<([^>]+)>/gi))
    prefixes[m[1]] = m[2];
  for (const m of text.matchAll(/@prefix\s+([a-zA-Z0-9_-]*):\s*<([^>]+)>\s*\./gi))
    prefixes[m[1]] = m[2];

  // Expand prefixed names inside a content block to full <IRI> form
  function expandPrefixes(content) {
    return content.replace(/\b([a-zA-Z][a-zA-Z0-9_-]*):([\w/#.-]+)/g, (_, pfx, local) => {
      if (pfx === 'http' || pfx === 'https' || pfx === 'urn') return `${pfx}:${local}`;
      return prefixes[pfx] !== undefined ? `<${prefixes[pfx]}${local}>` : `${pfx}:${local}`;
    });
  }

  // Extract content between balanced { } starting at `start` index
  function extractBraces(str, start) {
    let depth = 0, i = start;
    while (i < str.length) {
      if (str[i] === '{') { if (depth === 0) start = i + 1; depth++; }
      else if (str[i] === '}') { depth--; if (depth === 0) return str.slice(start, i); }
      i++;
    }
    return '';
  }

  // Parse triples from a block (may contain nested GRAPH { } blocks)
  function parseBlock(block) {
    const bindings = [];
    // Handle GRAPH <iri> { ... } sub-blocks
    const graphRe = /\bGRAPH\s+(<[^>]+>|[a-zA-Z][a-zA-Z0-9_-]*:[^\s{]*)\s*\{/gi;
    let m;
    let consumed = new Set();
    while ((m = graphRe.exec(block)) !== null) {
      const inner = extractBraces(block, m.index + m[0].length - 1);
      const expanded = expandPrefixes(inner);
      const parsed = parseNTriplesToBindings(expanded);
      bindings.push(...parsed.results.bindings);
      consumed.add(m.index);
    }
    // Default-graph triples (remove GRAPH blocks first)
    const stripped = block.replace(/\bGRAPH\s+(?:<[^>]+>|[a-zA-Z][a-zA-Z0-9_-]*:[^\s{]*)\s*\{[^{}]*(?:\{[^{}]*\}[^{}]*)?\}/gi, '');
    const expanded = expandPrefixes(stripped);
    const parsed = parseNTriplesToBindings(expanded);
    bindings.push(...parsed.results.bindings);
    return bindings;
  }

  // Detect WHERE-based updates (patterns rather than explicit data)
  const isPatternBased = /\bWHERE\b/i.test(text);

  // Find INSERT DATA { } and DELETE DATA { } blocks
  const inserts = [], deletes = [];
  const insertRe = /\bINSERT\s+DATA\s*\{/gi;
  const deleteRe = /\bDELETE\s+DATA\s*\{/gi;

  let match;
  while ((match = insertRe.exec(text)) !== null)
    inserts.push(...parseBlock(extractBraces(text, match.index + match[0].length - 1)));
  while ((match = deleteRe.exec(text)) !== null)
    deletes.push(...parseBlock(extractBraces(text, match.index + match[0].length - 1)));

  return { inserts, deletes, isPatternBased };
}

// ─── RDF serializers ──────────────────────────────────────────────────────────

interface Triple {
  subject?: RdfTerm;
  predicate?: RdfTerm;
  object?: RdfTerm;
  graph?: RdfTerm;
}

function ntTerm(term: RdfTerm | undefined): string {
  if (!term) return '""';
  if (term.type === 'uri' || term.type === 'iri') return `<${term.value}>`;
  if (term.type === 'bnode') return `_:${term.value}`;
  // literal
  const esc = (term.value || '').replace(/\\/g, '\\\\').replace(/"/g, '\\"').replace(/\n/g, '\\n').replace(/\r/g, '\\r').replace(/\t/g, '\\t');
  if (term.language) return `"${esc}"@${term.language}`;
  if (term.datatype && term.datatype !== 'http://www.w3.org/2001/XMLSchema#string') return `"${esc}"^^<${term.datatype}>`;
  return `"${esc}"`;
}

/** Serialize triples as N-Triples (no graph). */
export function toNTriples(triples: Triple[]): string {
  return triples
    .map(t => `${ntTerm(t.subject)} ${ntTerm(t.predicate)} ${ntTerm(t.object)} .`)
    .join('\n');
}

/** Serialize triples as N-Quads (includes graph). */
export function toNQuads(triples: Triple[]): string {
  return triples
    .map(t => {
      const g = t.graph?.value ? ` <${t.graph.value}>` : '';
      return `${ntTerm(t.subject)} ${ntTerm(t.predicate)} ${ntTerm(t.object)}${g} .`;
    })
    .join('\n');
}

function ttlTerm(term: RdfTerm | undefined, prefixes: Record<string, string>): string {
  if (!term) return '""';
  if (term.type === 'uri' || term.type === 'iri') {
    const short = shortenIRI(term.value || '', prefixes);
    // Only use prefixed form if it's genuinely abbreviated (no <…> needed)
    if (short !== term.value && !short.startsWith('\u2026') && /^[a-zA-Z0-9_.-]+:[a-zA-Z0-9_.-]+$/.test(short)) return short;
    return `<${term.value}>`;
  }
  if (term.type === 'bnode') return `_:${term.value}`;
  const esc = (term.value || '').replace(/\\/g, '\\\\').replace(/"/g, '\\"').replace(/\n/g, '\\n');
  if (term.language) return `"${esc}"@${term.language}`;
  if (term.datatype && term.datatype !== 'http://www.w3.org/2001/XMLSchema#string') return `"${esc}"^^${ttlTerm({ type: 'uri', value: term.datatype }, prefixes)}`;
  return `"${esc}"`;
}

/** Serialize triples as Turtle (no named graphs). */
export function toTurtle(triples: Triple[]): string {
  const prefixes = { ...COMMON_PREFIXES };
  const prefixBlock = Object.entries(prefixes)
    .map(([ns, label]) => `@prefix ${label}: <${ns}> .`)
    .join('\n');

  const lines = triples.map(t =>
    `${ttlTerm(t.subject, prefixes)} ${ttlTerm(t.predicate, prefixes)} ${ttlTerm(t.object, prefixes)} .`
  );
  return prefixBlock + '\n\n' + lines.join('\n');
}

/** Serialize triples as TriG (groups by named graph). */
export function toTrig(triples: Triple[]): string {
  const prefixes = { ...COMMON_PREFIXES };
  const prefixBlock = Object.entries(prefixes)
    .map(([ns, label]) => `@prefix ${label}: <${ns}> .`)
    .join('\n');

  // Group by graph IRI (null key = default graph)
  const byGraph = new Map<string | null, Triple[]>();
  for (const t of triples) {
    const g = t.graph?.value || null;
    if (!byGraph.has(g)) byGraph.set(g, []);
    byGraph.get(g)!.push(t);
  }

  const blocks: string[] = [];
  for (const [g, ts] of byGraph) {
    const inner = ts.map(t =>
      `  ${ttlTerm(t.subject, prefixes)} ${ttlTerm(t.predicate, prefixes)} ${ttlTerm(t.object, prefixes)} .`
    ).join('\n');
    if (g) blocks.push(`<${g}> {\n${inner}\n}`);
    else   blocks.push(`{\n${inner}\n}`);
  }

  return prefixBlock + '\n\n' + blocks.join('\n\n');
}

export function isValidIri(iri: unknown): boolean {
  if (!iri || typeof iri !== 'string') return false;
  try { new URL(iri); return true; } catch {}
  return /^[a-zA-Z][a-zA-Z0-9+\-.]*:.+/.test(iri);
}

/**
 * Convert SPARQL JSON results to CSV string.
 * @param {{ head: { vars: string[] }, results: { bindings: Object[] } }} results
 * @returns {string}
 */
interface SparqlResults {
  head: { vars: string[] };
  results: { bindings: Record<string, RdfTerm>[] };
}

export function resultsToCsv(results: SparqlResults | null | undefined): string {
  if (!results?.results?.bindings || !results?.head?.vars) return '';
  const vars = results.head.vars;
  const header = vars.join(',');
  const rows = results.results.bindings.map(row =>
    vars.map(v => {
      const cell = row[v];
      if (!cell) return '';
      // Prefix formula-starting characters to prevent CSV injection in spreadsheet apps
      const rawVal = /^[=+\-@\t\r]/.test(cell.value) ? "'" + cell.value : cell.value;
      const val = rawVal.replace(/"/g, '""');
      return `"${val}"`;
    }).join(',')
  );
  return [header, ...rows].join('\n');
}

/**
 * Trigger a browser file download.
 * @param {string} content
 * @param {string} filename
 * @param {string} [mimeType='text/plain']
 */
export function downloadFile(content: string, filename: string, mimeType = 'text/plain'): void {
  const blob = new Blob([content], { type: mimeType });
  const url = URL.createObjectURL(blob);
  const a = document.createElement('a');
  a.href = url;
  a.download = filename;
  a.click();
  URL.revokeObjectURL(url);
}

/**
 * Format a large number with commas.
 * @param {number} n
 * @returns {string}
 */
export function formatNumber(n: number | undefined | null): string {
  return n?.toLocaleString() ?? '—';
}

/**
 * Detect whether an RDF file is an OWL ontology and extract key metadata.
 *
 * Performs a lightweight text-scan (no full parse) looking for:
 *   - `rdf:type owl:Ontology` / `a owl:Ontology` (Turtle / TriG)
 *   - `<owl:Ontology` (RDF/XML)
 *   - `"@type"` with `"owl:Ontology"` or the full IRI (JSON-LD)
 *
 * Also extracts `owl:versionInfo` and the ontology IRI subject when present.
 *
 * @param {string} content - raw file text
 * @param {string} filename - used as a fallback hint (.owl extension)
 * @returns {{ isOntology: boolean, ontologyIri: string|null, version: string|null }}
 */
export function detectOntologyInfo(content: string, filename = ''): { isOntology: boolean; ontologyIri: string | null; version: string | null } {
  if (!content) return { isOntology: false, ontologyIri: null, version: null };

  const lower = filename.toLowerCase();
  let isOntology = false;
  let ontologyIri = null;
  let version = null;

  // ── Detect ontology declaration ──────────────────────────────────────────

  // Turtle / TriG — `a owl:Ontology` or `rdf:type owl:Ontology`
  // Also handle full-IRI form: rdf:type <http://www.w3.org/2002/07/owl#Ontology>
  const turtleOntRe = /(?:a|rdf:type)\s+(?:owl:Ontology|<http:\/\/www\.w3\.org\/2002\/07\/owl#Ontology>)/;
  if (turtleOntRe.test(content)) {
    isOntology = true;
    // Extract the subject IRI — look for a preceding <...> on the same or previous line
    const subjectMatch = content.match(/<([^>]+)>\s*(?:\r?\n\s*)?(?:a|rdf:type)\s+(?:owl:Ontology|<http:\/\/www\.w3\.org\/2002\/07\/owl#Ontology>)/);
    if (subjectMatch) ontologyIri = subjectMatch[1];
  }

  // RDF/XML — `<owl:Ontology` or `<owl:Ontology rdf:about="..."`
  if (!isOntology) {
    const xmlOntRe = /<owl:Ontology/;
    if (xmlOntRe.test(content)) {
      isOntology = true;
      const aboutMatch = content.match(/<owl:Ontology[^>]*rdf:about="([^"]+)"/);
      if (!aboutMatch) {
        // try alternate attribute order
        const aboutMatch2 = content.match(/rdf:about="([^"]+)"[^>]*>/);
        if (aboutMatch2) ontologyIri = aboutMatch2[1];
      } else {
        ontologyIri = aboutMatch[1];
      }
    }
  }

  // JSON-LD — "@type": "owl:Ontology" or "@type": "http://www.w3.org/2002/07/owl#Ontology"
  if (!isOntology) {
    const jsonldRe = /"@type"\s*:\s*(?:"owl:Ontology"|"http:\/\/www\.w3\.org\/2002\/07\/owl#Ontology")/;
    if (jsonldRe.test(content)) {
      isOntology = true;
      // Try to extract @id near the match
      const idMatch = content.match(/"@id"\s*:\s*"([^"]+)"/);
      if (idMatch) ontologyIri = idMatch[1];
    }
  }

  // .owl extension fallback — treat as ontology even without explicit declaration
  if (!isOntology && lower.endsWith('.owl')) {
    isOntology = true;
  }

  // Model heuristic — files containing OWL/RDFS constructs but no explicit
  // owl:Ontology declaration are still model (ontology) content, not instance data.
  if (!isOntology) {
    const modelPatterns = [
      /\b(?:a|rdf:type)\s+owl:Class\b/,
      /\b(?:a|rdf:type)\s+rdfs:Class\b/,
      /\b(?:a|rdf:type)\s+owl:ObjectProperty\b/,
      /\b(?:a|rdf:type)\s+owl:DatatypeProperty\b/,
      /\b(?:a|rdf:type)\s+owl:AnnotationProperty\b/,
      /\b(?:a|rdf:type)\s+sh:NodeShape\b/,
      /\b(?:a|rdf:type)\s+sh:PropertyShape\b/,
      /\brdfs:subClassOf\b/,
      /\brdfs:subPropertyOf\b/,
      /<owl:Class\b/,
      /<rdfs:Class\b/,
      /<owl:ObjectProperty\b/,
      /<owl:DatatypeProperty\b/,
      /<sh:NodeShape\b/,
      /"(?:owl:Class|owl:ObjectProperty|owl:DatatypeProperty|owl:AnnotationProperty|rdfs:Class|sh:NodeShape|sh:PropertyShape)"/,
      /<http:\/\/www\.w3\.org\/2002\/07\/owl#(?:Class|ObjectProperty|DatatypeProperty|AnnotationProperty)>/,
      /<http:\/\/www\.w3\.org\/2000\/01\/rdf-schema#(?:Class|subClassOf|subPropertyOf)>/,
    ];
    if (modelPatterns.some((re) => re.test(content))) {
      isOntology = true;
    }
  }

  // ── Extract version ──────────────────────────────────────────────────────
  // Turtle: owl:versionInfo "1.0"
  let vm = content.match(/owl:versionInfo\s+"([^"]+)"/);
  if (vm) { version = vm[1].trim(); }
  if (!version) {
    // RDF/XML: <owl:versionInfo>1.0</owl:versionInfo>
    vm = content.match(/<owl:versionInfo[^>]*>\s*([^<]+?)\s*<\/owl:versionInfo>/);
    if (vm) version = vm[1].trim();
  }
  if (!version) {
    // JSON-LD: "owl:versionInfo": "1.0"
    vm = content.match(/"owl:versionInfo"\s*:\s*"([^"]+)"/);
    if (vm) version = vm[1].trim();
  }

  return { isOntology, ontologyIri, version };
}

// ── detectContentKindFromText ──────────────────────────────────────────────────
// Client-side, pre-upload content classification by text pattern scan.
// Used in the import wizard to show a kind badge before the file is uploaded.
//
// Returns:
//   kind: 'model' | 'vocabulary' | 'shapes' | 'entailment' | 'instances' | 'mixed' | 'unknown'
//   confidence: 'high' | 'low'
//
// 'model' is kept as an alias for 'model' for backward compatibility.
export function detectContentKindFromText(
  content: string
): { kind: 'model' | 'vocabulary' | 'shapes' | 'entailment' | 'instances' | 'mixed' | 'unknown'; confidence: 'high' | 'low' } {
  // SKOS vocabulary signals
  const skosSchemePatterns = [
    /\ba\s+skos:ConceptScheme\b/,
    /<skos:ConceptScheme\b/,
    /rdf:type\s+skos:ConceptScheme\b/,
    /<http:\/\/www\.w3\.org\/2004\/02\/skos\/core#ConceptScheme>/,
    /"@type"\s*:\s*"skos:ConceptScheme"/,
    /"@type"\s*:\s*"http:\/\/www\.w3\.org\/2004\/02\/skos\/core#ConceptScheme"/,
  ];
  const skosConceptPatterns = [
    /\ba\s+skos:Concept\b/,
    /<skos:Concept\b/,
    /<http:\/\/www\.w3\.org\/2004\/02\/skos\/core#Concept>/,
  ];

  // OWL/RDFS model signals (classes, properties — but NOT SHACL shapes)
  const owlOntologyPatterns = [
    /\ba\s+owl:Ontology\b/,
    /<owl:Ontology\b/,
    /"@type"\s*:\s*"owl:Ontology"/,
    /<http:\/\/www\.w3\.org\/2002\/07\/owl#Ontology>/,
  ];
  const owlClassPatterns = [
    /\ba\s+owl:Class\b/,
    /\ba\s+rdfs:Class\b/,
    /\ba\s+owl:ObjectProperty\b/,
    /\ba\s+owl:DatatypeProperty\b/,
    /<owl:Class\b/,
    /<rdfs:Class\b/,
    /<http:\/\/www\.w3\.org\/2002\/07\/owl#(?:Class|ObjectProperty|DatatypeProperty)>/,
  ];

  // SHACL shapes signals (distinct from OWL model content)
  const shaclShapePatterns = [
    /\ba\s+sh:NodeShape\b/,
    /\ba\s+sh:PropertyShape\b/,
    /\bsh:targetClass\b/,
    /<sh:NodeShape\b/,
    /<sh:PropertyShape\b/,
    /<http:\/\/www\.w3\.org\/ns\/shacl#(?:NodeShape|PropertyShape)>/,
  ];

  // Entailment / rule signals (SWRL, SPIN)
  const entailmentPatterns = [
    /\ba\s+swrl:Imp\b/,
    /\bsp:Rule\b/,
    /<swrl:Imp\b/,
    /<http:\/\/www\.w3\.org\/2003\/11\/swrl#Imp>/,
    /http:\/\/spinrdf\.org\/spin#rule/,
  ];

  const hasSkosScheme = skosSchemePatterns.some(re => re.test(content));
  const hasSkosConcept = skosConceptPatterns.some(re => re.test(content));
  const hasOwlOntology = owlOntologyPatterns.some(re => re.test(content));
  const hasOwlClass = owlClassPatterns.some(re => re.test(content));
  const hasShaclShape = shaclShapePatterns.some(re => re.test(content));
  const hasEntailment = entailmentPatterns.some(re => re.test(content));

  const isVocab = hasSkosScheme || hasSkosConcept;
  const isModel = hasOwlOntology || hasOwlClass;
  const isShapes = hasShaclShape && !hasOwlClass; // pure SHACL without OWL classes
  const isModelWithShapes = hasShaclShape && hasOwlClass; // SHACL + OWL model content
  const isEntailment = hasEntailment;

  // Entailment takes precedence when clearly dominant
  if (isEntailment && !isVocab && !isModel && !isShapes) {
    return { kind: 'entailment', confidence: 'high' };
  }
  // Pure SHACL (no OWL class definitions)
  if (isShapes && !isVocab && !isModel) {
    return { kind: 'shapes', confidence: 'high' };
  }
  // Pure vocabulary
  if (isVocab && !isModel && !isShapes && !isModelWithShapes) {
    return { kind: 'vocabulary', confidence: hasSkosScheme ? 'high' : 'low' };
  }
  // OWL/RDFS model content (possibly with SHACL shapes mixed in → still 'model')
  if ((isModel || isModelWithShapes) && !isVocab) {
    return { kind: 'model', confidence: hasOwlOntology ? 'high' : 'low' };
  }
  // Mixed model + vocabulary
  if ((isModel || isModelWithShapes) && isVocab) {
    return { kind: 'mixed', confidence: 'low' };
  }
  return { kind: 'unknown', confidence: 'low' };
}

export type ContentKind = 'model' | 'vocabulary' | 'shapes' | 'entailment' | 'instances' | 'mixed' | 'unknown';

// ── Graph role display + normalization ─────────────────────────────────────────
// Canonical graph-role tokens are those stored by the backend `GraphKind`:
//   'instances' | 'model' | 'vocabulary' | 'shapes' | 'entailment' | 'system'
// The UI historically used a few divergent spellings ('instance', the legacy
// 'abox'/'tbox') — these helpers fold everything onto the canonical token so a
// badge always renders with a matching label and CSS class.
export type GraphRole = 'instances' | 'model' | 'vocabulary' | 'shapes' | 'entailment' | 'system';

export const GRAPH_ROLE_LABELS: Record<GraphRole, string> = {
  instances: 'Instances',
  model: 'Model',
  vocabulary: 'Vocabulary',
  shapes: 'Shapes',
  entailment: 'Entailment',
  system: 'System',
};

// Fold any legacy / singular spelling onto the canonical role token.
export function normalizeGraphRole(role: string | null | undefined): GraphRole | null {
  if (!role) return null;
  switch (String(role).toLowerCase()) {
    case 'instance':
    case 'instances':
    case 'abox':
      return 'instances';
    case 'model':
    case 'tbox':
      return 'model';
    case 'vocabulary':
      return 'vocabulary';
    case 'shapes':
      return 'shapes';
    case 'entailment':
      return 'entailment';
    case 'system':
      return 'system';
    default:
      return null;
  }
}

// Human-readable label for a (possibly legacy-spelled) role token.
export function graphRoleLabel(role: string | null | undefined): string | null {
  const r = normalizeGraphRole(role);
  return r ? GRAPH_ROLE_LABELS[r] : null;
}

// Map a content-kind verdict (probeContentKind / detectContentKindFromText) to a
// stored graph role. 'mixed' / 'empty' / 'unknown' have no single role.
export function contentKindToRole(kind: string | null | undefined): GraphRole | null {
  return normalizeGraphRole(kind);
}

// ── detectGraphRolesFromContent ────────────────────────────────────────────────
// For quad formats (TriG / N-Quads) that carry their own named graphs, classify
// EACH embedded graph independently rather than the file as a whole. This lets
// the import wizard show one role badge per graph instead of a single "Mixed"
// verdict for a file that is really several role-typed graphs side by side.
//
// Returns a map of graph IRI → content kind. Graphs whose body yields no clear
// signal map to 'unknown'.
export function detectGraphRolesFromContent(
  filename: string,
  content: string,
): Record<string, ContentKind> {
  const lower = filename.toLowerCase();
  const roles: Record<string, ContentKind> = {};

  if (lower.endsWith('.nq') || lower.endsWith('.nquads')) {
    const byGraph: Record<string, string[]> = {};
    for (const line of content.split('\n')) {
      const trimmed = line.trim();
      if (!trimmed || trimmed.startsWith('#')) continue;
      const m = trimmed.match(/<[^>]+>\s+<[^>]+>\s+(?:<[^>]+>|"[^"]*"[^\s]*|\S+)\s+(<[^>]+>)\s*\.$/);
      if (!m) continue;
      const g = m[1].slice(1, -1);
      (byGraph[g] ||= []).push(trimmed);
    }
    for (const [g, lines] of Object.entries(byGraph)) {
      roles[g] = detectContentKindFromText(lines.join('\n')).kind;
    }
    return roles;
  }

  if (lower.endsWith('.trig')) {
    const prefixes: Record<string, string> = {};
    for (const m of content.matchAll(/@prefix\s+([a-zA-Z0-9_-]*):\s*<([^>]+)>\s*\./gi))
      prefixes[m[1]] = m[2];
    for (const m of content.matchAll(/PREFIX\s+([a-zA-Z0-9_-]*):\s*<([^>]+)>/gi))
      prefixes[m[1]] = m[2];

    // Match a graph-block header: optional GRAPH keyword, a full <iri> or a
    // prefixed name, then an opening brace. We then brace-match to find the body.
    const header = /(?:GRAPH\s+)?(?:<([^>]+)>|([a-zA-Z][a-zA-Z0-9_-]*):([\w-]*))\s*\{/gi;
    let m: RegExpExecArray | null;
    while ((m = header.exec(content)) !== null) {
      let iri: string | null = null;
      if (m[1] !== undefined) iri = m[1];
      else if (m[2] !== undefined && prefixes[m[2]] !== undefined) iri = prefixes[m[2]] + (m[3] || '');

      let depth = 1;
      let i = header.lastIndex;
      for (; i < content.length && depth > 0; i++) {
        if (content[i] === '{') depth++;
        else if (content[i] === '}') depth--;
      }
      const body = content.slice(header.lastIndex, i - 1);
      if (iri) roles[iri] = detectContentKindFromText(body).kind;
      header.lastIndex = i; // resume scanning after this block
    }
    return roles;
  }

  return roles;
}
