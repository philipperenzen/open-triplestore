<script>
  import { browseResource } from '../lib/api.js';
  import { navigate } from '../lib/router/index.js';
  import { location } from '../lib/locationStore.js';
  import { shortenIRI, graphResultsToElements } from '../lib/rdf-utils.js';
  import { safeExternalUrl, safeImageUrl } from '../lib/safeUrl.js';
  import RdfTerm from '../components/RdfTerm.svelte';
  // GraphCanvas (cytoscape) loads lazily when the graph tab opens (graphCanvasMod
  // below), so it no longer rides into the main bundle via this eagerly-imported
  // page — cytoscape is off the initial/landing download.
  import ValueRenderer from '../components/ontology/ValueRenderer.svelte';
  import TermDefinitionCard from '../components/ontology/TermDefinitionCard.svelte';
  import { lookupTerm } from '../lib/ontology/termDictionary.js';
  // GeoPreview pulls in leaflet; load it lazily only when the resource actually
  // has geometry (geoPreviewMod below) so leaflet stays out of the main bundle.
  import FileViewer from '../components/viewer/FileViewer.svelte';
  import { modelFormatFromUrl, fileResourceKind, isGeometryPredicate, isIfcGuidPredicate, FORMAT_LABELS } from '../lib/viewer/detect';

  // Model3D pulls the heavy three.js chunk; this page is in the main bundle,
  // so load the viewer only when the resource actually has a 3D model.
  const model3d = () => import('../components/viewer/Model3D.svelte');

  // GeoSPARQL/OMG geometry often hangs off a *named* node
  // (wb:Boog geo:hasGeometry wb:geom-boog; wb:geom-boog geo:asWKT "...").
  // browseResource only inlines blank nodes, so follow hasGeometry objects one
  // hop and harvest their WKT + model-file values too.
  let hopWkts = [];
  let hopModels = [];
  async function followGeometryHops(rows) {
    // Snapshot the IRI: the router reuses this component across navigations,
    // so a slow response for the previous resource must not overwrite state.
    const forIri = iri;
    hopWkts = [];
    hopModels = [];
    const targets = rows
      .filter((r) => isGeometryPredicate(r.p?.value))
      .map((r) => r.o)
      .filter((o) => o?.type === 'uri' || o?.type === 'iri')
      .map((o) => o.value)
      .slice(0, 4);
    if (!targets.length) return;
    const results = await Promise.all(
      targets.map((t) => browseResource(t, graphScope || undefined).catch(() => null))
    );
    if (forIri !== iri) return; // navigated away while loading
    const wkts = [];
    const models = [];
    const scanRow = (r) => {
      if (isWktLiteral(r.o)) wkts.push(r.o.value);
      const v = r.o?.value;
      if (typeof v === 'string') {
        const format = modelFormatFromUrl(v);
        if (format) models.push({ id: v, label: shortenIRI(r.p?.value || ''), url: v, format });
      }
    };
    for (const res of results) {
      if (!res) continue;
      for (const r of res.outgoing || []) scanRow(r);
      for (const rows2 of Object.values(res.bnodes || {})) for (const r of rows2 || []) scanRow(r);
    }
    hopWkts = wkts;
    hopModels = models;
  }
  $: followGeometryHops(outgoing);

  $: allWkts = [...featuredWkts, ...hopWkts.filter((w) => !featuredWkts.includes(w))];
  $: allModels = [
    ...featuredModels,
    ...hopModels.filter((m) => !featuredModels.some((f) => f.url === m.url)),
  ];

  // Real-world footprint for the map's "to scale" toggle, measured from the
  // resource's own 3D model (0 hides the toggle - no fabricated sizes).
  let modelMeters = 0;
  async function measureModel(models) {
    const forIri = iri; // see followGeometryHops: guard against stale loads
    modelMeters = 0;
    if (!models.length) return;
    try {
      const { loadModel, realWorldMeters } = await import('../lib/viewer/models');
      const group = await loadModel(models[0].url, models[0].format, { upAxis: models[0].upAxis });
      if (forIri !== iri) return;
      modelMeters = Math.round(realWorldMeters(group, 0));
    } catch {
      if (forIri !== iri) return;
      modelMeters = 0;
    }
  }
  $: measureModel(allModels);
  import Select from '../components/Select.svelte';
  import { t as i18nT } from 'svelte-i18n';
  import {
    ArrowLeft, ArrowRight, ArrowDownLeft,
    BookOpen, Tag, Layers, Copy, Check, MapPin, Image as ImageIcon, Link2, Info,
    Search, X, ArrowDownUp, LayoutGrid, List, Network, Table2, ChevronDown, ChevronRight, Share2,
    Hash, CalendarClock, Boxes, FileText,
  } from 'lucide-svelte';
  import { tick } from 'svelte';
  import { detectValueKind, datatypeLabel } from '../lib/ontology/valueType.js';
  import { prefixForNamespace, lookupNamespacePrefix } from '../lib/ontology/prefixService.js';
  import { copyToClipboard } from '../lib/clipboard.js';

  const isIri = (term) => term && (term.type === 'uri' || term.type === 'iri');
  const langOf = (term) => term && (term['xml:lang'] || term.language || term.lang || '');

  // Well-known predicates for image/geo/link surfacing in Overview
  const GEO_WKT_DATATYPE = 'http://www.opengis.net/ont/geosparql#wktLiteral';
  // Any WKT geometry literal, with optional CRS prefix / Z·M flag / EMPTY form.
  const WKT_RE = /^\s*(?:<[^>]*>\s*)?(?:POINT|MULTIPOINT|LINESTRING|MULTILINESTRING|POLYGON|MULTIPOLYGON|GEOMETRYCOLLECTION|TRIANGLE|TIN|POLYHEDRALSURFACE|CIRCULARSTRING|CURVEPOLYGON)\s*(?:Z|M|ZM)?\s*(?:\(|EMPTY)/i;
  const IMAGE_PREDS = new Set([
    'http://xmlns.com/foaf/0.1/depiction',
    'http://xmlns.com/foaf/0.1/img',
    'http://schema.org/image',
    'http://www.w3.org/ns/dcat#thumbnail',
  ]);
  const SEE_ALSO_PREDS = new Set([
    'http://www.w3.org/2000/01/rdf-schema#seeAlso',
    'http://xmlns.com/foaf/0.1/homepage',
    'http://xmlns.com/foaf/0.1/page',
    'http://schema.org/url',
  ]);
  const WGS84_LAT = 'http://www.w3.org/2003/01/geo/wgs84_pos#lat';
  const WGS84_LONG = 'http://www.w3.org/2003/01/geo/wgs84_pos#long';
  const SCHEMA_LAT = 'http://schema.org/latitude';
  const SCHEMA_LONG = 'http://schema.org/longitude';

  // Well-known predicates for the "definitions / labels" block.
  const LABEL_PREDS = new Set([
    'http://www.w3.org/2000/01/rdf-schema#label',
    'http://www.w3.org/2004/02/skos/core#prefLabel',
    'http://purl.org/dc/terms/title',
    'http://purl.org/dc/elements/1.1/title',
  ]);
  const ALT_LABEL_PREDS = new Set([
    'http://www.w3.org/2004/02/skos/core#altLabel',
    'http://www.w3.org/2004/02/skos/core#hiddenLabel',
  ]);
  const DEFINITION_PREDS = new Set([
    'http://www.w3.org/2004/02/skos/core#definition',
    'http://www.w3.org/2000/01/rdf-schema#comment',
    'http://purl.org/dc/terms/description',
    'http://purl.org/dc/elements/1.1/description',
    'http://www.w3.org/2004/02/skos/core#scopeNote',
    'http://www.w3.org/2004/02/skos/core#note',
  ]);
  const RDF_TYPE = 'http://www.w3.org/1999/02/22-rdf-syntax-ns#type';

  // Predicates already surfaced by their own Overview card — excluded from the
  // generic "Key facts / Quantities / Timeline" derivation so nothing duplicates.
  const SHOWN_ELSEWHERE = new Set([
    ...LABEL_PREDS, ...ALT_LABEL_PREDS, ...DEFINITION_PREDS,
    ...IMAGE_PREDS, ...SEE_ALSO_PREDS,
    WGS84_LAT, WGS84_LONG, SCHEMA_LAT, SCHEMA_LONG,
  ]);

  let iri = '';
  let graphScope = '';

  // Bundled vocabulary definition for this IRI (DCAT/OWL/SKOS/…/OTS) — shown as a
  // prominent card when the resource is a known linked-data term, even if the
  // user's own data does not carry the vocabulary triples. Null otherwise.
  let vocabMeta = null;
  async function loadVocabMeta(i) {
    vocabMeta = null;
    if (!i || i.startsWith('_:')) return;
    const m = await lookupTerm(i);
    if (i === iri) vocabMeta = m;
  }
  $: loadVocabMeta(iri);
  let data = null;
  let error = '';
  let loading = false;
  // When the ?iri= is actually a file (a samples/asset path or an http(s) URL
  // ending in a known extension), we render a FileViewer instead of resolving it
  // as an RDF resource — browseResource would otherwise throw "IRI must be
  // absolute" for a site-relative path like "/samples/x.city.json".
  let fileResource = null;
  let graphElements = { nodes: [], edges: [] };
  let types = [];
  let showLdMenu = false;
  let showTbMenu = false;
  let copied = false;
  let activeTab = 'overview'; // overview | properties | linkedFrom | graph

  // Lazily import GraphCanvas the first time the graph tab opens (memoised).
  let graphCanvasMod;
  $: if (activeTab === 'graph' && !graphCanvasMod) graphCanvasMod = import('../components/GraphCanvas.svelte');
  // GeoPreview (leaflet) only when the resource carries geometry.
  let geoPreviewMod;
  $: if (allWkts.length > 0 && !geoPreviewMod) geoPreviewMod = import('../components/GeoPreview.svelte');

  // Properties tab controls
  let propSearch = '';
  let propSort = 'pred-asc'; // pred-asc | pred-desc | val-asc | val-desc | type
  let propGroup = true;
  let propCollapsed = new Set(); // vocabulary keys collapsed in Properties

  // Linked-from tab controls
  let incomingSearch = '';
  let incomingSort = 'count-desc'; // count-desc | count-asc | name-asc
  let incomingGroup = true;
  let incomingCollapsed = new Set(); // vocabulary keys collapsed in Linked from

  // Transient highlight target after clicking a summary stat ('types' | 'definitions' | '')
  let flash = '';

  $: localResourcePath = (() => {
    if (typeof window === 'undefined') return null;
    const prefix = window.location.origin + '/resource/';
    return iri.startsWith(prefix) ? iri.slice(prefix.length) : null;
  })();

  $: {
    const params = new URLSearchParams($location.search);
    const nextIri = params.get('iri') || '';
    const nextGraph = params.get('graph') || '';
    if (nextIri !== iri || nextGraph !== graphScope) {
      iri = nextIri;
      graphScope = nextGraph;
      if (iri) fetchResource();
    }
  }

  async function fetchResource() {
    loading = true;
    error = '';
    data = null;
    graphElements = { nodes: [], edges: [] };
    types = [];
    // Reset per-resource view state so controls don't carry over between IRIs.
    propSearch = '';
    incomingSearch = '';
    propCollapsed = new Set();
    incomingCollapsed = new Set();
    flash = '';
    activeTab = 'overview';
    // A file (relative samples/asset path or an http(s) URL with a known
    // extension) is rendered by FileViewer — never sent to browseResource, which
    // rejects non-absolute IRIs ("IRI must be absolute").
    fileResource = fileResourceKind(iri);
    if (fileResource) {
      loading = false;
      return;
    }
    try {
      data = await browseResource(iri, graphScope || undefined);
      buildGraph();
      extractTypes();
    } catch (e) {
      error = e.message;
    } finally {
      loading = false;
    }
  }

  function buildGraph() {
    if (!data) return;
    const bindings = [];
    for (const row of (data.outgoing || [])) {
      bindings.push({ s: { type: 'uri', value: iri }, p: row.p, o: row.o });
    }
    for (const row of (data.incoming || [])) {
      bindings.push({ s: row.s, p: row.p, o: { type: 'uri', value: iri } });
    }
    // Include blank-node descriptions so geometry/address nodes and their
    // literals show up in the neighbourhood graph instead of dead-ending.
    for (const [id, rows] of Object.entries(data.bnodes || {})) {
      for (const row of (rows || [])) {
        bindings.push({ s: { type: 'bnode', value: id }, p: row.p, o: row.o });
      }
    }
    graphElements = graphResultsToElements(bindings, 's', 'p', 'o', 120);
  }

  // A canonical identity for an RDF term, used for de-duplication. The backend
  // unions FROM + FROM NAMED across every accessible graph, so a triple that
  // lives in more than one graph (or in both the default + named projection)
  // comes back repeated. We collapse those here.
  function termKey(term) {
    if (!term) return '∅';
    return [term.type || '', term.value || '', langOf(term), term.datatype || ''].join('');
  }
  function dedupeBy(rows, keyFn) {
    const seen = new Set();
    const out = [];
    for (const r of rows) {
      const k = keyFn(r);
      if (seen.has(k)) continue;
      seen.add(k);
      out.push(r);
    }
    return out;
  }

  function extractTypes() {
    const seen = new Set();
    const out = [];
    for (const r of (data?.outgoing || [])) {
      if (r.p?.value === RDF_TYPE && isIri(r.o) && !seen.has(r.o.value)) {
        seen.add(r.o.value);
        out.push(r.o.value);
      }
    }
    types = out;
  }

  function viewResource(newIri) {
    const qs = new URLSearchParams({ iri: newIri });
    if (graphScope) qs.set('graph', graphScope);
    navigate(`/resource?${qs.toString()}`);
  }

  function goBack() {
    if (typeof window !== 'undefined' && window.history.length > 1) {
      window.history.back();
    } else {
      navigate('/browse');
    }
  }

  // Open this resource in the Triple Browser, set as the exact-match subject,
  // in the requested view. 'table' is the default view (no view param).
  function openInBrowser(view) {
    showTbMenu = false;
    const qs = new URLSearchParams();
    qs.set('subject', iri); // Triple Browser defaults to exact-IRI match mode.
    if (view && view !== 'table') qs.set('view', view); // 'graph'
    if (graphScope) qs.set('graph', graphScope);
    navigate(`/browse?${qs.toString()}`);
  }

  async function copyIri() {
    if (await copyToClipboard(iri)) {
      copied = true;
      setTimeout(() => (copied = false), 1500);
    }
  }

  // Svelte action: invoke callback on a click outside the node.
  function clickOutside(node, callback) {
    const handle = (e) => { if (!node.contains(e.target)) callback(); };
    document.addEventListener('click', handle, true);
    return { destroy() { document.removeEventListener('click', handle, true); } };
  }

  function nsOf(uri) {
    if (!uri) return '';
    const hash = uri.lastIndexOf('#');
    if (hash !== -1) return uri.slice(0, hash + 1);
    const slash = uri.lastIndexOf('/');
    if (slash !== -1) return uri.slice(0, slash + 1);
    return uri;
  }

  // Open a displayed SPARQL literal in the SPARQL editor, prefilled. The editor
  // reads this hand-off key on mount — the same path saved queries use.
  function onRunSparql(e) {
    const q = e?.detail?.query || '';
    if (!q) return;
    try { sessionStorage.setItem('ots_sparql_load', q); } catch {}
    navigate('/sparql');
  }

  // Collapsible groups — shared accordion behaviour across Properties + Linked from.
  function toggleProp(key) {
    const n = new Set(propCollapsed);
    n.has(key) ? n.delete(key) : n.add(key);
    propCollapsed = n;
  }
  function toggleIncoming(key) {
    const n = new Set(incomingCollapsed);
    n.has(key) ? n.delete(key) : n.add(key);
    incomingCollapsed = n;
  }
  function togglePropAll() {
    propCollapsed = propAllCollapsed ? new Set() : new Set(propertyGroups.map(g => g.ns));
  }
  function toggleIncomingAll() {
    incomingCollapsed = incomingAllCollapsed ? new Set() : new Set(incomingGroups.map(g => g.ns));
  }

  // Summary stat → reveal the underlying data (types live in the header, definitions in Overview).
  async function reveal(target) {
    if (target === 'definitions') activeTab = 'overview';
    await tick();
    const el = typeof document !== 'undefined' && document.getElementById(`${target}-anchor`);
    if (el) el.scrollIntoView({ behavior: 'smooth', block: 'center' });
    flash = target;
    setTimeout(() => { if (flash === target) flash = ''; }, 1500);
  }

  // ── Overview summary formatting helpers ──────────────────────────────────
  const fmtNum = (v) => { const n = Number(v); return Number.isFinite(n) ? n.toLocaleString() : String(v ?? ''); };
  const parseTime = (v) => { const d = new Date(v); return isNaN(d.getTime()) ? Number.parseInt(String(v).slice(0, 4), 10) || 0 : d.getTime(); };
  function fmtDate(v) {
    const s = String(v ?? '');
    if (/^-?\d{4}(-\d{2})?$/.test(s) || s.startsWith('--')) return s; // gYear / gYearMonth / gMonthDay
    if (/^\d{4}-\d{2}-\d{2}T/.test(s)) return s.slice(0, 10); // drop time-of-day in the timeline
    return s;
  }
  const unitLabel = (dt) => datatypeLabel(dt).replace(/^xsd:/, '');

  // Vocabulary prefix pills — same hash-colour + prefix scheme as the facet rail,
  // so a vocabulary's colour matches it across the app.
  function strHue(str) { let h = 0; for (let i = 0; i < str.length; i++) h = (h * 31 + str.charCodeAt(i)) & 0xffffffff; return Math.abs(h) % 360; }
  const nsColor = (ns) => `hsl(${strHue(ns)},52%,38%)`;
  const nsBg = (ns) => `hsl(${strHue(ns)},65%,94%)`;
  const PREFIX_BASES = [
    { prefix: 'ex',  bases: ['https://example.org/', 'http://example.org/'] },
    { prefix: 'ots', bases: ['https://opentriplestore.org/', 'http://opentriplestore.org/'] },
  ];
  const basePrefix = (ns) => PREFIX_BASES.find(({ bases }) => bases.some(b => ns.startsWith(b)))?.prefix || null;
  const deriveLabel = (ns) => ns.replace(/[#/]+$/, '').split(/[#/]/).filter(Boolean).pop() || ns;
  const rawPrefix = (ns) => prefixForNamespace(ns) || basePrefix(ns) || deriveLabel(ns);

  // -------- derived views (de-duplicated) --------
  $: outgoing = dedupeBy(data?.outgoing || [], r => `${termKey(r.p)}${termKey(r.o)}`);
  $: incoming = dedupeBy(data?.incoming || [], r => `${termKey(r.s)}${termKey(r.p)}`);
  // A high-in-degree hub is capped server-side at 500 rows; show "500+" rather than
  // implying the truncated set is the true count.
  $: incomingLabel = data?.incoming_truncated ? `${incoming.length}+` : `${incoming.length}`;

  // Blank-node closure: id → [{p, o}]. Lets values that are blank nodes (e.g. a
  // geometry behind geo:hasGeometry) expand inline instead of dead-ending.
  $: bnodes = data?.bnodes || {};

  $: labels = outgoing.filter(r =>
    LABEL_PREDS.has(r.p?.value) && r.o?.type === 'literal');
  $: altLabels = outgoing.filter(r =>
    ALT_LABEL_PREDS.has(r.p?.value) && r.o?.type === 'literal');
  $: definitions = outgoing.filter(r =>
    DEFINITION_PREDS.has(r.p?.value) && r.o?.type === 'literal');

  // Highlighted label candidate for header: prefer English/no-lang first, else first.
  $: primaryLabel = (() => {
    if (labels.length === 0) return '';
    const en = labels.find(r => {
      const l = langOf(r.o);
      return !l || l.toLowerCase().startsWith('en');
    });
    return (en || labels[0])?.o?.value || '';
  })();

  // ── Featured visuals for Overview ────────────────────────────────────────
  // A WKT literal, identified by datatype or by its leading geometry keyword.
  const isWktLiteral = (t) =>
    t?.type === 'literal' && (t.datatype === GEO_WKT_DATATYPE || WKT_RE.test(t.value || ''));

  // 3D model / BIM references on this resource (or its blank-node closure):
  // any object value that is a loadable model URL (glb/gltf/stl — the FOG
  // pattern omg:hasGeometry/fog:asGltf…), plus an IFC GlobalId if present.
  $: featuredModels = (() => {
    const seen = new Set();
    const out = [];
    const scan = (r) => {
      const v = r.o?.value;
      if (typeof v !== 'string' || seen.has(v)) return;
      const format = modelFormatFromUrl(v);
      if (format) {
        seen.add(v);
        out.push({ id: v, label: shortenIRI(r.p?.value || ''), url: v, format });
      }
    };
    for (const r of outgoing) scan(r);
    for (const rows of Object.values(bnodes)) for (const r of (rows || [])) scan(r);
    return out;
  })();

  $: featuredIfcGuid = (() => {
    const isGuidPred = (r) => isIfcGuidPredicate(r.p?.value);
    for (const r of outgoing) if (isGuidPred(r)) return r.o?.value;
    for (const rows of Object.values(bnodes)) for (const r of (rows || [])) if (isGuidPred(r)) return r.o?.value;
    return null;
  })();

  $: featuredWkts = (() => {
    const seen = new Set();
    const out = [];
    const addWkt = (val) => { if (val && !seen.has(val)) { seen.add(val); out.push(val); } };

    // Direct WKT literals on the resource itself…
    for (const r of outgoing) if (isWktLiteral(r.o)) addWkt(r.o.value);
    // …and WKT literals carried by any blank node in this resource's closure,
    // which is the common GeoSPARQL shape: geo:hasGeometry [ geo:asWKT "…" ].
    for (const rows of Object.values(bnodes)) {
      for (const r of (rows || [])) if (isWktLiteral(r.o)) addWkt(r.o.value);
    }

    // schema:/wgs84 latitude + longitude pair, on the resource or a blank node.
    const findVal = (preds) => {
      for (const r of outgoing) if (preds.includes(r.p?.value)) return r.o?.value;
      for (const rows of Object.values(bnodes))
        for (const r of (rows || [])) if (preds.includes(r.p?.value)) return r.o?.value;
      return undefined;
    };
    const lat = findVal([SCHEMA_LAT, WGS84_LAT]);
    const lon = findVal([SCHEMA_LONG, WGS84_LONG]);
    if (lat && lon && !isNaN(parseFloat(lat)) && !isNaN(parseFloat(lon))) {
      addWkt(`POINT(${parseFloat(lon)} ${parseFloat(lat)})`);
    }
    return out;
  })();

  $: featuredImages = [...new Set(outgoing
    .filter(r => isIri(r.o) && IMAGE_PREDS.has(r.p?.value || ''))
    .map(r => r.o.value))];

  $: featuredLinks = [...new Set(outgoing
    .filter(r => isIri(r.o) && SEE_ALSO_PREDS.has(r.p?.value || ''))
    .map(r => r.o.value))];

  // ── Overview summary cards: key facts / quantities / dates / vocabularies ──
  // Literal properties not already surfaced by a dedicated card, classified by
  // value kind so each lands in the right summary.
  $: factRows = outgoing.filter(r =>
    r.o?.type === 'literal' && !SHOWN_ELSEWHERE.has(r.p?.value) && !isWktLiteral(r.o));
  $: factKinds = factRows.map(r => ({
    row: r,
    kind: detectValueKind({ type: 'literal', value: r.o.value, datatype: r.o.datatype, lang: r.o['xml:lang'] || r.o.lang }, r.p?.value || '').kind,
  }));
  $: quantities = factKinds.filter(f => f.kind === 'number').map(f => f.row);
  $: timelineDates = factKinds.filter(f => f.kind === 'date').map(f => f.row);
  $: showTimeline = timelineDates.length >= 2;
  $: timelineSorted = timelineDates
    .map(r => ({ row: r, t: parseTime(r.o.value) }))
    .sort((a, b) => a.t - b.t);
  // Everything else fact-like (bool, duration, color, lang, plain/long text…),
  // plus lone dates when there aren't enough for a timeline.
  $: keyFacts = factKinds.filter(f => {
    if (f.kind === 'number') return false;
    if (f.kind === 'date') return !showTimeline;
    return !['geo', 'image', 'sparql', 'html'].includes(f.kind);
  }).map(f => f.row);

  // Which vocabularies this resource draws on — its predicates + its classes.
  $: vocabularies = (() => {
    const m = new Map();
    const add = (iri) => { if (!iri) return; const ns = nsOf(iri); const cur = m.get(ns) || { ns, count: 0 }; cur.count += 1; m.set(ns, cur); };
    for (const r of outgoing) add(r.p?.value);
    for (const ty of types) add(ty);
    return [...m.values()].sort((a, b) => b.count - a.count);
  })();
  let _prefixTick = 0;
  $: vocabLabels = (() => {
    void _prefixTick;
    const raw = vocabularies.map(v => ({ ns: v.ns, p: rawPrefix(v.ns) }));
    const groups = {};
    for (const r of raw) (groups[r.p] ||= []).push(r.ns);
    const out = {};
    for (const r of raw) { const arr = groups[r.p]; out[r.ns] = arr.length > 1 ? `${r.p}${arr.indexOf(r.ns) + 1}` : r.p; }
    return out;
  })();
  // Warm unknown namespaces from the prefix service; re-render when they resolve.
  $: { void _prefixTick; for (const v of vocabularies) { if (!prefixForNamespace(v.ns) && !basePrefix(v.ns)) { lookupNamespacePrefix(v.ns).then(p => { if (p) _prefixTick += 1; }); } } }

  // ── Properties: filter + sort + (optional) namespace grouping ────────────
  $: filteredOutgoing = (() => {
    const q = propSearch.trim().toLowerCase();
    if (!q) return outgoing;
    return outgoing.filter(r => {
      const hay = `${r.p?.value || ''} ${shortenIRI(r.p?.value || '')} ${r.o?.value || ''}`.toLowerCase();
      return hay.includes(q);
    });
  })();

  // Note: `propSort` is referenced directly here (not hidden inside a helper
  // function) so Svelte tracks it as a dependency and re-sorts when it changes.
  $: sortedOutgoing = (() => {
    const key = propSort;
    const desc = key.endsWith('desc');
    const valueOf = (row) => {
      if (key.startsWith('val')) return (row.o?.value || '').toLowerCase();
      if (key === 'type') return row.o?.type || '';
      return (shortenIRI(row.p?.value || '') || row.p?.value || '').toLowerCase();
    };
    return [...filteredOutgoing].sort((a, b) => {
      const av = valueOf(a), bv = valueOf(b);
      let c = av < bv ? -1 : av > bv ? 1 : 0;
      if (c === 0) {
        // Stable secondary sort so equal keys keep a predictable order.
        const ao = (a.o?.value || ''), bo = (b.o?.value || '');
        c = ao < bo ? -1 : ao > bo ? 1 : 0;
      }
      return desc ? -c : c;
    });
  })();

  $: propertyGroups = (() => {
    if (!propGroup) return [];
    const map = new Map();
    for (const row of sortedOutgoing) {
      const ns = nsOf(row.p?.value);
      if (!map.has(ns)) map.set(ns, []);
      map.get(ns).push(row);
    }
    return Array.from(map.entries())
      .map(([ns, rows]) => ({ ns, rows }))
      .sort((a, b) => a.ns.localeCompare(b.ns));
  })();

  // ── Linked from: filter + sort ───────────────────────────────────────────
  $: filteredIncoming = (() => {
    const q = incomingSearch.trim().toLowerCase();
    if (!q) return incoming;
    return incoming.filter(r => {
      const hay = `${r.s?.value || ''} ${shortenIRI(r.s?.value || '')} ${r.p?.value || ''} ${shortenIRI(r.p?.value || '')}`.toLowerCase();
      return hay.includes(q);
    });
  })();

  // Sort incoming rows by predicate then subject so same-predicate links cluster.
  const byPredThenSubject = (a, b) => {
    const pa = shortenIRI(a.p?.value || ''), pb = shortenIRI(b.p?.value || '');
    if (pa !== pb) return pa.localeCompare(pb);
    return (a.s?.value || '').localeCompare(b.s?.value || '');
  };

  // Linked from grouped by vocabulary (predicate namespace) — mirrors Properties.
  $: incomingGroups = (() => {
    const map = new Map();
    for (const row of filteredIncoming) {
      const ns = nsOf(row.p?.value);
      if (!map.has(ns)) map.set(ns, []);
      map.get(ns).push(row);
    }
    const arr = Array.from(map.entries())
      .map(([ns, rows]) => ({ ns, rows: rows.slice().sort(byPredThenSubject) }));
    if (incomingSort === 'count-asc') arr.sort((a, b) => a.rows.length - b.rows.length);
    else if (incomingSort === 'name-asc') arr.sort((a, b) => (shortenIRI(a.ns) || a.ns).localeCompare(shortenIRI(b.ns) || b.ns));
    else arr.sort((a, b) => b.rows.length - a.rows.length); // count-desc (default)
    return arr;
  })();

  // Flat (ungrouped) incoming list, sorted the same way.
  $: flatIncoming = filteredIncoming.slice().sort(byPredThenSubject);

  $: propAllCollapsed = propertyGroups.length > 0 && propertyGroups.every(g => propCollapsed.has(g.ns));
  $: incomingAllCollapsed = incomingGroups.length > 0 && incomingGroups.every(g => incomingCollapsed.has(g.ns));
</script>

<div class="resource-page">
  <!-- Breadcrumb / back -->
  <div class="crumbs">
    <button class="crumb-btn" on:click={goBack} title={$i18nT('pages.resource.goBack')}>
      <ArrowLeft size={14} /> {$i18nT('system.back')}
    </button>

    {#if graphScope}
      <span class="crumb-sep">/</span>
      <span class="crumb-scope" title={graphScope}>
        <Layers size={12} /> {shortenIRI(graphScope)}
      </span>
    {/if}
  </div>

  {#if fileResource}
    <!-- File resource: a samples/asset path or a file URL — render the file
         directly instead of resolving it as an RDF resource. -->
    <div class="card file-header">
      <div class="file-title">
        <FileText size={16} />
        <h2>{shortenIRI(iri)}</h2>
      </div>
      <div class="iri-full">
        <span class="truncate">{iri}</span>
        <button class="icon-btn" on:click={copyIri} title={$i18nT('pages.resource.copyIri')}>
          {#if copied}<Check size={12} />{:else}<Copy size={12} />{/if}
        </button>
      </div>
    </div>
    <div class="card">
      <FileViewer url={iri} height="360px" />
    </div>
  {:else}

  <!-- Header -->
  <div class="card resource-header">
    <div class="header-top">
      <div class="min-w-0">
        {#if primaryLabel}
          <h2>{primaryLabel}</h2>
          <div class="short-iri">{shortenIRI(iri)}</div>
        {:else}
          <h2>{shortenIRI(iri)}</h2>
        {/if}
        <div class="iri-full">
          <span class="truncate">{iri}</span>
          <button class="icon-btn" on:click={copyIri} title={$i18nT('pages.resource.copyIri')}>
            {#if copied}<Check size={12} />{:else}<Copy size={12} />{/if}
          </button>
        </div>
      </div>
      <div class="header-actions">
        <div class="tb-menu-wrap" use:clickOutside={() => (showTbMenu = false)}>
          <button class="btn btn-sm btn-ghost" on:click|stopPropagation={() => (showTbMenu = !showTbMenu)} title={$i18nT('pages.resource.openInTripleBrowserExact')}>
            <Share2 size={14} /> {$i18nT('pages.resource.tripleBrowser')} <ChevronDown size={12} />
          </button>
          {#if showTbMenu}
            <div class="tb-dropdown">
              <div class="tb-dropdown-head">{$i18nT('pages.resource.viewAsExactMatch')}</div>
              <button on:click={() => openInBrowser('table')}><Table2 size={14} /> {$i18nT('pages.resource.tableView')}</button>
              <button on:click={() => openInBrowser('graph')}><Network size={14} /> {$i18nT('pages.resource.graphView')}</button>
            </div>
          {/if}
        </div>
        {#if localResourcePath !== null}
          <!-- svelte-ignore a11y-no-static-element-interactions -->
          <div class="ld-menu-wrap" on:mouseleave={() => (showLdMenu = false)}>
            <button class="btn btn-sm btn-ghost" on:click={() => (showLdMenu = !showLdMenu)}
              title={$i18nT('pages.resource.viewRawLinkedData')}>&lt;/&gt; {$i18nT('pages.resource.linkedData')} ▾</button>
            {#if showLdMenu}
              <div class="ld-dropdown">
                <a href="/resource/{localResourcePath}?format=turtle" target="_blank" rel="noopener">Turtle</a>
                <a href="/resource/{localResourcePath}?format=jsonld" target="_blank" rel="noopener">JSON-LD</a>
                <a href="/resource/{localResourcePath}?format=ntriples" target="_blank" rel="noopener">N-Triples</a>
                <a href="/resource/{localResourcePath}?format=rdfxml" target="_blank" rel="noopener">RDF/XML</a>
              </div>
            {/if}
          </div>
        {/if}
      </div>
    </div>

    {#if types.length > 0}
      <div class="type-badges" id="types-anchor" class:flash={flash === 'types'}>
        <span class="badge-label">{types.length === 1 ? $i18nT('pages.resource.type') : $i18nT('pages.resource.types')}</span>
        {#each types as ty}
          <button class="type-badge" on:click={() => viewResource(ty)} title={ty}>
            <Tag size={11} /> {shortenIRI(ty)}
          </button>
        {/each}
      </div>
    {/if}

    <!-- Summary stats. While the fetch is in flight `data` is null, so these
         counts are all 0 — printing "0 properties / 0 linked from …" directly above
         a "Loading resource…" body reads as broken. Show a shimmer until it lands. -->
    <div class="stat-row">
      <button class="stat" class:stat-clickable={!loading && outgoing.length > 0} on:click={() => outgoing.length && (activeTab = 'properties')}>
        <ArrowRight size={14} />
        {#if loading}<span class="skel stat-skel"></span>{:else}<span class="stat-num">{outgoing.length}</span>{/if}
        <span class="stat-label">{$i18nT('pages.resource.properties')}</span>
      </button>
      <button class="stat" class:stat-clickable={!loading && incoming.length > 0} on:click={() => incoming.length && (activeTab = 'linkedFrom')}>
        <ArrowDownLeft size={14} />
        {#if loading}<span class="skel stat-skel"></span>{:else}<span class="stat-num">{incomingLabel}</span>{/if}
        <span class="stat-label">{$i18nT('pages.resource.linkedFrom')}</span>
      </button>
      <button class="stat" class:stat-clickable={!loading && types.length > 0} on:click={() => types.length && reveal('types')}>
        <Tag size={14} />
        {#if loading}<span class="skel stat-skel"></span>{:else}<span class="stat-num">{types.length}</span>{/if}
        <span class="stat-label">{$i18nT('pages.resource.types')}</span>
      </button>
      <button class="stat" class:stat-clickable={!loading && definitions.length > 0} on:click={() => definitions.length && reveal('definitions')}>
        <BookOpen size={14} />
        {#if loading}<span class="skel stat-skel"></span>{:else}<span class="stat-num">{definitions.length}</span>{/if}
        <span class="stat-label">{$i18nT('pages.resource.definitions')}</span>
      </button>
    </div>
  </div>

  {#if error}<p class="error">{error}</p>{/if}

  {#if loading}
    <div class="card"><p class="loading-text">{$i18nT('pages.resource.loadingResource')}</p></div>
  {:else if data}
    {#if vocabMeta}
      <div class="card vocab-def-card">
        <h3 class="vocab-def-title">{$i18nT('pages.resource.vocabularyDefinition')}</h3>
        <TermDefinitionCard {iri} meta={vocabMeta} variant="rich" />
      </div>
    {/if}
    <!-- Tabs -->
    <div class="tabs">
      <button class="tab" class:tab-active={activeTab === 'overview'} on:click={() => (activeTab = 'overview')}>
        <Info size={14} /> {$i18nT('pages.resource.overview')}
      </button>
      <button class="tab" class:tab-active={activeTab === 'properties'} on:click={() => (activeTab = 'properties')}>
        <ArrowRight size={14} /> {$i18nT('pages.resource.properties')} <span class="tab-count">{outgoing.length}</span>
      </button>
      <button class="tab" class:tab-active={activeTab === 'linkedFrom'} on:click={() => (activeTab = 'linkedFrom')}>
        <ArrowDownLeft size={14} /> {$i18nT('pages.resource.linkedFrom')} <span class="tab-count">{incoming.length}</span>
      </button>
      <button class="tab" class:tab-active={activeTab === 'graph'} on:click={() => (activeTab = 'graph')}>
        <Network size={14} /> {$i18nT('pages.resource.graph')}
      </button>
    </div>

    {#if activeTab === 'graph'}
      <div class="card">
        <p class="tab-hint">
          <Network size={12} /> {$i18nT('pages.resource.graphNeighbourhood')} — {$i18nT('pages.resource.graphHint')}
        </p>
        {#if graphElements.nodes.length > 0}
          {#await graphCanvasMod then GC}
            {#if GC}
              <svelte:component this={GC.default}
                nodes={graphElements.nodes}
                edges={graphElements.edges}
                height="460px"
                on:nodeOpen={(e) => e.detail.fullIri && e.detail.fullIri !== iri && viewResource(e.detail.fullIri)}
              />
            {/if}
          {/await}
        {:else}
          <p class="muted empty-inline">{$i18nT('pages.resource.noGraphNeighbourhood')}</p>
        {/if}
      </div>
    {/if}

    {#if activeTab === 'overview'}
      {#if data?.reason === 'no_accessible_graphs'}
        <div class="card empty-state">
          <Info size={20} />
          <div>
            <strong>{$i18nT('pages.resource.noAccessibleGraphs')}</strong>
            <p class="muted">{$i18nT('pages.resource.noAccessibleGraphsDesc')}</p>
          </div>
        </div>
      {:else if outgoing.length === 0 && incoming.length === 0}
        <div class="card empty-state">
          <Info size={20} />
          <div>
            <strong>{$i18nT('pages.resource.iriNotInAnyGraph')}</strong>
            <p class="muted">{$i18nT('pages.resource.iriNotInAnyGraphDesc')}</p>
            <button class="btn btn-sm btn-ghost" on:click={() => openInBrowser('table')}>{$i18nT('pages.resource.openInTripleBrowser')}</button>
          </div>
        </div>
      {/if}

      <!-- Featured visuals (geo / images / links) -->
      {#if allWkts.length > 0}
        <div class="card">
          <h3><MapPin size={14} /> {$i18nT('pages.resource.geometry')}</h3>
          {#await geoPreviewMod then GP}
            {#if GP}
              <svelte:component this={GP.default} wkts={allWkts} scaleMeters={modelMeters} />
            {/if}
          {/await}
        </div>
      {/if}

      {#if allModels.length > 0}
        <div class="card">
          <h3><Boxes size={14} /> {$i18nT('viewer.model3dBim')}</h3>
          {#await model3d() then mod}
            <svelte:component this={mod.default} refs={[allModels[0]]} height="260px" />
          {/await}
          <div class="bim-facts">
            {#if featuredIfcGuid}
              <span class="bim-fact" title="IFC GlobalId">IFC GlobalId <code>{featuredIfcGuid}</code></span>
            {/if}
            {#each allModels as m}
              <a class="bim-fact" href={m.url} target="_blank" rel="noopener" title={m.url}>
                {FORMAT_LABELS[m.format]} ↗
              </a>
            {/each}
          </div>
        </div>
      {/if}

      {#if featuredImages.length > 0}
        <div class="card">
          <h3><ImageIcon size={14} /> {$i18nT('pages.resource.images')}</h3>
          <div class="img-strip">
            {#each featuredImages as src}
              {@const safeSrc = safeImageUrl(src)}
              <a href={safeSrc} target="_blank" rel="noopener" title={src}>
                <img src={safeSrc} alt="" loading="lazy" on:error={(e) => { /** @type {HTMLElement} */ (e.currentTarget).style.display = 'none'; }} />
              </a>
            {/each}
          </div>
        </div>
      {/if}

      {#if featuredLinks.length > 0}
        <div class="card">
          <h3><Link2 size={14} /> {$i18nT('pages.resource.links')}</h3>
          <div class="link-chips">
            {#each featuredLinks as href}
              <a class="link-chip" href={safeExternalUrl(href)} target="_blank" rel="noopener" title={href}>
                {shortenIRI(href)}
              </a>
            {/each}
          </div>
        </div>
      {/if}

      <!-- Labels -->
      {#if labels.length > 0 || altLabels.length > 0}
        <div class="card">
          <h3><Tag size={14} /> {$i18nT('pages.resource.labels')}</h3>
          {#if labels.length > 0}
            <div class="label-list">
              {#each labels as row}
                <div class="label-item">
                  <span class="lit">{row.o.value}</span>
                  {#if langOf(row.o)}<span class="lang-tag">@{langOf(row.o)}</span>{/if}
                  <span class="pred-hint">{shortenIRI(row.p.value)}</span>
                </div>
              {/each}
            </div>
          {/if}
          {#if altLabels.length > 0}
            <div class="label-list alt">
              <div class="pred-hint" style="margin-right:0.4rem">{$i18nT('pages.resource.altLabel')}</div>
              {#each altLabels as row}
                <span class="alt-chip">
                  {row.o.value}{#if langOf(row.o)} <em>@{langOf(row.o)}</em>{/if}
                </span>
              {/each}
            </div>
          {/if}
        </div>
      {/if}

      <!-- Definitions -->
      {#if definitions.length > 0}
        <div class="card" id="definitions-anchor" class:flash={flash === 'definitions'}>
          <h3><BookOpen size={14} /> {$i18nT('pages.resource.definitionsAndNotes')}</h3>
          <div class="def-list">
            {#each definitions as row}
              <div class="def-item">
                <div class="def-text">
                  {row.o.value}
                  {#if langOf(row.o)}<span class="lang-tag">@{langOf(row.o)}</span>{/if}
                </div>
                <div class="def-pred" title={row.p.value}>{shortenIRI(row.p.value)}</div>
              </div>
            {/each}
          </div>
        </div>
      {/if}

      <!-- Key facts: notable scalar properties at a glance -->
      {#if keyFacts.length > 0}
        <div class="card">
          <h3><Table2 size={14} /> {$i18nT('pages.resource.keyFacts')}</h3>
          <table class="facts-table">
            <tbody>
              {#each keyFacts as row}
                <tr>
                  <td class="fact-key" title={row.p?.value}>{shortenIRI(row.p?.value || '')}</td>
                  <td class="fact-val"><ValueRenderer term={row.o} predicate={row.p?.value || ''} {bnodes} on:run-sparql={onRunSparql} /></td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
      {/if}

      <!-- Quantities: numeric / measurement highlights -->
      {#if quantities.length > 0}
        <div class="card">
          <h3><Hash size={14} /> {$i18nT('pages.resource.quantities')}</h3>
          <div class="quantity-grid">
            {#each quantities as row}
              <div class="quantity-card">
                <div class="quantity-num">{fmtNum(row.o.value)}</div>
                <div class="quantity-label" title={row.p?.value}>{shortenIRI(row.p?.value || '')}</div>
                {#if row.o.datatype}<div class="quantity-dt" title={row.o.datatype}>{unitLabel(row.o.datatype)}</div>{/if}
              </div>
            {/each}
          </div>
        </div>
      {/if}

      <!-- Timeline: the resource's date / time properties in order -->
      {#if showTimeline}
        <div class="card">
          <h3><CalendarClock size={14} /> {$i18nT('pages.resource.timeline')}</h3>
          <div class="timeline">
            {#each timelineSorted as item}
              <div class="tl-item">
                <span class="tl-date">{fmtDate(item.row.o.value)}</span>
                <span class="tl-pred" title={item.row.p?.value}>{shortenIRI(item.row.p?.value || '')}</span>
              </div>
            {/each}
          </div>
        </div>
      {/if}

      <!-- Vocabularies this resource draws on -->
      {#if vocabularies.length > 0}
        <div class="card">
          <h3><Boxes size={14} /> {$i18nT('pages.resource.vocabularies')}</h3>
          <div class="vocab-pills">
            {#each vocabularies as v}
              <span class="vocab-pill" style="color:{nsColor(v.ns)};background:{nsBg(v.ns)}" title={v.ns}>
                {vocabLabels[v.ns]}<span class="vocab-count">{v.count}</span>
              </span>
            {/each}
          </div>
        </div>
      {/if}

      <!-- Quick summary of what it links to / linked from -->
      <div class="split-grid">
        <div class="card">
          <h3><ArrowRight size={14} /> {$i18nT('pages.resource.linksTo', { values: { count: outgoing.length } })}</h3>
          {#if outgoing.length === 0}
            <p class="muted">{$i18nT('pages.resource.noOutgoingLinks')}</p>
          {:else}
            <ul class="mini-list">
              {#each outgoing.slice(0, 8) as row}
                <li>
                  <span class="predicate" title={row.p?.value}>{shortenIRI(row.p?.value || '')}</span>
                  → <ValueRenderer term={row.o} predicate={row.p?.value || ''} {bnodes} compact on:run-sparql={onRunSparql} />
                </li>
              {/each}
            </ul>
            {#if outgoing.length > 8}
              <button class="btn btn-sm btn-ghost" on:click={() => (activeTab = 'properties')}>{$i18nT('pages.resource.seeAll', { values: { count: outgoing.length } })}</button>
            {/if}
          {/if}
        </div>
        <div class="card">
          <h3><ArrowDownLeft size={14} /> {$i18nT('pages.resource.linkedFromCount', { values: { count: incoming.length } })}</h3>
          {#if incoming.length === 0}
            <p class="muted">{$i18nT('pages.resource.nothingLinksHere')}</p>
          {:else}
            <ul class="mini-list">
              {#each incoming.slice(0, 8) as row}
                <li>
                  <RdfTerm term={row.s} />
                  <span class="predicate" title={row.p?.value}>{shortenIRI(row.p?.value || '')}</span>
                </li>
              {/each}
            </ul>
            {#if incoming.length > 8}
              <button class="btn btn-sm btn-ghost" on:click={() => (activeTab = 'linkedFrom')}>{$i18nT('pages.resource.seeAll', { values: { count: incoming.length } })}</button>
            {/if}
          {/if}
        </div>
      </div>
    {/if}

    {#if activeTab === 'properties'}
      <div class="card">
        <p class="tab-hint">
          <ArrowRight size={12} /> {$i18nT('pages.resource.outgoingHintBefore')}<strong>{$i18nT('pages.resource.subjectWord')}</strong>{$i18nT('pages.resource.outgoingHintAfter')}
        </p>
        <div class="toolbar">
          <div class="search-wrap">
            <Search size={14} class="search-ico" />
            <input
              class="toolbar-search"
              type="text"
              placeholder={$i18nT('pages.resource.searchPropertiesPlaceholder')}
              bind:value={propSearch}
            />
            {#if propSearch}
              <button class="clear-btn" on:click={() => (propSearch = '')} title={$i18nT('system.clear')}><X size={12} /></button>
            {/if}
          </div>
          <div class="toolbar-controls">
            <label class="sort-control" title={$i18nT('pages.resource.sortProperties')}>
              <ArrowDownUp size={13} />
              <Select size="sm" bind:value={propSort} ariaLabel={$i18nT('pages.resource.sortProperties')} options={[
                { value: 'pred-asc', label: $i18nT('pages.resource.sortPredAsc') },
                { value: 'pred-desc', label: $i18nT('pages.resource.sortPredDesc') },
                { value: 'val-asc', label: $i18nT('pages.resource.sortValAsc') },
                { value: 'val-desc', label: $i18nT('pages.resource.sortValDesc') },
                { value: 'type', label: $i18nT('pages.resource.sortValueType') },
              ]} />
            </label>
            <button class="toggle-btn" class:toggle-on={propGroup}
              on:click={() => (propGroup = !propGroup)}
              title={propGroup ? $i18nT('pages.resource.groupedByVocabulary') : $i18nT('pages.resource.flatList')}>
              {#if propGroup}<LayoutGrid size={13} />{:else}<List size={13} />{/if}
              {$i18nT('pages.resource.group')}
            </button>
            {#if propGroup && propertyGroups.length > 1}
              <button class="toggle-btn" on:click={togglePropAll}
                title={propAllCollapsed ? $i18nT('pages.resource.expandAllGroups') : $i18nT('pages.resource.collapseAllGroups')}>
                {#if propAllCollapsed}<ChevronDown size={13} />{:else}<ChevronRight size={13} />{/if}
                {propAllCollapsed ? $i18nT('pages.resource.expandAll') : $i18nT('pages.resource.collapseAll')}
              </button>
            {/if}
            <span class="count-pill">
              {filteredOutgoing.length}{#if filteredOutgoing.length !== outgoing.length} / {outgoing.length}{/if}
            </span>
          </div>
        </div>

        {#if sortedOutgoing.length === 0}
          <p class="muted empty-inline">{propSearch ? $i18nT('pages.resource.noPropertiesMatchQuery', { values: { query: propSearch } }) : $i18nT('pages.resource.noPropertiesMatch')}</p>
        {:else if propGroup}
          {#each propertyGroups as group (group.ns)}
            <div class="group">
              <button class="ns-header" type="button" title={group.ns}
                aria-expanded={!propCollapsed.has(group.ns)}
                on:click={() => toggleProp(group.ns)}>
                {#if propCollapsed.has(group.ns)}<ChevronRight size={13} />{:else}<ChevronDown size={13} />{/if}
                <span class="ns-name">{shortenIRI(group.ns) || group.ns}</span>
                <span class="ns-count">{group.rows.length}</span>
              </button>
              {#if !propCollapsed.has(group.ns)}
                <div class="table-scroll">
                  <table class="prop-table">
                    <tbody>
                      {#each group.rows as row}
                        <tr>
                          <td class="pred-cell">
                            <span class="predicate" title={row.p?.value}>{shortenIRI(row.p?.value || '')}</span>
                          </td>
                          <td><ValueRenderer term={row.o} predicate={row.p?.value || ''} {bnodes} on:run-sparql={onRunSparql} /></td>
                        </tr>
                      {/each}
                    </tbody>
                  </table>
                </div>
              {/if}
            </div>
          {/each}
        {:else}
          <div class="table-scroll">
            <table class="prop-table">
              <tbody>
                {#each sortedOutgoing as row}
                  <tr>
                    <td class="pred-cell">
                      <span class="predicate" title={row.p?.value}>{shortenIRI(row.p?.value || '')}</span>
                    </td>
                    <td><ValueRenderer term={row.o} predicate={row.p?.value || ''} {bnodes} on:run-sparql={onRunSparql} /></td>
                  </tr>
                {/each}
              </tbody>
            </table>
          </div>
        {/if}
      </div>
    {/if}

    {#if activeTab === 'linkedFrom'}
      <div class="card">
        <p class="tab-hint">
          <ArrowDownLeft size={12} /> {$i18nT('pages.resource.incomingHintBefore')}<strong>{$i18nT('pages.resource.objectWord')}</strong>{$i18nT('pages.resource.incomingHintAfter')}
        </p>
        {#if incoming.length === 0}
          <p class="muted empty-inline">{$i18nT('pages.resource.nothingLinksHere')}</p>
        {:else}
          <div class="toolbar">
            <div class="search-wrap">
              <Search size={14} class="search-ico" />
              <input
                class="toolbar-search"
                type="text"
                placeholder={$i18nT('pages.resource.searchIncomingPlaceholder')}
                bind:value={incomingSearch}
              />
              {#if incomingSearch}
                <button class="clear-btn" on:click={() => (incomingSearch = '')} title={$i18nT('system.clear')}><X size={12} /></button>
              {/if}
            </div>
            <div class="toolbar-controls">
              <label class="sort-control" title={$i18nT('pages.resource.sortGroups')}>
                <ArrowDownUp size={13} />
                <Select size="sm" bind:value={incomingSort} ariaLabel={$i18nT('pages.resource.sortIncomingGroups')} options={[
                  { value: 'count-desc', label: $i18nT('pages.resource.sortMostResults') },
                  { value: 'count-asc', label: $i18nT('pages.resource.sortFewestResults') },
                  { value: 'name-asc', label: $i18nT('pages.resource.sortVocabularyAsc') },
                ]} />
              </label>
              <button class="toggle-btn" class:toggle-on={incomingGroup}
                on:click={() => (incomingGroup = !incomingGroup)}
                title={incomingGroup ? $i18nT('pages.resource.groupedByVocabulary') : $i18nT('pages.resource.flatList')}>
                {#if incomingGroup}<LayoutGrid size={13} />{:else}<List size={13} />{/if}
                {$i18nT('pages.resource.group')}
              </button>
              {#if incomingGroup && incomingGroups.length > 1}
                <button class="toggle-btn" on:click={toggleIncomingAll}
                  title={incomingAllCollapsed ? $i18nT('pages.resource.expandAllGroups') : $i18nT('pages.resource.collapseAllGroups')}>
                  {#if incomingAllCollapsed}<ChevronDown size={13} />{:else}<ChevronRight size={13} />{/if}
                  {incomingAllCollapsed ? $i18nT('pages.resource.expandAll') : $i18nT('pages.resource.collapseAll')}
                </button>
              {/if}
              <span class="count-pill">
                {filteredIncoming.length}{#if filteredIncoming.length !== incoming.length} / {incoming.length}{/if}
              </span>
            </div>
          </div>

          {#if filteredIncoming.length === 0}
            <p class="muted empty-inline">{incomingSearch ? $i18nT('pages.resource.noIncomingMatchQuery', { values: { query: incomingSearch } }) : $i18nT('pages.resource.noIncomingMatch')}</p>
          {:else if incomingGroup}
            {#each incomingGroups as group (group.ns)}
              <div class="group">
                <button class="ns-header" type="button" title={group.ns}
                  aria-expanded={!incomingCollapsed.has(group.ns)}
                  on:click={() => toggleIncoming(group.ns)}>
                  {#if incomingCollapsed.has(group.ns)}<ChevronRight size={13} />{:else}<ChevronDown size={13} />{/if}
                  <span class="ns-name">{shortenIRI(group.ns) || group.ns}</span>
                  <span class="ns-count">{group.rows.length}</span>
                </button>
                {#if !incomingCollapsed.has(group.ns)}
                  <div class="table-scroll">
                    <table class="prop-table">
                      <tbody>
                        {#each group.rows as row}
                          <tr>
                            <td class="pred-cell">
                              <span class="predicate" title={row.p?.value}>{shortenIRI(row.p?.value || '')}</span>
                            </td>
                            <td><RdfTerm term={row.s} /></td>
                          </tr>
                        {/each}
                      </tbody>
                    </table>
                  </div>
                {/if}
              </div>
            {/each}
          {:else}
            <div class="table-scroll">
              <table class="prop-table">
                <tbody>
                  {#each flatIncoming as row}
                    <tr>
                      <td class="pred-cell">
                        <span class="predicate" title={row.p?.value}>{shortenIRI(row.p?.value || '')}</span>
                      </td>
                      <td><RdfTerm term={row.s} /></td>
                    </tr>
                  {/each}
                </tbody>
              </table>
            </div>
          {/if}
        {/if}
      </div>
    {/if}
  {/if}
  {/if}
</div>

<style>
  .resource-page { display: flex; flex-direction: column; gap: 1rem; }
  h2 { margin: 0 0 0.2rem; word-break: break-word; font-size: 1.4rem; letter-spacing: -0.01em; }
  h3 { margin: 0 0 0.75rem; font-size: 0.95rem; color: #555; display: inline-flex; align-items: center; gap: 0.35rem; }

  .crumbs { display: flex; align-items: center; gap: 0.5rem; font-size: 0.8rem; color: var(--ink-700); flex-wrap: wrap; }
  .crumb-btn { display: inline-flex; align-items: center; gap: 0.3rem; padding: 0.32rem 0.65rem; border-radius: 10px; border: 1px solid var(--line-soft); background: rgba(255,255,255,0.7); cursor: pointer; color: inherit; font-size: 0.8rem; transition: background .12s, border-color .12s; }
  .crumb-btn:hover { background: #fff; border-color: var(--brand-300, #90caf9); }
  .crumb-sep { color: #aaa; }
  .crumb-scope { display: inline-flex; align-items: center; gap: 0.3rem; padding: 0.2rem 0.55rem; border-radius: 999px; background: #eef5ff; color: #1565c0; font-weight: 500; }

  /* Triple-browser dropdown */
  .tb-menu-wrap { position: relative; }
  .tb-dropdown { position: absolute; right: 0; top: calc(100% + 6px); background: #fff; border: 1px solid #e2e6ee; border-radius: 12px; box-shadow: 0 10px 28px rgba(20,40,80,0.16); z-index: 120; min-width: 188px; padding: 5px; overflow: hidden; }
  .tb-dropdown-head { font-size: 0.66rem; text-transform: uppercase; letter-spacing: 0.07em; color: #9aa3b2; padding: 0.35rem 0.55rem 0.25rem; }
  .tb-dropdown button { display: flex; align-items: center; gap: 0.55rem; width: 100%; padding: 0.5rem 0.6rem; border: none; background: transparent; cursor: pointer; font-size: 0.85rem; color: #2b3445; border-radius: 8px; text-align: left; }
  .tb-dropdown button:hover { background: #eef4ff; color: #1565c0; }

  /* Header card */
  .resource-header { background: linear-gradient(180deg, #ffffff 0%, #fbfcfe 100%); }
  .file-header { display: flex; flex-direction: column; gap: 0.4rem; }
  .file-title { display: inline-flex; align-items: center; gap: 0.45rem; color: var(--ink-800, #2b3445); }
  .file-title h2 { margin: 0; }
  .vocab-def-card { border-left: 3px solid #6366f1; }
  .vocab-def-title { margin: 0 0 0.5rem; font-size: 0.7rem; font-weight: 700; text-transform: uppercase; letter-spacing: 0.04em; color: #6366f1; }
  .header-top { display: flex; align-items: flex-start; justify-content: space-between; gap: 1rem; flex-wrap: wrap; }
  .min-w-0 { min-width: 0; }
  .short-iri { font-size: 0.8rem; color: #6a5acd; margin-bottom: 0.25rem; font-weight: 500; }
  .iri-full { display: inline-flex; align-items: center; gap: 0.4rem; font-family: monospace; font-size: 0.75rem; color: #4a90d9; word-break: break-all; max-width: 100%; }
  .icon-btn { border: none; background: transparent; color: inherit; cursor: pointer; padding: 2px; border-radius: 6px; }
  .icon-btn:hover { background: rgba(0,0,0,0.06); }

  .header-actions { display: flex; gap: 0.4rem; flex-shrink: 0; flex-wrap: wrap; }

  .type-badges { display: flex; flex-wrap: wrap; gap: 0.35rem; margin-top: 0.85rem; align-items: center; }
  .badge-label { font-size: 0.7rem; color: #888; text-transform: uppercase; letter-spacing: 0.08em; margin-right: 0.25rem; }
  .type-badge { display: inline-flex; align-items: center; gap: 0.28rem; padding: 3px 9px; background: #e3f2fd; color: #1565c0; border: 1px solid #bbdefb; border-radius: 999px; font-size: 0.74rem; cursor: pointer; transition: background .12s, color .12s; }
  .type-badge:hover { background: #1565c0; color: #fff; border-color: #1565c0; }

  .stat-row { display: flex; flex-wrap: wrap; gap: 0.6rem; margin-top: 1rem; padding-top: 0.85rem; border-top: 1px dashed var(--line-soft); }
  .stat { display: inline-flex; align-items: center; gap: 0.4rem; font-size: 0.82rem; color: var(--ink-700); background: #f7f9fc; border: 1px solid var(--line-soft); border-radius: 10px; padding: 0.4rem 0.7rem; }
  .stat-clickable { cursor: pointer; transition: background .12s, border-color .12s, transform .08s; }
  .stat-clickable:hover { background: #eef4ff; border-color: var(--brand-300, #90caf9); }
  .stat-clickable:active { transform: translateY(1px); }
  .stat-num { font-weight: 700; color: var(--ink-900, #111); font-variant-numeric: tabular-nums; }
  /* Placeholder for a stat count while the resource is still loading, sized like a
     1-2 digit number so the row does not resize when the real value arrives. */
  .stat-skel { display: inline-block; width: 1.4rem; height: 0.9rem; border-radius: 5px; vertical-align: middle; }
  .stat-label { color: #888; }

  .tabs { display: flex; gap: 0.25rem; padding: 0.3rem; border-radius: 14px; background: rgba(255,255,255,0.7); border: 1px solid var(--line-soft); width: fit-content; max-width: 100%; flex-wrap: wrap; }
  .tab { display: inline-flex; align-items: center; gap: 0.4rem; padding: 0.42rem 0.85rem; border: none; background: transparent; border-radius: 10px; cursor: pointer; font-size: 0.85rem; color: var(--ink-700); transition: background .12s, color .12s; }
  .tab:hover { background: rgba(0,0,0,0.04); }
  .tab-active { background: #1565c0; color: #fff; }
  .tab-active:hover { background: #0d4a94; }
  .tab-count { font-size: 0.72rem; padding: 1px 7px; border-radius: 999px; background: rgba(0,0,0,0.08); font-variant-numeric: tabular-nums; }
  .tab-active .tab-count { background: rgba(255,255,255,0.25); }

  .label-list { display: flex; flex-direction: column; gap: 0.4rem; }
  .label-item { display: flex; flex-wrap: wrap; align-items: baseline; gap: 0.5rem; }
  .lit { font-size: 1rem; font-weight: 600; color: #222; }
  .lang-tag { font-size: 0.7rem; padding: 1px 6px; background: #eef; color: #446; border-radius: 6px; }
  .pred-hint { font-family: monospace; font-size: 0.72rem; color: #888; }
  .label-list.alt { display: flex; flex-direction: row; flex-wrap: wrap; gap: 0.35rem; margin-top: 0.6rem; padding-top: 0.5rem; border-top: 1px dashed var(--line-soft); align-items: center; }
  .alt-chip { padding: 2px 8px; background: #f3f3f7; border: 1px solid #e2e2e9; border-radius: 10px; font-size: 0.78rem; }
  .alt-chip em { color: #888; font-style: normal; font-size: 0.7rem; }

  .def-list { display: flex; flex-direction: column; gap: 0.6rem; }
  .def-item { padding: 0.7rem 0.85rem; background: #fbfaf5; border: 1px solid #f0ecdc; border-radius: 12px; }
  .def-text { color: #222; line-height: 1.5; white-space: pre-wrap; }
  .def-pred { margin-top: 0.4rem; font-family: monospace; font-size: 0.72rem; color: #9a8b3a; }

  .split-grid { display: grid; grid-template-columns: 1fr 1fr; gap: 1rem; }
  @media (max-width: 900px) { .split-grid { grid-template-columns: 1fr; } }
  .mini-list { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 0.35rem; font-size: 0.85rem; }
  .mini-list li { display: flex; align-items: center; gap: 0.4rem; flex-wrap: wrap; }
  .muted { color: #888; font-size: 0.85rem; }
  .empty-inline { padding: 0.5rem 0.25rem; }

  /* Toolbar (search + sort + group) */
  .toolbar { display: flex; align-items: center; gap: 0.6rem; margin-bottom: 0.85rem; flex-wrap: wrap; }
  .search-wrap { position: relative; flex: 1 1 240px; display: flex; align-items: center; }
  .search-wrap :global(.search-ico) { position: absolute; left: 0.6rem; color: #9aa3b2; pointer-events: none; }
  .toolbar-search { flex: 1; width: 100%; padding: 0.46rem 1.9rem 0.46rem 2rem; border-radius: 10px; border: 1px solid var(--line-soft); background: #fff; font-size: 0.85rem; }
  .toolbar-search:focus { outline: none; border-color: var(--brand-300, #90caf9); box-shadow: 0 0 0 3px rgba(21,101,192,0.10); }
  .clear-btn { position: absolute; right: 0.45rem; border: none; background: transparent; color: #9aa3b2; cursor: pointer; padding: 2px; border-radius: 6px; display: inline-flex; }
  .clear-btn:hover { background: rgba(0,0,0,0.06); color: #555; }
  .toolbar-controls { display: flex; align-items: center; gap: 0.5rem; flex-wrap: wrap; }
  .sort-control { display: inline-flex; align-items: center; gap: 0.35rem; color: #6a7383; border: 1px solid var(--line-soft); border-radius: 10px; padding: 0.2rem 0.45rem 0.2rem 0.55rem; background: #fff; }
  :global(.sort-control .sel-trigger) { border: none; background: transparent; font-size: 0.82rem; color: #2b3445; cursor: pointer; padding: 0.22rem 0.1rem; box-shadow: none; }
  .toggle-btn { display: inline-flex; align-items: center; gap: 0.35rem; padding: 0.4rem 0.7rem; border: 1px solid var(--line-soft); border-radius: 10px; background: #fff; cursor: pointer; font-size: 0.82rem; color: #6a7383; transition: background .12s, color .12s, border-color .12s; }
  .toggle-btn:hover { background: #f4f7fb; }
  .toggle-on { background: #eef4ff; color: #1565c0; border-color: #bbdefb; }
  .count-pill { font-size: 0.76rem; color: #6a7383; background: #f0f2f6; border-radius: 999px; padding: 0.2rem 0.6rem; font-variant-numeric: tabular-nums; white-space: nowrap; }

  .group { margin: 0.7rem 0 0.3rem; }
  .ns-header { display: flex; align-items: center; gap: 0.4rem; width: 100%; padding: 0.4rem 0.6rem; font-family: monospace; font-size: 0.75rem; color: #6a5acd; background: #f7f5ff; border: none; border-left: 3px solid #6a5acd; border-radius: 4px; cursor: pointer; text-align: left; }
  .ns-header:hover { background: #efeaff; }
  .ns-header:focus-visible { outline: 2px solid #6a5acd; outline-offset: 1px; }
  .ns-name { font-family: inherit; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .ns-count { margin-left: auto; flex-shrink: 0; font-family: inherit; font-size: 0.7rem; color: #8a7fd0; background: #ece9fb; border-radius: 999px; padding: 1px 8px; }

  .tab-hint { display: flex; align-items: center; gap: 0.35rem; margin: 0 0 0.8rem; font-size: 0.78rem; color: #7a8294; }
  .tab-hint strong { color: #46506a; font-weight: 600; }

  .flash { animation: flashHi 1.5s ease; border-radius: 12px; }
  @keyframes flashHi {
    0% { box-shadow: 0 0 0 0 rgba(106,90,205,0); }
    18% { box-shadow: 0 0 0 3px rgba(106,90,205,0.5); }
    100% { box-shadow: 0 0 0 0 rgba(106,90,205,0); }
  }

  .loading-text { color: #888; }
  .table-scroll { overflow-x: auto; }
  .prop-table { width: 100%; border-collapse: collapse; }
  .prop-table td { vertical-align: top; padding: 0.4rem 0.55rem; border-bottom: 1px solid #f1f3f7; }
  .prop-table tr:last-child td { border-bottom: none; }
  .prop-table tr:hover td { background: #fafbfe; }
  .pred-cell { width: 35%; white-space: nowrap; }
  .predicate { color: #6a5acd; font-size: 0.85rem; font-weight: 500; }

  .btn-ghost { background: transparent; color: #4a90d9; border: 1px solid #4a90d9; }
  .btn-ghost:hover { background: #e8f2fc; }
  .ld-menu-wrap { position: relative; }
  .ld-dropdown { position: absolute; right: 0; top: calc(100% + 4px); background: #fff; border: 1px solid #d0d0d0; border-radius: 6px; box-shadow: 0 4px 12px rgba(0,0,0,0.12); z-index: 100; min-width: 120px; padding: 4px 0; }
  .ld-dropdown a { display: block; padding: 6px 14px; font-size: 0.82rem; color: #333; text-decoration: none; }
  .ld-dropdown a:hover { background: #f0f4ff; }

  .truncate { overflow: hidden; text-overflow: ellipsis; max-width: 600px; }

  .empty-state { display: flex; gap: 0.75rem; align-items: flex-start; padding: 1rem; background: #fffbe6; border: 1px solid #ffe58f; color: #614700; }
  .empty-state strong { display: block; margin-bottom: 0.25rem; }
  .empty-state .muted { margin: 0 0 0.5rem; }

  .bim-facts {
    display: flex;
    flex-wrap: wrap;
    gap: 0.5rem;
    margin-top: 0.5rem;
    align-items: center;
  }
  .bim-fact {
    font-size: 0.74rem;
    padding: 2px 10px;
    border-radius: 99px;
    border: 1px solid var(--line-soft, #e5e9ee);
    background: var(--bg-subtle, #f8fafc);
    color: var(--muted, #64748b);
    text-decoration: none;
  }
  .bim-fact code { font-size: 0.72rem; color: var(--ink-900, #0f172a); }
  a.bim-fact:hover { border-color: var(--brand-500, #2f88d8); color: var(--brand-600, #1d6fb8); }

  .img-strip { display: flex; flex-wrap: wrap; gap: 0.5rem; }
  .img-strip a { display: inline-block; }
  .img-strip img { max-width: 160px; max-height: 140px; object-fit: contain; border: 1px solid #e5e7eb; border-radius: 6px; background: #f9fafb; }

  .link-chips { display: flex; flex-wrap: wrap; gap: 0.35rem; }
  .link-chip { display: inline-block; padding: 3px 10px; background: #eef5ff; color: #1565c0; border: 1px solid #cfe1fb; border-radius: 999px; text-decoration: none; font-size: 0.8rem; }
  .link-chip:hover { background: #1565c0; color: #fff; }

  /* Key facts infobox */
  .facts-table { width: 100%; border-collapse: collapse; }
  .facts-table td { vertical-align: top; padding: 0.4rem 0.55rem; border-bottom: 1px solid #f1f3f7; }
  .facts-table tr:last-child td { border-bottom: none; }
  .fact-key { width: 38%; color: #6a5acd; font-size: 0.85rem; font-weight: 500; white-space: nowrap; }

  /* Quantities */
  .quantity-grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(150px, 1fr)); gap: 0.6rem; }
  .quantity-card { padding: 0.7rem 0.85rem; background: #f7f9fc; border: 1px solid var(--line-soft); border-radius: 12px; }
  .quantity-num { font-size: 1.5rem; font-weight: 700; color: #1e40af; font-variant-numeric: tabular-nums; line-height: 1.1; word-break: break-word; }
  .quantity-label { margin-top: 0.25rem; font-size: 0.8rem; color: #555; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .quantity-dt { margin-top: 0.2rem; font-family: monospace; font-size: 0.68rem; color: #94a3b8; }

  /* Timeline (vertical, dot drawn via ::before) */
  .timeline { display: flex; flex-direction: column; }
  .tl-item { display: flex; align-items: baseline; gap: 0.6rem; flex-wrap: wrap; padding: 0 0 0.7rem 1.1rem; margin-left: 5px; position: relative; border-left: 2px solid #e6e3f7; }
  .tl-item:last-child { border-left-color: transparent; padding-bottom: 0; }
  .tl-item::before { content: ''; position: absolute; left: -6px; top: 0.3rem; width: 10px; height: 10px; border-radius: 50%; background: #6a5acd; border: 2px solid #fff; box-shadow: 0 0 0 2px #d9d3f5; }
  .tl-date { font-size: 0.85rem; font-weight: 600; color: #46506a; font-variant-numeric: tabular-nums; min-width: 96px; }
  .tl-pred { font-family: monospace; font-size: 0.75rem; color: #8a7fd0; }

  /* Vocabularies */
  .vocab-pills { display: flex; flex-wrap: wrap; gap: 0.4rem; }
  .vocab-pill { display: inline-flex; align-items: center; gap: 0.35rem; padding: 0.2rem 0.4rem 0.2rem 0.6rem; border-radius: 999px; font-family: monospace; font-size: 0.78rem; font-weight: 600; }
  .vocab-count { font-size: 0.68rem; background: rgba(255,255,255,0.65); border-radius: 999px; padding: 0 6px; font-weight: 700; }

  /* ─── Dark theme overrides ───────────────────────────────────────────────── */
  :global(:is([data-theme="dark"], .dark)) h3,
  :global(:is([data-theme="dark"], .dark)) .crumb-sep,
  :global(:is([data-theme="dark"], .dark)) .badge-label,
  :global(:is([data-theme="dark"], .dark)) .stat-label,
  :global(:is([data-theme="dark"], .dark)) .pred-hint,
  :global(:is([data-theme="dark"], .dark)) .alt-chip em,
  :global(:is([data-theme="dark"], .dark)) .muted,
  :global(:is([data-theme="dark"], .dark)) .loading-text,
  :global(:is([data-theme="dark"], .dark)) .quantity-label,
  :global(:is([data-theme="dark"], .dark)) .tab-hint,
  :global(:is([data-theme="dark"], .dark)) .tb-dropdown-head { color: var(--ink-500); }
  :global(:is([data-theme="dark"], .dark)) .lit,
  :global(:is([data-theme="dark"], .dark)) .def-text { color: var(--ink-900); }
  :global(:is([data-theme="dark"], .dark)) .short-iri,
  :global(:is([data-theme="dark"], .dark)) .predicate,
  :global(:is([data-theme="dark"], .dark)) .fact-key { color: #a5b4fc; }
  :global(:is([data-theme="dark"], .dark)) .tl-pred { color: #c4b5fd; }
  :global(:is([data-theme="dark"], .dark)) .iri-full { color: #60a5fa; }

  :global(:is([data-theme="dark"], .dark)) .crumb-btn { background: rgba(255,255,255,0.06); }
  :global(:is([data-theme="dark"], .dark)) .crumb-btn:hover { background: rgba(255,255,255,0.1); }
  :global(:is([data-theme="dark"], .dark)) .icon-btn:hover { background: rgba(255,255,255,0.08); }
  :global(:is([data-theme="dark"], .dark)) .crumb-scope,
  :global(:is([data-theme="dark"], .dark)) .type-badge,
  :global(:is([data-theme="dark"], .dark)) .toggle-on,
  :global(:is([data-theme="dark"], .dark)) .link-chip { background: rgba(59,130,246,0.16); color: #93c5fd; border-color: rgba(59,130,246,0.3); }
  :global(:is([data-theme="dark"], .dark)) .quantity-num { color: #93c5fd; }

  :global(:is([data-theme="dark"], .dark)) .tb-dropdown,
  :global(:is([data-theme="dark"], .dark)) .ld-dropdown { background: var(--bg-strong); border-color: var(--line-strong); }
  :global(:is([data-theme="dark"], .dark)) .tb-dropdown button { color: var(--ink-800); }
  :global(:is([data-theme="dark"], .dark)) .tb-dropdown button:hover,
  :global(:is([data-theme="dark"], .dark)) .stat-clickable:hover,
  :global(:is([data-theme="dark"], .dark)) .ld-dropdown a:hover { background: rgba(59,130,246,0.15); color: #93c5fd; }
  :global(:is([data-theme="dark"], .dark)) .ld-dropdown a { color: var(--ink-800); }

  :global(:is([data-theme="dark"], .dark)) .resource-header { background: linear-gradient(180deg, rgba(255,255,255,0.04), rgba(255,255,255,0.02)); }
  :global(:is([data-theme="dark"], .dark)) .stat,
  :global(:is([data-theme="dark"], .dark)) .quantity-card,
  :global(:is([data-theme="dark"], .dark)) .alt-chip { background: rgba(255,255,255,0.04); }
  :global(:is([data-theme="dark"], .dark)) .alt-chip { border-color: var(--line-soft); }
  :global(:is([data-theme="dark"], .dark)) .tabs { background: rgba(255,255,255,0.05); }
  :global(:is([data-theme="dark"], .dark)) .tab:hover { background: rgba(255,255,255,0.06); }
  :global(:is([data-theme="dark"], .dark)) .tab-count { background: rgba(255,255,255,0.12); }
  :global(:is([data-theme="dark"], .dark)) .lang-tag { background: rgba(99,102,241,0.18); color: #c7d2fe; }

  :global(:is([data-theme="dark"], .dark)) .def-item { background: rgba(245,158,11,0.07); border-color: rgba(245,158,11,0.18); }
  :global(:is([data-theme="dark"], .dark)) .def-pred { color: #cbb765; }

  :global(:is([data-theme="dark"], .dark)) .toolbar-search,
  :global(:is([data-theme="dark"], .dark)) .sort-control,
  :global(:is([data-theme="dark"], .dark)) .toggle-btn { background: var(--bg-strong); }
  :global(:is([data-theme="dark"], .dark)) .sort-control { color: var(--ink-600); }
  :global(:is([data-theme="dark"], .dark) .sort-control .sel-trigger) { color: var(--ink-800); }
  :global(:is([data-theme="dark"], .dark)) .toggle-btn { color: var(--ink-600); }
  :global(:is([data-theme="dark"], .dark)) .toggle-btn:hover { background: rgba(255,255,255,0.06); }
  :global(:is([data-theme="dark"], .dark)) .clear-btn,
  :global(:is([data-theme="dark"], .dark)) .search-wrap :global(.search-ico) { color: var(--ink-500); }
  :global(:is([data-theme="dark"], .dark)) .clear-btn:hover { background: rgba(255,255,255,0.08); color: var(--ink-300); }
  :global(:is([data-theme="dark"], .dark)) .count-pill { background: rgba(255,255,255,0.06); color: var(--ink-600); }

  :global(:is([data-theme="dark"], .dark)) .ns-header { background: rgba(124,58,237,0.12); color: #c4b5fd; border-left-color: #a78bfa; }
  :global(:is([data-theme="dark"], .dark)) .ns-header:hover { background: rgba(124,58,237,0.18); }
  :global(:is([data-theme="dark"], .dark)) .ns-count { background: rgba(124,58,237,0.2); color: #c4b5fd; }
  :global(:is([data-theme="dark"], .dark)) .tab-hint strong,
  :global(:is([data-theme="dark"], .dark)) .tl-date { color: var(--ink-300); }

  :global(:is([data-theme="dark"], .dark)) .prop-table td,
  :global(:is([data-theme="dark"], .dark)) .facts-table td { border-bottom-color: var(--line-soft); }
  :global(:is([data-theme="dark"], .dark)) .prop-table tr:hover td { background: rgba(255,255,255,0.04); }

  :global(:is([data-theme="dark"], .dark)) .btn-ghost { color: #60a5fa; border-color: rgba(96,165,250,0.5); background: transparent; }
  :global(:is([data-theme="dark"], .dark)) .btn-ghost:hover { background: rgba(59,130,246,0.15); }
  :global(:is([data-theme="dark"], .dark)) .empty-state { background: rgba(245,158,11,0.12); border-color: rgba(245,158,11,0.4); color: #fcd34d; }
  :global(:is([data-theme="dark"], .dark)) .img-strip img { border-color: var(--line-soft); background: rgba(255,255,255,0.03); }

  :global(:is([data-theme="dark"], .dark)) .tl-item { border-left-color: rgba(124,58,237,0.3); }
  :global(:is([data-theme="dark"], .dark)) .tl-item::before { background: #a78bfa; border-color: var(--bg-canvas); box-shadow: 0 0 0 2px rgba(124,58,237,0.4); }
  :global(:is([data-theme="dark"], .dark)) .vocab-count { background: rgba(255,255,255,0.15); }
</style>
