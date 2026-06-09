<script>
  import { onMount, onDestroy } from 'svelte';
  import { sparqlQuery, datasetSparqlQuery, listDatasets, listOrganisations, listServices, listDatasetGraphs, listDatasetVersions, getDataset, getOrganisation, nlToSparql, sendLlmFeedback, llmHealth } from '../lib/api.js';
  import { graphResultsToElements, resultsToCsv, downloadFile, parseNTriplesToBindings } from '../lib/rdf-utils.js';
  import GraphCanvas from '../components/GraphCanvas.svelte';
  import SparqlEditorCM from '../components/SparqlEditorCM.svelte';
  import Select from '../components/Select.svelte';
  import DataTable from '../components/DataTable.svelte';
  import PrefixSearchPanel from '../components/PrefixSearchPanel.svelte';
  import { t as i18nT } from 'svelte-i18n';
  import { Play, Loader2, Clock, Download, Check, X as XIcon, Trash2, FileCode, BookOpen, Layers, Wand2, ThumbsUp, ThumbsDown, LayoutList, Building2, Database, Plus, Lock } from 'lucide-svelte';
  import { Link, navigate } from '../lib/router/index.js';
  import { extractDeclaredPrefixes, extractUsedPrefixes, lookupPrefix, lookupPrefixSync } from '../lib/ontology/prefixService.js';
  import { formatSparql } from '../lib/ontology/sparqlFormat.js';
  import { NAMESPACES } from '../lib/ontology/vocabularies.js';
  import { autofocus } from '../lib/actions/autofocus.js';
  import PageHeader from '../components/PageHeader.svelte';

  // Optional context props – when set the editor is scoped to a dataset or org
  export let datasetId = null;
  export let orgId = null;
  let contextName = null;
  // Whether the signed-in caller may save queries to this dataset (owner/editor).
  let canWriteDataset = false;
  // When opened from an API service, a link back to that service (re-expanded).
  let returnTo = null;
  let returnFrom = '';

  // Hand the current query off to the dataset's api-services page to save it.
  function saveAsSavedQuery() {
    try { sessionStorage.setItem('ots_sq_prefill', query); } catch {}
    navigate(`/datasets/${datasetId}/api-services?prefill=1`);
  }
  $: if (datasetId) {
    getDataset(datasetId).then(d => { contextName = d?.name ?? datasetId; canWriteDataset = !!d?.can_write; }).catch(() => { contextName = datasetId; });
  } else if (orgId) {
    getOrganisation(orgId).then(o => { contextName = o?.name ?? orgId; }).catch(() => { contextName = orgId; });
  }

  const HISTORY_KEY = 'sparql_query_history_v2';
  const MAX_HISTORY = 50;

  const TEMPLATES = [
    { name: 'List classes', body: `PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nPREFIX owl: <http://www.w3.org/2002/07/owl#>\n\nSELECT DISTINCT ?class ?label WHERE {\n  { ?class a owl:Class } UNION { ?class a rdfs:Class }\n  OPTIONAL { ?class rdfs:label ?label }\n} LIMIT 100` },
    { name: 'Class hierarchy', body: `PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\n\nSELECT ?child ?parent WHERE {\n  ?child rdfs:subClassOf ?parent .\n  FILTER(isIRI(?parent))\n} LIMIT 500` },
    { name: 'Properties with domain/range', body: `PREFIX rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\n\nSELECT ?p ?domain ?range WHERE {\n  ?p a rdf:Property .\n  OPTIONAL { ?p rdfs:domain ?domain }\n  OPTIONAL { ?p rdfs:range ?range }\n} LIMIT 200` },
    { name: 'SHACL shapes', body: `PREFIX sh: <http://www.w3.org/ns/shacl#>\n\nSELECT ?shape ?target ?path WHERE {\n  ?shape a sh:NodeShape .\n  OPTIONAL { ?shape sh:targetClass ?target }\n  OPTIONAL { ?shape sh:property/sh:path ?path }\n} LIMIT 200` },
    { name: 'Count by type', body: `SELECT ?type (COUNT(?s) AS ?count) WHERE {\n  ?s a ?type\n} GROUP BY ?type ORDER BY DESC(?count) LIMIT 50` },
    { name: 'Named graphs', body: `SELECT ?g (COUNT(*) AS ?triples) WHERE {\n  GRAPH ?g { ?s ?p ?o }\n} GROUP BY ?g ORDER BY DESC(?triples)` },
  ];

  let query = `PREFIX rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#>
PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>

SELECT ?s ?p ?o
WHERE {
  ?s ?p ?o
}
LIMIT 25`;

  let results = null;
  let error = '';
  let loading = false;
  let elapsed = 0;
  let activeTab = 'table'; // 'table' | 'json' | 'graph'

  // ── Working-state memory (sessionStorage) ──────────────────────────────────
  // Persist the editor's working state — query, scope (datasets/orgs + selected
  // graphs) and active result tab — so a browser back/forward (Alt+←) returns to
  // the editor as the user left it instead of resetting. Keyed per route so the
  // global editor and a dataset/org-scoped editor don't share state. Result sets
  // are intentionally NOT persisted (quota / size).
  $: routeKey = datasetId ? `dataset:${datasetId}` : orgId ? `org:${orgId}` : 'global';
  $: stateKey = `ots:sparqlEditor:${routeKey}`;
  let persistReady = false; // gate writes until after the initial restore

  function persistState() {
    if (!persistReady) return;
    try {
      sessionStorage.setItem(stateKey, JSON.stringify({
        query,
        scopeItems,
        selectedGraphs: [...selectedGraphIris],
        activeTab,
      }));
    } catch {}
  }
  // Re-persist whenever any tracked piece of working state changes. Listing the
  // dependencies keeps Svelte's reactivity tracking them; the guard inside
  // persistState() suppresses writes until the initial restore has run.
  $: persistReady, query, scopeItems, selectedGraphIris, activeTab, persistState();

  // ── NL → SPARQL ("lookup triples with LLM") ──────────────────────────────────
  let nlQuestion = '';
  let nlLoading = false;
  let nlError = '';
  // Set after a successful generation so we can attribute the user's next action
  // (run as-is = approve, run after editing = edit, dismiss = reject) as a training signal.
  let lastGenerated = null; // { question, sparql } | null
  let nlComment = '';       // optional free-text review attached to the next feedback signal
  let nlReviewed = null;    // 'up' | 'down' — thumbs review the user gave this suggestion
  let llmStatus = null;     // { reachable, gateway } — LLM service availability for the health badge

  async function generateFromNl() {
    const q = nlQuestion.trim();
    if (!q || nlLoading) return;
    nlLoading = true;
    nlError = '';
    try {
      const schemaHint = Object.entries(declaredPrefixes)
        .map(([p, ns]) => `PREFIX ${p}: <${ns}>`)
        .join('\n');
      // Pass the current editor content so the model can refine/extend it in place
      // rather than always replacing it.
      const { sparql } = await nlToSparql(q, schemaHint, query);
      if (sparql && sparql.trim()) {
        // Auto-format so the generated query lands as readable multi-line SPARQL
        // instead of a single line. formatSparql is idempotent and returns the
        // input unchanged if it can't parse it, so this is always safe.
        let generated = sparql.trim();
        try { generated = formatSparql(generated); } catch { /* keep raw on format failure */ }
        query = generated;
        lastGenerated = { question: q, sparql: query };
      } else {
        nlError = $i18nT('pages.sparql.emptyQueryError');
      }
    } catch (e) {
      nlError = e?.message || $i18nT('pages.sparql.generationFailed');
    } finally {
      nlLoading = false;
    }
  }

  function buildSparqlSignal(decision, editedArtifact, rating = null) {
    return {
      track: 'sparql',
      event: 'sparql_gen',
      input: { nl_question: lastGenerated?.question ?? null },
      output: { corrected_turtle: lastGenerated?.sparql ?? null },
      label: {
        decision,
        edited_turtle: editedArtifact ?? null,
        source: 'human',
        rating,
        comment: nlComment.trim() || null,
      },
      prov: { app: 'opentriplestore' },
    };
  }

  // Explicit thumbs review of the generated query. 👍 is a strong positive; 👎 marks it poor
  // (and dismisses it). Both can carry the optional comment.
  function reviewGenerated(rating) {
    if (!lastGenerated) return;
    nlReviewed = rating;
    sendLlmFeedback(buildSparqlSignal(rating === 'down' ? 'reject' : null, null, rating));
    if (rating === 'down') { lastGenerated = null; nlComment = ''; }
  }


  let history = []; // Array<{q, t}>
  let showHistory = false;
  let showTemplates = false;
  let historyFilter = '';

  // Reactive: declared + used prefixes
  $: declaredPrefixes = extractDeclaredPrefixes(query);
  $: usedPrefixes = extractUsedPrefixes(query);
  $: missingPrefixes = usedPrefixes.filter(p => !declaredPrefixes[p]);

  function handleOutsideClick(e) {
    if (!e.target.closest('.history-wrapper')) showHistory = false;
    if (!e.target.closest('.templates-wrapper')) showTemplates = false;
    if (!e.target.closest('.scope-picker-wrap')) scopePickerOpen = false;
    if (!e.target.closest('.graph-scope-wrap')) graphScopeOpen = false;
    if (!e.target.closest('.prefix-add-wrap')) prefixSearchOpen = false;
  }
  function handleKey(e) {
    if (e.key === 'Escape') { showHistory = false; showTemplates = false; scopePickerOpen = false; graphScopeOpen = false; prefixSearchOpen = false; }
  }

  function insertMissingPrefixes() {
    const lines = [];
    for (const p of missingPrefixes) {
      const ns = NAMESPACES[p] || lookupPrefixSync(p);
      if (ns) lines.push(`PREFIX ${p}: <${ns}>`);
    }
    // Async fetch any still unknown
    Promise.all(missingPrefixes
      .filter(p => !NAMESPACES[p] && !lookupPrefixSync(p))
      .map(async p => {
        const ns = await lookupPrefix(p);
        if (ns) return `PREFIX ${p}: <${ns}>`;
        return null;
      })
    ).then(extra => {
      const add = [...lines, ...extra.filter(Boolean)].join('\n');
      if (add) query = add + '\n' + query.replace(new RegExp(`^(${lines.join('\\n')})\\n?`), '');
    });
    if (lines.length) query = lines.join('\n') + '\n' + query;
  }

  function removePrefix(p) {
    const re = new RegExp(`^\\s*PREFIX\\s+${p}\\s*:\\s*<[^>]+>\\s*\\n?`, 'mi');
    query = query.replace(re, '');
  }

  // ── Prefix search panel (richer "Add prefix" affordance) ───────────────────
  let prefixSearchOpen = false;
  // Insert a PREFIX declaration chosen from the search panel, unless that prefix
  // label is already declared (mirrors insertMissingPrefixes' prepend approach).
  function addPrefixFromSearch(prefix, namespace) {
    if (!prefix || !namespace) return;
    if (declaredPrefixes[prefix]) return; // already declared — no-op
    query = `PREFIX ${prefix}: <${namespace}>\n` + query;
  }

  function useTemplate(tpl) {
    query = tpl.body;
    showTemplates = false;
  }

  function deleteHistoryItem(idx, e) {
    e.stopPropagation();
    history = history.filter((_, i) => i !== idx);
    try { localStorage.setItem(HISTORY_KEY, JSON.stringify(history)); } catch {}
  }
  function clearHistory() {
    if (!confirm($i18nT('pages.sparql.clearHistoryConfirm'))) return;
    history = [];
    try { localStorage.removeItem(HISTORY_KEY); } catch {}
  }

  function relativeTime(t) {
    const s = Math.floor((Date.now() - t) / 1000);
    if (s < 60) return $i18nT('pages.sparql.secondsAgo', { values: { n: s } });
    if (s < 3600) return $i18nT('pages.sparql.minutesAgo', { values: { n: Math.floor(s / 60) } });
    if (s < 86400) return $i18nT('pages.sparql.hoursAgo', { values: { n: Math.floor(s / 3600) } });
    return $i18nT('pages.sparql.daysAgo', { values: { n: Math.floor(s / 86400) } });
  }

  // Live-info fetcher for editor hover tooltips — routes through currently-selected endpoint
  async function hoverFetcher(q) {
    const endpoint = selectedEndpoint || 'main';
    if (versionActive && scopeDatasetId) {
      return datasetSparqlQuery(scopeDatasetId, serviceSlugFor(scopeDatasetId), q, selectedVersion);
    }
    if (selectedGraphList.length > 0) {
      return sparqlQuery(injectFromClauses(q, selectedGraphList));
    }
    if (scopeActive) {
      const iris = scopedFromIris();
      return sparqlQuery(iris.length ? injectFromClauses(q, iris) : q);
    }
    if (endpoint === 'main') return sparqlQuery(q);
    if (endpoint === 'dataset-default') return sparqlQuery(injectFromClauses(q, datasetGraphIris));
    if (endpoint.startsWith('dataset:')) {
      const dsId = endpoint.slice('dataset:'.length);
      return sparqlQuery(injectFromClauses(q, datasetGraphsById[dsId] ?? []));
    }
    const slash = endpoint.indexOf('/');
    return datasetSparqlQuery(endpoint.slice(0, slash), endpoint.slice(slash + 1), q);
  }

  $: filteredHistory = historyFilter
    ? history.filter(h => h.q.toLowerCase().includes(historyFilter.toLowerCase()))
    : history;

  // Endpoint options. Each item: { value, label, group? }
  // value formats:
  //   'main'                   — main store, all graphs
  //   'dataset-default'        — current page's dataset (when datasetId prop set)
  //   'dataset:<id>'           — all graphs of dataset <id> via FROM clauses
  //   '<dsId>/<svcSlug>'       — a configured service endpoint of that dataset
  let endpoints = [];
  let selectedEndpoint = '';
  let datasetServices = {}; // datasetId -> services[] (used for option building)
  let datasetGraphIris = []; // graph IRIs for dataset-default scoped queries
  let datasetGraphsById = {}; // dsId -> graph IRIs (for 'dataset:<id>' endpoints)

  // ── Dataset / organisation scope (ported from the Triple Browser) ──────────
  // Global-editor affordance. When non-empty, the scope supersedes the endpoint
  // selector: the query runs as a FROM-union over the *live* graphs of every
  // dataset in scope (dataset chips + every dataset owned by a scoped org). When
  // the scope resolves to exactly one dataset, the header version <select> can
  // pin it to a snapshot (routed through that dataset's service endpoint, which
  // is the only path that authorises version-snapshot graphs).
  let scopeItems = [];      // Array<{type:'dataset'|'org', id, name}>
  let allDatasets = [];     // populated for the scope picker (global editor)
  let allOrgs = [];
  let scopePickerOpen = false;
  let scopeSearch = '';

  // Dataset IDs in scope: explicit dataset chips + every dataset owned by a
  // scoped organisation.
  $: scopedDatasetIds = (() => {
    const ids = new Set();
    for (const s of scopeItems) {
      if (s.type === 'dataset') ids.add(s.id);
      else if (s.type === 'org') {
        for (const d of allDatasets) {
          if (d.owner_type === 'organisation' && String(d.owner_id) === String(s.id)) ids.add(d.id);
        }
      }
    }
    return [...ids];
  })();
  // A dataset/org-scoped editor instance already has a fixed context, so the
  // scope bar only appears (and only drives execution) in the global editor.
  $: scopeActive = !datasetId && !orgId && scopeItems.length > 0;

  // Union of the live graph IRIs of every dataset in scope, for FROM injection.
  function scopedFromIris() {
    const iris = new Set();
    for (const id of scopedDatasetIds) {
      for (const iri of (datasetGraphsById[id] ?? [])) iris.add(iri);
    }
    return [...iris];
  }

  // ── Named-graph scope: which datasets' graphs are selectable ────────────────
  // Works in all three contexts: a locked dataset (its own graphs), a locked org
  // (its datasets' graphs), or the global editor (every dataset in scope).
  $: graphScopeDatasets = (() => {
    if (datasetId) return [{ id: datasetId, name: contextName ?? datasetId }];
    if (orgId) {
      return allDatasets
        .filter(d => d.owner_type === 'organisation' && String(d.owner_id) === String(orgId))
        .map(d => ({ id: d.id, name: d.name ?? d.id }));
    }
    return scopedDatasetIds.map(id => {
      const d = allDatasets.find(x => String(x.id) === String(id));
      return { id, name: d?.name ?? id };
    });
  })();
  $: graphScopeDatasetIds = graphScopeDatasets.map(d => String(d.id));

  // Lazily fetch the full graph list (graph_iri/private/triple_count) of every
  // dataset that became selectable, so the GRAPHS section can list named graphs.
  $: ensureScopeGraphs(graphScopeDatasetIds);
  async function ensureScopeGraphs(ids) {
    const missing = ids.filter(id => !(id in datasetGraphObjsById));
    if (!missing.length) return;
    graphScopeLoading = true;
    await Promise.all(missing.map(async (id) => {
      try {
        datasetGraphObjsById[id] = await listDatasetGraphs(id);
      } catch {
        datasetGraphObjsById[id] = [];
      }
    }));
    datasetGraphObjsById = { ...datasetGraphObjsById };
    graphScopeLoading = false;
  }

  // Flattened, de-duplicated list of selectable named graphs across scoped
  // datasets: [{ graph_iri, datasets: [name], private, triple_count }].
  $: availableGraphs = (() => {
    const byIri = new Map();
    for (const ds of graphScopeDatasets) {
      for (const g of (datasetGraphObjsById[String(ds.id)] ?? [])) {
        if (!g?.graph_iri) continue;
        const existing = byIri.get(g.graph_iri);
        if (existing) existing.datasets.push(ds.name);
        else byIri.set(g.graph_iri, { graph_iri: g.graph_iri, datasets: [ds.name], private: g.private, triple_count: g.triple_count });
      }
    }
    return [...byIri.values()];
  })();
  // Whether the GRAPHS section is shown at all: only once a dataset is in scope.
  $: graphScopeAvailable = graphScopeDatasetIds.length > 0;

  // Prune selections that left the available set (scope shrank). Only once the
  // in-scope datasets' graphs have actually been fetched, so a restored selection
  // isn't wiped while availableGraphs is still empty mid-load.
  $: graphScopeResolved = !graphScopeLoading && graphScopeDatasetIds.every(id => id in datasetGraphObjsById);
  $: if (graphScopeResolved && selectedGraphIris.size) {
    const avail = new Set(availableGraphs.map(g => g.graph_iri));
    const kept = [...selectedGraphIris].filter(iri => avail.has(iri));
    if (kept.length !== selectedGraphIris.size) selectedGraphIris = new Set(kept);
  }

  function toggleGraphScope(iri) {
    if (selectedGraphIris.has(iri)) selectedGraphIris.delete(iri);
    else selectedGraphIris.add(iri);
    selectedGraphIris = new Set(selectedGraphIris);
  }
  $: selectedGraphList = [...selectedGraphIris];

  function addScopeItem(item) {
    if (!scopeItems.some(s => s.type === item.type && s.id === item.id)) {
      scopeItems = [...scopeItems, item];
    }
    scopePickerOpen = false;
    scopeSearch = '';
  }
  function removeScopeItem(item) {
    scopeItems = scopeItems.filter(s => !(s.type === item.type && s.id === item.id));
  }
  function clearDatasetScope() { scopeItems = []; selectedGraphIris = new Set(); }

  // ── Named-graph scope (folded into the scope filter) ───────────────────────
  // When one or more datasets are in scope (explicit chips, an org's datasets, or
  // the locked dataset/org context), the user can narrow execution to specific
  // named graphs. Selected IRIs are injected as FROM clauses in executeQuery.
  let datasetGraphObjsById = {}; // dsId -> DatasetGraph[] ({graph_iri, private, triple_count})
  let selectedGraphIris = new Set(); // user-chosen named-graph IRIs (empty = all in-scope graphs)
  let graphScopeOpen = false; // the graphs picker popover
  let graphScopeLoading = false;

  // ── Version scoping ──────────────────────────────────────────────────────
  // A query can target a dataset's version snapshot instead of its live data.
  // The "in-scope dataset" depends on the selected endpoint (a single dataset
  // for dataset-default / dataset:<id> / a service endpoint; none for main).
  let selectedVersion = '';            // '' = live (current)
  let versionsByDataset = {};          // dsId -> DatasetVersion[]

  $: scopeDatasetId = (() => {
    // The scope bar wins when it pins to exactly one dataset, so the version
    // <select> and version-scoped execution target that dataset. Multi-dataset
    // scope has no single version target (→ null, FROM-union path instead).
    if (scopeActive) return scopedDatasetIds.length === 1 ? scopedDatasetIds[0] : null;
    const ep = selectedEndpoint;
    if (!ep || ep === 'main') return null;
    if (ep === 'dataset-default') return datasetId;
    if (ep.startsWith('dataset:')) return ep.slice('dataset:'.length);
    const slash = ep.indexOf('/');
    return slash > 0 ? ep.slice(0, slash) : null;
  })();

  $: if (scopeDatasetId) ensureVersions(scopeDatasetId);

  async function ensureVersions(dsId) {
    if (versionsByDataset[dsId]) return;
    try {
      const vs = await listDatasetVersions(dsId);
      versionsByDataset = { ...versionsByDataset, [dsId]: vs || [] };
    } catch {
      versionsByDataset = { ...versionsByDataset, [dsId]: [] };
    }
  }

  function verParts(v) {
    return String(v?.version ?? '').replace(/^v/i, '').split('.').map(n => parseInt(n, 10) || 0);
  }
  $: scopeVersions = (scopeDatasetId ? (versionsByDataset[scopeDatasetId] || []) : [])
    .slice()
    .sort((a, b) => {
      const pa = verParts(a), pb = verParts(b);
      for (let i = 0; i < Math.max(pa.length, pb.length); i++) {
        const x = pa[i] ?? 0, y = pb[i] ?? 0;
        if (x !== y) return y - x;
      }
      return 0;
    });

  $: versionActive = !!selectedVersion && !['live', 'latest', 'current'].includes(selectedVersion);

  // Reset the version pin whenever the in-scope dataset changes, so a version
  // never silently carries over to a different dataset.
  let _prevScopeDs = null;
  $: if (scopeDatasetId !== _prevScopeDs) { _prevScopeDs = scopeDatasetId; selectedVersion = ''; }

  // The service slug to route a version-scoped query through. Prefers the
  // selected service endpoint's slug, else a known service, else the
  // auto-provisioned default "sparql" service.
  function serviceSlugFor(dsId) {
    const ep = selectedEndpoint;
    if (ep && ep.includes('/')) {
      const slash = ep.indexOf('/');
      if (ep.slice(0, slash) === dsId) return ep.slice(slash + 1);
    }
    const svcs = datasetServices[dsId];
    if (svcs && svcs.length) return svcs[0].slug;
    return 'sparql';
  }

  onMount(async () => {
    // Track whether an explicit source (handoff / URL) already set the query or
    // scope, so the sessionStorage restore below doesn't override an intentful
    // deep-link or "open in editor" handoff.
    let queryFromExternal = false;
    let scopeFromUrl = false;
    // A saved query opened "in the SPARQL editor" hands its text off here so
    // anyone (including anonymous users on public data) can edit and run it.
    try {
      const loaded = sessionStorage.getItem('ots_sparql_load');
      if (loaded) {
        sessionStorage.removeItem('ots_sparql_load');
        query = loaded;
        queryFromExternal = true;
      } else {
        // Call sites like TripleBrowser's "Open in editor" hand the query off via
        // ?query= rather than the sessionStorage path, so honour it here too.
        const urlQuery = new URLSearchParams(window.location.search).get('query');
        if (urlQuery) { query = urlQuery; queryFromExternal = true; }
      }
    } catch {}
    // A return link (set when opened from an API service) lets the user get back
    // to that service with it re-expanded.
    try {
      const sp = new URLSearchParams(window.location.search);
      const ret = sp.get('return');
      if (ret) { returnTo = ret; returnFrom = sp.get('from') || 'API service'; }
    } catch {}
    // Global editor: seed the scope bar from ?dataset / ?org deep-links so the
    // Triple Browser's "Open in SPARQL" can carry the active scope across.
    try {
      if (!datasetId && !orgId) {
        const sp2 = new URLSearchParams(window.location.search);
        const dsParam = sp2.get('dataset');
        const orgParam = sp2.get('org');
        if (dsParam) {
          scopeItems = [{ type: 'dataset', id: dsParam, name: dsParam }];
          scopeFromUrl = true;
          getDataset(dsParam)
            .then(d => { scopeItems = scopeItems.map(s => (s.type === 'dataset' && s.id === dsParam) ? { ...s, name: d?.name ?? dsParam } : s); })
            .catch(() => {});
        } else if (orgParam) {
          scopeItems = [{ type: 'org', id: orgParam, name: orgParam }];
          scopeFromUrl = true;
          getOrganisation(orgParam)
            .then(o => { scopeItems = scopeItems.map(s => (s.type === 'org' && s.id === orgParam) ? { ...s, name: o?.name ?? orgParam } : s); })
            .catch(() => {});
        }
      }
    } catch {}
    // Restore the editor's working state for this route, but never clobber a
    // query/scope that an explicit handoff or deep-link just set above.
    try {
      const saved = JSON.parse(sessionStorage.getItem(stateKey) || 'null');
      if (saved) {
        if (!queryFromExternal && typeof saved.query === 'string') query = saved.query;
        if (!scopeFromUrl && !datasetId && !orgId && Array.isArray(saved.scopeItems)) scopeItems = saved.scopeItems;
        if (Array.isArray(saved.selectedGraphs)) selectedGraphIris = new Set(saved.selectedGraphs);
        if (saved.activeTab) activeTab = saved.activeTab;
      }
    } catch {}
    persistReady = true;
    // Probe LLM service availability for the health badge (best-effort, non-blocking).
    llmHealth().then(s => { llmStatus = s; }).catch(() => {});
    // Load query history (migrate legacy string[] format)
    try {
      const stored = localStorage.getItem(HISTORY_KEY);
      if (stored) history = JSON.parse(stored);
      if (!history.length) {
        const legacy = localStorage.getItem('sparql_query_history');
        if (legacy) {
          const arr = JSON.parse(legacy);
          history = arr.map(q => ({ q, t: Date.now() }));
        }
      }
    } catch {}
    window.addEventListener('click', handleOutsideClick);
    window.addEventListener('keydown', handleKey);

    // Load endpoint options depending on context
    try {
      let relevantDatasets = [];
      let orgsById = {};

      if (datasetId) {
        // Dataset-scoped: only services for this one dataset
        // Also pre-fetch graph IRIs for the fallback "all graphs" endpoint
        try {
          const graphs = await listDatasetGraphs(datasetId);
          datasetGraphIris = graphs.map(g => g.graph_iri).filter(Boolean);
        } catch {}
        const ds = { id: datasetId, name: datasetId };
        relevantDatasets = [ds];
      } else if (orgId) {
        // Org-scoped: only datasets owned by this org
        const all = await listDatasets();
        relevantDatasets = all.filter(
          (d) => d.owner_type === 'organisation' && d.owner_id === orgId
        );
        allDatasets = relevantDatasets; // lets the GRAPHS scope section enumerate the org's datasets
      } else {
        // Global: all accessible datasets, plus the user's organisations for grouping labels.
        const [dsList, orgs] = await Promise.all([
          listDatasets(),
          listOrganisations().catch(() => []),
        ]);
        relevantDatasets = dsList;
        allDatasets = dsList;      // for the scope picker
        allOrgs = orgs ?? [];      // for the scope picker
        for (const o of orgs ?? []) orgsById[o.id] = o;
      }

      // Fetch services and graph IRIs for every accessible dataset in parallel
      await Promise.all(relevantDatasets.map(async (ds) => {
        const [svcs, graphs] = await Promise.all([
          listServices(ds.id).catch(() => []),
          listDatasetGraphs(ds.id).catch(() => []),
        ]);
        if (svcs?.length) datasetServices[ds.id] = svcs;
        const iris = (graphs ?? []).map(g => g.graph_iri).filter(Boolean);
        if (iris.length) datasetGraphsById[ds.id] = iris;
      }));
      datasetGraphsById = { ...datasetGraphsById };

      // Build endpoint list with optgroup-style grouping
      const built = [];
      if (!datasetId && !orgId) {
        built.push({ value: 'main', label: $i18nT('pages.sparql.defaultEndpoint'), group: 'Global' });
      }
      if (datasetId && datasetGraphIris.length > 0) {
        built.push({ value: 'dataset-default', label: $i18nT('pages.sparql.datasetEndpoint') });
      }

      // Sort datasets so org-owned ones group together by org name, then user-owned
      const sortedDatasets = [...relevantDatasets].sort((a, b) => {
        const ag = a.owner_type === 'organisation'
          ? (orgsById[a.owner_id]?.name ?? a.owner_id)
          : '￿'; // user-owned sorts last
        const bg = b.owner_type === 'organisation'
          ? (orgsById[b.owner_id]?.name ?? b.owner_id)
          : '￿';
        if (ag !== bg) return ag.localeCompare(bg);
        return (a.name ?? a.id).localeCompare(b.name ?? b.id);
      });

      for (const ds of sortedDatasets) {
        const dsName = ds.name ?? ds.id;
        const groupName = (!datasetId && !orgId)
          ? (ds.owner_type === 'organisation'
              ? (orgsById[ds.owner_id]?.name ?? `Org ${ds.owner_id}`)
              : 'Personal')
          : undefined;
        const iris = datasetGraphsById[ds.id];
        if (iris && iris.length > 0) {
          built.push({
            value: `dataset:${ds.id}`,
            label: `${dsName} (all graphs)`,
            group: groupName,
          });
        }
        const svcs = datasetServices[ds.id] ?? [];
        for (const svc of svcs) {
          built.push({
            value: `${ds.id}/${svc.slug}`,
            label: `${dsName} › ${svc.name ?? svc.slug}`,
            group: groupName,
          });
        }
      }

      // If context-scoped and nothing was discoverable, fall back to main
      if (built.length === 0) {
        built.push({ value: 'main', label: $i18nT('pages.sparql.defaultEndpoint') });
      }
      endpoints = built;

      // The scope bar replaced the endpoint dropdown, so routing now starts
      // deterministically: the whole store in the global editor (the scope bar
      // narrows it), or the context's default endpoint (all-graphs / version-picker
      // source) in a dataset/organisation-scoped editor.
      selectedEndpoint = (!datasetId && !orgId) ? 'main' : (endpoints[0]?.value ?? 'main');

      // Carry a ?version= param (from an explore-tile link) into the version
      // picker for the page's dataset. Set last so the scope-change reset above
      // doesn't clobber it.
      const urlVersion = new URLSearchParams(window.location.search).get('version');
      if (urlVersion && datasetId) {
        await ensureVersions(datasetId);
        selectedVersion = urlVersion;
      } else if (urlVersion && scopedDatasetIds.length === 1) {
        // ?version= alongside a single ?dataset= scope chip pins that dataset.
        await ensureVersions(scopedDatasetIds[0]);
        selectedVersion = urlVersion;
      }
    } catch {}
  });

  // Insert FROM <iri> clauses just before the WHERE keyword in a SPARQL query.
  // SPARQL grammar requires DatasetClause after SelectClause and before WhereClause.
  function injectFromClauses(q, graphIris) {
    if (!graphIris.length) return q;
    const fromBlock = graphIris.map(g => `FROM <${g}>`).join('\n') + '\n';
    // Match WHERE followed by optional whitespace and an opening brace (case-insensitive)
    const whereMatch = q.match(/\bWHERE\s*\{/i);
    if (whereMatch) {
      return q.slice(0, whereMatch.index) + fromBlock + q.slice(whereMatch.index);
    }
    // Fallback: prepend (shouldn't be needed for valid queries)
    return fromBlock + q;
  }

  async function executeQuery() {
    if (!query.trim()) return;
    loading = true;
    error = '';
    results = null;
    const start = performance.now();
    // Default to main if endpoint selection hasn't loaded yet
    const endpoint = selectedEndpoint || 'main';
    // A named-graph selection in the scope filter narrows execution to exactly
    // those graphs (FROM-injected), superseding the broader endpoint/scope set.
    const graphSubset = selectedGraphList;
    try {
      if (versionActive && scopeDatasetId) {
        // Version-scoped: route through the dataset service endpoint, which
        // resolves the version's snapshot graphs server-side (a graph subset is
        // ignored here — the version pin defines the graph set).
        results = await datasetSparqlQuery(scopeDatasetId, serviceSlugFor(scopeDatasetId), query, selectedVersion);
      } else if (graphSubset.length > 0) {
        // Explicit named-graph selection (any context): FROM-union over exactly
        // the chosen graphs. These are live graphs, so the main /sparql endpoint
        // (which authorises live data via FROM) resolves them.
        results = await sparqlQuery(injectFromClauses(query, graphSubset));
      } else if (scopeActive) {
        // Scope bar: FROM-union over the live graphs of every dataset in scope.
        // (A single dataset pinned to a version is handled by the branch above.)
        const iris = scopedFromIris();
        // No graphs in scope → query a non-existent graph so the result is empty
        // rather than silently falling back to the whole store.
        const scoped = injectFromClauses(query, iris.length ? iris : ['urn:ots:empty-scope']);
        results = await sparqlQuery(scoped);
      } else if (endpoint === 'main') {
        results = await sparqlQuery(query);
      } else if (endpoint === 'dataset-default') {
        // Scope query to dataset graphs by injecting FROM clauses
        const scopedQuery = injectFromClauses(query, datasetGraphIris);
        results = await sparqlQuery(scopedQuery);
      } else if (endpoint.startsWith('dataset:')) {
        const dsId = endpoint.slice('dataset:'.length);
        const iris = datasetGraphsById[dsId] ?? [];
        results = await sparqlQuery(injectFromClauses(query, iris));
      } else {
        const slash = endpoint.indexOf('/');
        const dsId = endpoint.slice(0, slash);
        const svcSlug = endpoint.slice(slash + 1);
        results = await datasetSparqlQuery(dsId, svcSlug, query);
      }
      elapsed = Math.round(performance.now() - start);
      addToHistory(query);
      // Running a generated query is positive feedback — approve if unchanged, edit if the
      // user tweaked it before running. Feeds the sparql track's training loop.
      if (lastGenerated) {
        const edited = query.trim() !== lastGenerated.sparql.trim();
        sendLlmFeedback(buildSparqlSignal(edited ? 'edit' : 'approve', edited ? query.trim() : null, nlReviewed));
        lastGenerated = null;
        nlComment = '';
        nlReviewed = null;
      }
      // Convert CONSTRUCT/DESCRIBE graph results to tabular bindings
      if (results?._graphResult) {
        results = parseNTriplesToBindings(results.ntriples);
        activeTab = 'graph';
      } else {
        activeTab = 'table';
      }
    } catch (e) {
      error = e.message;
    } finally {
      loading = false;
    }
  }

  function addToHistory(q) {
    const trimmed = q.trim();
    history = [{ q: trimmed, t: Date.now() }, ...history.filter(h => h.q !== trimmed)].slice(0, MAX_HISTORY);
    try { localStorage.setItem(HISTORY_KEY, JSON.stringify(history)); } catch {}
  }

  function loadFromHistory(q) {
    query = q;
    showHistory = false;
  }

  onDestroy(() => {
    if (typeof window !== 'undefined') {
      window.removeEventListener('click', handleOutsideClick);
      window.removeEventListener('keydown', handleKey);
    }
  });

  function exportCsv() {
    if (!results?.results) return;
    downloadFile(resultsToCsv(results), 'sparql-results.csv', 'text/csv');
  }

  function exportJson() {
    if (!results) return;
    downloadFile(JSON.stringify(results, null, 2), 'sparql-results.json', 'application/json');
  }

  // Compute graph elements for CONSTRUCT results or SELECT with ?s ?p ?o
  $: graphElements = (() => {
    if (!results) return { nodes: [], edges: [] };
    if (results.results?.bindings) {
      const vars = results.head?.vars || [];
      const hasGraph = vars.includes('s') && vars.includes('p') && vars.includes('o');
      if (hasGraph) return graphResultsToElements(results.results.bindings);
    }
    return { nodes: [], edges: [] };
  })();

  $: canGraph = graphElements.nodes.length > 0;

  $: rowCount = results?.results?.bindings?.length ?? 0;
</script>

<div class="sparql-page">
  <PageHeader
    title={$i18nT('pages.sparql.editor')}
    breadcrumbs={orgId
      ? [{ label: $i18nT('pages.sparql.breadcrumbOrganisations'), href: '/organisations' }, { label: contextName ?? '…', href: '/organisations/' + orgId }, { label: 'SPARQL' }]
      : datasetId
        ? [{ label: $i18nT('pages.sparql.breadcrumbDatasets'), href: '/datasets' }, { label: contextName ?? '…', href: '/datasets/' + datasetId }, { label: 'SPARQL' }]
        : [{ label: $i18nT('pages.sparql.breadcrumbDatasets'), href: '/datasets' }, { label: 'SPARQL' }]}
  />
  <!-- Editor panel -->
  <div class="card editor-card">
    {#if returnTo}
      <!-- Opened from a saved API service: surface what we're editing + a way back. -->
      <div class="svc-context">
        <FileCode size={15} />
        <span class="svc-context-text">{$i18nT('pages.sparql.editingApiService')} <strong>{returnFrom}</strong></span>
        <button class="btn btn-sm btn-ghost svc-context-back" on:click={() => navigate(returnTo)} title={$i18nT('pages.sparql.backToServiceTitle')}>
          ← {$i18nT('pages.sparql.backToService')}
        </button>
      </div>
    {/if}
    <div class="editor-header">
      <h2>{$i18nT('pages.sparql.editor')}</h2>
      <div class="header-actions">
        {#if scopeVersions.length > 0}
          <Select
            bind:value={selectedVersion}
            size="sm"
            class="endpoint-select version-select {versionActive ? 'version-active' : ''}"
            title={$i18nT('pages.sparql.versionSelectTitle')}
            options={[{ value: '', label: $i18nT('pages.sparql.liveCurrent') }, ...scopeVersions.map((v) => ({ value: v.version, label: `v${v.version}${v.status && v.status !== 'published' ? ` · ${v.status}` : ''}` }))]}
          />
        {/if}

        {#if datasetId}
          {#if canWriteDataset}
            <button class="btn btn-sm btn-ghost" title={$i18nT('pages.sparql.saveAsApiServiceTitle')} on:click={saveAsSavedQuery}>
              <BookOpen size={14} /> {$i18nT('pages.sparql.saveAsApiService')}
            </button>
          {/if}
          <Link to={`/datasets/${datasetId}/api-services`} class="btn btn-sm btn-ghost">{$i18nT('pages.sparql.apiServices')}</Link>
        {/if}

        <!-- Templates -->
        <div class="templates-wrapper">
          <button class="btn btn-sm btn-ghost" on:click|stopPropagation={() => { showTemplates = !showTemplates; showHistory = false; }} title={$i18nT('pages.sparql.queryTemplates')}>
            <BookOpen size={14} /> {$i18nT('pages.sparql.templates')}
          </button>
          {#if showTemplates}
            <div class="dropdown templates-dropdown">
              {#each TEMPLATES as tpl}
                <button class="dd-item" on:click={() => useTemplate(tpl)}>
                  <div class="dd-title">{tpl.name}</div>
                  <div class="dd-sub">{tpl.body.split('\n').slice(-1)[0].trim()}</div>
                </button>
              {/each}
            </div>
          {/if}
        </div>

        <!-- History -->
        <div class="history-wrapper">
          <button class="btn btn-sm btn-ghost" on:click|stopPropagation={() => { showHistory = !showHistory; showTemplates = false; }} title={$i18nT('pages.sparql.history')}>
            <Clock size={14} /> {$i18nT('pages.sparql.history')} ({history.length})
          </button>
          {#if showHistory}
            <div class="dropdown history-dropdown" role="presentation" on:click|stopPropagation on:keydown|stopPropagation>
              <div class="dd-head">
                <input type="search" bind:value={historyFilter} placeholder={$i18nT('pages.sparql.filterHistory')} class="dd-filter" />
                {#if history.length > 0}
                  <button class="btn btn-sm btn-ghost" on:click={clearHistory} title={$i18nT('pages.sparql.clearAll')}>
                    <Trash2 size={12} /> {$i18nT('system.clear')}
                  </button>
                {/if}
              </div>
              {#if filteredHistory.length === 0}
                <div class="history-empty">{$i18nT('pages.sparql.noQueriesYet')}</div>
              {:else}
                {#each filteredHistory as h, i (h.t)}
                  <div class="history-item" on:click={() => loadFromHistory(h.q)} role="button" tabindex="0"
                       on:keydown={(e) => e.key === 'Enter' && loadFromHistory(h.q)}>
                    <div class="history-meta">
                      <span class="history-time">{relativeTime(h.t)}</span>
                      <button class="icon-btn" on:click={(e) => deleteHistoryItem(i, e)} title={$i18nT('system.delete')}>
                        <XIcon size={11} />
                      </button>
                    </div>
                    <pre class="history-preview">{h.q.split('\n').slice(0, 4).join('\n')}{h.q.split('\n').length > 4 ? '\n…' : ''}</pre>
                  </div>
                {/each}
              {/if}
            </div>
          {/if}
        </div>
      </div>
    </div>

    <!-- Scope bar — the single control for what the query runs against. Editable in
         the global editor (a FROM-union over the chosen datasets/orgs); a locked chip
         showing the fixed context in a dataset/organisation-scoped editor. -->
    <div class="dataset-scope-bar">
      <LayoutList size={13} />
      <span class="dataset-scope-label">{$i18nT('pages.sparql.scopeLabel')}</span>
      {#if datasetId}
        <span class="dataset-scope-chip locked" title={$i18nT('pages.sparql.lockedToDataset', { values: { name: contextName ?? datasetId } })}>
          <Database size={11} /> {contextName ?? datasetId} <Lock size={10} />
        </span>
      {:else if orgId}
        <span class="dataset-scope-chip locked" title={$i18nT('pages.sparql.lockedToOrg', { values: { name: contextName ?? orgId } })}>
          <Building2 size={11} /> {contextName ?? orgId} <Lock size={10} />
        </span>
      {:else}
        {#each scopeItems as item}
          <span class="dataset-scope-chip">
            {#if item.type === 'org'}<Building2 size={11} />{:else}<Database size={11} />{/if}
            {item.name}
            <button class="dataset-scope-x" on:click={() => removeScopeItem(item)} title={$i18nT('pages.sparql.removeScope')}><XIcon size={11} /></button>
          </span>
        {/each}
        <div class="scope-picker-wrap">
          <button class="scope-add-btn" on:click|stopPropagation={() => { scopePickerOpen = !scopePickerOpen; scopeSearch = ''; }}>
            <Plus size={12} /> {$i18nT('system.add')}
          </button>
          {#if scopePickerOpen}
            <div class="scope-picker" role="presentation" on:click|stopPropagation on:keydown|stopPropagation>
              <input class="scope-search" placeholder={$i18nT('pages.sparql.scopeSearchPlaceholder')} bind:value={scopeSearch} use:autofocus />
              {#if allOrgs.filter(o => !scopeItems.some(s => s.type === 'org' && s.id === o.id) && (!scopeSearch || (o.name || '').toLowerCase().includes(scopeSearch.toLowerCase()))).length > 0}
                <div class="scope-group-label">{$i18nT('pages.sparql.organisations')}</div>
                {#each allOrgs.filter(o => !scopeItems.some(s => s.type === 'org' && s.id === o.id) && (!scopeSearch || (o.name || '').toLowerCase().includes(scopeSearch.toLowerCase()))) as org}
                  <button class="scope-item" on:click={() => addScopeItem({ type: 'org', id: org.id, name: org.name })}>
                    <Building2 size={12} /> {org.name}
                  </button>
                {/each}
              {/if}
              {#if allDatasets.filter(d => !scopeItems.some(s => s.type === 'dataset' && s.id === d.id) && (!scopeSearch || (d.name || d.id).toLowerCase().includes(scopeSearch.toLowerCase()))).length > 0}
                <div class="scope-group-label">{$i18nT('pages.sparql.datasets')}</div>
                {#each allDatasets.filter(d => !scopeItems.some(s => s.type === 'dataset' && s.id === d.id) && (!scopeSearch || (d.name || d.id).toLowerCase().includes(scopeSearch.toLowerCase()))) as ds}
                  <button class="scope-item" on:click={() => addScopeItem({ type: 'dataset', id: ds.id, name: ds.name || ds.id })}>
                    <Database size={12} /> {ds.name || ds.id}
                  </button>
                {/each}
              {/if}
              {#if allOrgs.length === 0 && allDatasets.length === 0}
                <div class="scope-empty">{$i18nT('system.loading')}</div>
              {:else if allOrgs.filter(o => !scopeItems.some(s => s.type === 'org' && s.id === o.id) && (!scopeSearch || (o.name || '').toLowerCase().includes(scopeSearch.toLowerCase()))).length === 0 && allDatasets.filter(d => !scopeItems.some(s => s.type === 'dataset' && s.id === d.id) && (!scopeSearch || (d.name || d.id).toLowerCase().includes(scopeSearch.toLowerCase()))).length === 0}
                <div class="scope-empty">{$i18nT('pages.sparql.noResultsShort')}</div>
              {/if}
            </div>
          {/if}
        </div>
        {#if scopeItems.length > 0}
          <button class="scope-clear-all" on:click={clearDatasetScope}>{$i18nT('pages.sparql.clearAll')}</button>
        {:else}
          <span class="scope-hint">{$i18nT('pages.sparql.allDatasetsHint')}</span>
        {/if}
      {/if}

      <!-- Named-graph subset of the scope. Shown once a dataset is in scope
           (explicit chips, an org's datasets, or the locked dataset/org context).
           Selected graphs narrow execution to exactly those graphs. -->
      {#if graphScopeAvailable}
        <span class="graph-scope-sep" aria-hidden="true"></span>
        <Layers size={13} />
        {#each selectedGraphList as iri}
          <span class="dataset-scope-chip graph-chip" title={iri}>
            {iri.replace(/^https?:\/\//, '')}
            <button class="dataset-scope-x" on:click={() => toggleGraphScope(iri)} title={$i18nT('pages.sparql.removeScope')}><XIcon size={11} /></button>
          </span>
        {/each}
        <div class="graph-scope-wrap">
          <button class="scope-add-btn" on:click|stopPropagation={() => { graphScopeOpen = !graphScopeOpen; scopePickerOpen = false; }}>
            {#if graphScopeLoading}<Loader2 size={12} class="animate-spin" />{:else}<Layers size={12} />{/if}
            {$i18nT('pages.sparql.graphs')}{selectedGraphIris.size > 0 ? ` (${selectedGraphIris.size})` : ''}
          </button>
          {#if graphScopeOpen}
            <div class="scope-picker graph-scope-picker" role="presentation" on:click|stopPropagation on:keydown|stopPropagation>
              {#if availableGraphs.length === 0}
                <div class="scope-empty">{graphScopeLoading ? $i18nT('system.loading') : $i18nT('pages.sparql.noResultsShort')}</div>
              {:else}
                {#each availableGraphs as g}
                  <label class="graph-scope-item">
                    <input type="checkbox" checked={selectedGraphIris.has(g.graph_iri)} on:change={() => toggleGraphScope(g.graph_iri)} />
                    <code class="graph-scope-iri" title={g.graph_iri}>{g.graph_iri}</code>
                  </label>
                {/each}
              {/if}
            </div>
          {/if}
        </div>
        {#if selectedGraphIris.size > 0}
          <button class="scope-clear-all" on:click={() => selectedGraphIris = new Set()}>{$i18nT('pages.sparql.clearAll')}</button>
        {/if}
      {/if}
    </div>

    <!-- Natural-language → SPARQL -->
    <div class="nl-box">
      <Wand2 size={16} class="nl-icon" />
      {#if llmStatus}
        <span class="nl-status" class:offline={!llmStatus.reachable}
          title={llmStatus.reachable ? $i18nT('pages.sparql.llmOnline', { values: { gateway: llmStatus.gateway } }) : $i18nT('pages.sparql.llmOffline', { values: { gateway: llmStatus.gateway } })}>
          {llmStatus.reachable ? '● LLM' : '○ LLM'}
        </span>
      {/if}
      <input
        class="nl-input"
        type="text"
        bind:value={nlQuestion}
        disabled={llmStatus && !llmStatus.reachable}
        placeholder={llmStatus && !llmStatus.reachable ? $i18nT('pages.sparql.nlPlaceholderOffline') : $i18nT('pages.sparql.nlPlaceholder')}
        on:keydown={(e) => { if (e.key === 'Enter') { e.preventDefault(); generateFromNl(); } }}
      />
      <button class="btn btn-sm" on:click={generateFromNl} disabled={nlLoading || !nlQuestion.trim()}>
        {#if nlLoading}<Loader2 size={14} class="animate-spin" /> {$i18nT('pages.sparql.generating')}{:else}<Wand2 size={14} /> {$i18nT('pages.sparql.generate')}{/if}
      </button>
      {#if lastGenerated}
        <span class="nl-generated">
          <Check size={13} /> {$i18nT('pages.sparql.generatedHint')}
          <button class="nl-thumb" class:up={nlReviewed === 'up'} on:click={() => reviewGenerated('up')}
            title={$i18nT('pages.sparql.goodQuery')}>
            <ThumbsUp size={13} />
          </button>
          <button class="nl-thumb" class:down={nlReviewed === 'down'} on:click={() => reviewGenerated('down')}
            title={$i18nT('pages.sparql.poorQuery')}>
            <ThumbsDown size={13} />
          </button>
        </span>
      {/if}
    </div>
    {#if lastGenerated}
      <input class="nl-comment" type="text" bind:value={nlComment}
        placeholder={$i18nT('pages.sparql.nlCommentPlaceholder')} />
    {/if}
    {#if nlError}
      <div class="nl-error">{nlError}</div>
    {/if}

    <!-- Prefixes bar — declared chips (with remove), missing chips (with
         "Add missing"), and a search affordance to add any prefix/vocabulary.
         Sits between the AI input and the editor so prefix wrangling reads
         top-to-bottom with the query. -->
    <div class="prefix-bar">
      <FileCode size={13} class="prefix-bar-icon" />
      {#each Object.entries(declaredPrefixes) as [p, ns]}
        <span class="chip chip-ok" title={ns}>
          {p}:
          <button class="chip-x" on:click={() => removePrefix(p)} title={$i18nT('pages.sparql.removePrefix')}>×</button>
        </span>
      {/each}
      {#each missingPrefixes as p}
        <span class="chip chip-miss" title={$i18nT('pages.sparql.notDeclared')}>{p}:?</span>
      {/each}
      {#if missingPrefixes.length > 0}
        <button class="btn btn-sm btn-ghost" on:click={insertMissingPrefixes} title={$i18nT('pages.sparql.insertMissingPrefixes')}>
          <FileCode size={12} /> {$i18nT('pages.sparql.addMissing', { values: { count: missingPrefixes.length } })}
        </button>
      {/if}
      <!-- Add-prefix affordance (richer search across built-ins / prefix.cc / platform). -->
      <div class="prefix-add-wrap">
        <button class="scope-add-btn" on:click|stopPropagation={() => { prefixSearchOpen = !prefixSearchOpen; }} title={$i18nT('pages.sparql.insertMissingPrefixes')}>
          <Plus size={12} /> {$i18nT('system.add')}
        </button>
        {#if prefixSearchOpen}
          <div class="prefix-search-pop" role="presentation" on:click|stopPropagation on:keydown|stopPropagation>
            <PrefixSearchPanel
              existing={Object.keys(declaredPrefixes)}
              autofocus
              on:add={(e) => addPrefixFromSearch(e.detail.prefix, e.detail.namespace)}
            />
          </div>
        {/if}
      </div>
    </div>

    <SparqlEditorCM
      bind:query
      on:execute={executeQuery}
      height="260px"
      lint
      sparqlFetcher={hoverFetcher}
      graphIris={selectedEndpoint === 'dataset-default' ? datasetGraphIris : null}
    />

    <div class="toolbar">
      <button class="btn" on:click={executeQuery} disabled={loading}>
        {#if loading}<Loader2 size={14} class="animate-spin" /> {$i18nT('pages.sparql.running')}{:else}<Play size={14} /> {$i18nT('pages.sparql.execute')}{/if}
        <kbd>Ctrl+Enter</kbd>
      </button>
      {#if elapsed > 0}
        <span class="elapsed">{elapsed}ms · {rowCount} {rowCount !== 1 ? $i18nT('pages.sparql.rows') : $i18nT('pages.sparql.row')}</span>
      {/if}
    </div>
  </div>

  <!-- Error -->
  {#if error}
    <div class="card error-card">
      <strong>{$i18nT('pages.sparql.queryError')}</strong>
      <pre class="error-detail">{error}</pre>
    </div>
  {/if}

  <!-- Results -->
  {#if results}
    <div class="card results-card">
      <!-- Tabs -->
      <div class="results-header">
        <div class="tabs">
          <button class="tab" class:active={activeTab === 'table'} on:click={() => activeTab = 'table'}>
            {$i18nT('pages.sparql.table')}
          </button>
          <button class="tab" class:active={activeTab === 'json'} on:click={() => activeTab = 'json'}>
            {$i18nT('pages.sparql.json')}
          </button>
          {#if canGraph}
            <button class="tab" class:active={activeTab === 'graph'} on:click={() => activeTab = 'graph'}>
              {$i18nT('pages.sparql.graph')}
            </button>
          {/if}
        </div>
        <div class="export-actions">
          {#if results?.results}
            <button class="btn btn-sm btn-ghost" on:click={exportCsv}><Download size={13} /> {$i18nT('pages.sparql.csv')}</button>
          {/if}
          <button class="btn btn-sm btn-ghost" on:click={exportJson}><Download size={13} /> {$i18nT('pages.sparql.json')}</button>
        </div>
      </div>

      <!-- Table tab -->
      {#if activeTab === 'table'}
        {#if results.boolean !== undefined}
          <div class="boolean-result">
            <span class="bool-badge" class:bool-true={results.boolean} class:bool-false={!results.boolean}>
              {#if results.boolean}<Check size={16} /> {$i18nT('pages.sparql.true')}{:else}<XIcon size={16} /> {$i18nT('pages.sparql.false')}{/if}
            </span>
          </div>
        {:else if results.results}
          <DataTable
            mode="bindings"
            vars={results.head?.vars ?? []}
            bindings={results.results.bindings}
            loading={loading}
            emptyText={$i18nT('pages.sparql.noTabular')}
            maxHeight="500px"
          />
        {:else}
          <p class="no-results">{$i18nT('pages.sparql.noTabular')}</p>
        {/if}
      {/if}

      <!-- JSON tab -->
      {#if activeTab === 'json'}
        <pre class="json-view">{JSON.stringify(results, null, 2)}</pre>
      {/if}

      <!-- Graph tab -->
      {#if activeTab === 'graph' && canGraph}
        <GraphCanvas nodes={graphElements.nodes} edges={graphElements.edges} height="480px"
          on:nodeOpen={(e) => e.detail.fullIri && navigate(`/resource?iri=${encodeURIComponent(e.detail.fullIri)}`)} />
      {/if}
    </div>
  {/if}
</div>

<style>
  .sparql-page {
    display: flex;
    flex-direction: column;
    gap: 1rem;
  }

  .editor-card h2 { margin: 0; }

  .editor-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 0.75rem;
    flex-wrap: wrap;
    gap: 0.5rem;
  }

  .header-actions {
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }

  :global(.endpoint-select) {
    width: auto;
  }
  :global(.version-select.version-active) {
    color: #92400e;
    background: #fef3c7;
    border-color: #fde68a;
    font-weight: 600;
  }

  /* Banner shown when the editor was opened from a saved API service. */
  .svc-context {
    display: flex; align-items: center; gap: 0.5rem;
    margin-bottom: 0.75rem; padding: 0.45rem 0.7rem;
    background: #f0f9ff; border: 1px solid #bae6fd; border-radius: 8px;
    color: #075985; font-size: 0.82rem;
  }
  .svc-context :global(svg) { flex-shrink: 0; color: #0284c7; }
  .svc-context-text { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .svc-context-text strong { font-weight: 700; }
  .svc-context-back { margin-left: auto; flex-shrink: 0; }

  .history-wrapper, .templates-wrapper { position: relative; }

  .dropdown {
    position: absolute;
    top: calc(100% + 4px);
    right: 0;
    width: 460px;
    max-height: 420px;
    overflow-y: auto;
    background: #fff;
    border: 1px solid #d0d7de;
    border-radius: 6px;
    box-shadow: 0 8px 24px rgba(0,0,0,0.14);
    z-index: 100;
  }
  .templates-dropdown { width: 340px; }
  .dd-head {
    display: flex; gap: 0.4rem; padding: 0.4rem;
    border-bottom: 1px solid #e5e7eb; position: sticky; top: 0; background: #fff;
  }
  .dd-filter {
    flex: 1; font-size: 0.8rem; padding: 0.25rem 0.5rem;
    border: 1px solid #d0d7de; border-radius: 4px;
  }
  .dd-item {
    display: block; width: 100%; text-align: left;
    padding: 0.5rem 0.75rem; background: none; border: none;
    border-bottom: 1px solid #f0f0f0; cursor: pointer;
  }
  .dd-item:hover { background: #f6f8fa; }
  .dd-title { font-weight: 600; font-size: 0.85rem; color: #1f2937; }
  .dd-sub { font-size: 0.72rem; color: #6b7280; font-family: 'SF Mono', monospace;
    white-space: nowrap; overflow: hidden; text-overflow: ellipsis; margin-top: 2px; }

  .history-empty { padding: 1rem; color: #888; font-size: 0.85rem; text-align: center; }
  .history-item {
    padding: 0.5rem 0.75rem; border-bottom: 1px solid #f0f0f0;
    cursor: pointer; position: relative;
  }
  .history-item:hover { background: #f6f8fa; }
  .history-meta {
    display: flex; justify-content: space-between; align-items: center;
    font-size: 0.7rem; color: #6b7280; margin-bottom: 3px;
  }
  .history-time { font-family: sans-serif; }
  .icon-btn {
    background: none; border: none; cursor: pointer;
    padding: 2px; border-radius: 3px; color: #9ca3af;
    display: inline-flex; align-items: center;
  }
  .icon-btn:hover { background: #fee2e2; color: #dc2626; }
  .history-preview {
    margin: 0; font-family: 'SF Mono', monospace; font-size: 0.72rem;
    color: #1f2937; white-space: pre-wrap; line-height: 1.35;
    max-height: 5em; overflow: hidden;
  }

  .nl-box {
    display: flex; align-items: center; gap: 0.5rem;
    margin-bottom: 0.6rem; padding: 0.4rem 0.5rem;
    background: #f5f8ff; border: 1px solid #dbe4ff; border-radius: 6px;
  }
  .nl-box :global(.nl-icon) { color: #4a6fd9; flex-shrink: 0; }
  .nl-status {
    font-size: 0.7rem; font-weight: 600; color: #16a34a; white-space: nowrap;
    padding: 1px 6px; border-radius: 999px; background: #ecfdf5; border: 1px solid #bbf7d0;
  }
  .nl-status.offline { color: #b45309; background: #fffbeb; border-color: #fde68a; }
  .nl-input {
    flex: 1; font-size: 0.85rem; padding: 0.35rem 0.5rem;
    border: 1px solid #cdd7ee; border-radius: 4px; background: #fff;
  }
  .nl-input:focus { outline: none; border-color: #4a6fd9; }
  .nl-generated {
    display: inline-flex; align-items: center; gap: 0.3rem;
    font-size: 0.75rem; color: #166534; white-space: nowrap;
  }
  .nl-thumb {
    display: inline-flex; align-items: center; justify-content: center;
    background: #eef2ff; border: 1px solid #cdd7ee; border-radius: 4px; cursor: pointer;
    color: #475569; padding: 2px 5px;
  }
  .nl-thumb:hover { background: #e0e7ff; }
  .nl-thumb.up { background: #16a34a; border-color: #16a34a; color: #fff; }
  .nl-thumb.down { background: #dc2626; border-color: #dc2626; color: #fff; }
  .nl-comment {
    width: 100%; font-size: 0.8rem; padding: 0.35rem 0.5rem; margin-bottom: 0.6rem;
    border: 1px solid #dbe4ff; border-radius: 4px; background: #fbfcff;
  }
  .nl-comment:focus { outline: none; border-color: #4a6fd9; }
  .nl-error {
    margin-bottom: 0.6rem; padding: 0.4rem 0.6rem;
    background: #fff8f8; border: 1px solid #f3c9c9; border-radius: 4px;
    color: #b91c1c; font-size: 0.8rem;
  }

  .prefix-bar {
    display: flex; flex-wrap: wrap; gap: 4px; align-items: center;
    margin-bottom: 0.6rem; padding: 0.4rem 0.5rem;
    background: #f9fafb; border: 1px solid #e5e7eb; border-radius: 4px;
    font-size: 0.75rem;
  }
  .prefix-bar :global(.prefix-bar-icon) { color: #94a3b8; flex-shrink: 0; }
  /* Add-prefix affordance: an unobtrusive button that drops the shared
     PrefixSearchPanel as a popover anchored to the prefix bar. */
  .prefix-add-wrap { position: relative; margin-left: auto; }
  .prefix-search-pop {
    position: absolute; top: calc(100% + 5px); right: 0; z-index: 40;
    width: 420px; max-width: 90vw;
    box-shadow: 0 8px 28px rgba(0,0,0,0.14); border-radius: 10px;
  }
  .chip {
    display: inline-flex; align-items: center; gap: 3px;
    padding: 2px 6px; border-radius: 10px;
    font-family: 'SF Mono', monospace; font-size: 0.7rem;
  }
  .chip-ok { background: #dbeafe; color: #1e3a8a; }
  .chip-miss { background: #fef3c7; color: #854d0e; }
  .chip-x {
    background: none; border: none; cursor: pointer;
    color: #1e3a8a; opacity: 0.5; font-size: 13px; line-height: 1; padding: 0;
  }
  .chip-x:hover { opacity: 1; }

  .toolbar {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    margin-top: 0.6rem;
  }

  kbd {
    font-size: 0.7rem;
    background: #e8eaf0;
    padding: 1px 5px;
    border-radius: 3px;
    margin-left: 6px;
    color: #555;
    font-family: sans-serif;
  }

  .elapsed {
    font-size: 0.8rem;
    color: #888;
  }

  .error-card {
    border-left: 3px solid #d94a4a;
    background: #fff8f8;
  }

  .error-detail {
    margin: 0.5rem 0 0;
    font-size: 0.85rem;
    color: #d94a4a;
    white-space: pre-wrap;
    word-break: break-word;
  }

  .results-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 0.75rem;
    border-bottom: 1px solid #e0e0e0;
    padding-bottom: 0.5rem;
  }

  .tabs {
    display: flex;
    gap: 0;
  }

  .tab {
    padding: 0.3rem 0.8rem;
    border: 1px solid #d0d7de;
    background: #f6f8fa;
    cursor: pointer;
    font-size: 0.85rem;
    color: #444;
    border-radius: 0;
    transition: background 0.15s;
  }

  .tab:first-child { border-radius: 6px 0 0 6px; }
  .tab:last-child { border-radius: 0 6px 6px 0; }
  .tab:not(:first-child) { border-left: none; }

  .tab.active {
    background: #4a90d9;
    color: #fff;
    border-color: #4a90d9;
  }

  .export-actions {
    display: flex;
    gap: 0.4rem;
  }

  .boolean-result {
    padding: 1.5rem;
    display: flex;
    justify-content: center;
  }

  .bool-badge {
    font-size: 1.5rem;
    font-weight: 600;
    padding: 0.5rem 1.5rem;
    border-radius: 8px;
  }

  .bool-true { background: #d4edda; color: #155724; }
  .bool-false { background: #f8d7da; color: #721c24; }

  .json-view {
    font-family: 'SF Mono', 'Fira Code', monospace;
    font-size: 0.8rem;
    white-space: pre;
    overflow: auto;
    max-height: 500px;
    background: #f6f8fa;
    padding: 1rem;
    border-radius: 4px;
    margin: 0;
  }

  .no-results {
    color: #888;
    text-align: center;
    padding: 2rem;
  }

  /* ── Dark mode: page-scoped light islands → design tokens / desaturated tints.
     The `:is()` ancestor matches either signal set by lib/theme.ts on <html>. */
  :global(:is([data-theme="dark"], .dark)) .svc-context {
    background: rgba(2, 132, 199, 0.14);
    border-color: rgba(56, 189, 248, 0.35);
    color: #7dd3fc;
  }
  :global(:is([data-theme="dark"], .dark)) .svc-context :global(svg) { color: #38bdf8; }
  :global(:is([data-theme="dark"], .dark) .version-select.version-active) {
    color: #fde68a;
    background: rgba(245, 158, 11, 0.18);
    border-color: rgba(245, 158, 11, 0.4);
  }

  :global(:is([data-theme="dark"], .dark)) .dropdown {
    background: var(--bg-strong);
    border-color: var(--line-strong);
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.5);
  }
  :global(:is([data-theme="dark"], .dark)) .dd-head {
    background: var(--bg-strong);
    border-bottom-color: var(--line-soft);
  }
  :global(:is([data-theme="dark"], .dark)) .dd-filter { border-color: var(--line-strong); }
  :global(:is([data-theme="dark"], .dark)) .dd-item { border-bottom-color: var(--line-soft); }
  :global(:is([data-theme="dark"], .dark)) .dd-item:hover { background: var(--bg-soft); }
  :global(:is([data-theme="dark"], .dark)) .dd-title { color: var(--ink-800); }
  :global(:is([data-theme="dark"], .dark)) .dd-sub { color: var(--ink-500); }

  :global(:is([data-theme="dark"], .dark)) .history-empty,
  :global(:is([data-theme="dark"], .dark)) .no-results,
  :global(:is([data-theme="dark"], .dark)) .elapsed { color: var(--ink-400); }
  :global(:is([data-theme="dark"], .dark)) .history-item { border-bottom-color: var(--line-soft); }
  :global(:is([data-theme="dark"], .dark)) .history-item:hover { background: var(--bg-soft); }
  :global(:is([data-theme="dark"], .dark)) .history-meta { color: var(--ink-500); }
  :global(:is([data-theme="dark"], .dark)) .history-preview { color: var(--ink-700); }
  :global(:is([data-theme="dark"], .dark)) .icon-btn { color: var(--ink-400); }
  :global(:is([data-theme="dark"], .dark)) .icon-btn:hover { background: rgba(220, 38, 38, 0.2); color: #f87171; }

  :global(:is([data-theme="dark"], .dark)) .nl-box {
    background: rgba(99, 102, 241, 0.12);
    border-color: rgba(99, 102, 241, 0.35);
  }
  :global(:is([data-theme="dark"], .dark)) .nl-box :global(.nl-icon) { color: #a5b4fc; }
  :global(:is([data-theme="dark"], .dark)) .nl-status {
    color: #6ee7b7;
    background: rgba(16, 185, 129, 0.16);
    border-color: rgba(16, 185, 129, 0.4);
  }
  :global(:is([data-theme="dark"], .dark)) .nl-status.offline {
    color: #fcd34d;
    background: rgba(245, 158, 11, 0.16);
    border-color: rgba(245, 158, 11, 0.4);
  }
  :global(:is([data-theme="dark"], .dark)) .nl-input,
  :global(:is([data-theme="dark"], .dark)) .nl-comment {
    background: var(--bg-strong);
    border-color: rgba(99, 102, 241, 0.35);
    color: var(--ink-900);
  }
  :global(:is([data-theme="dark"], .dark)) .nl-generated { color: #6ee7b7; }
  :global(:is([data-theme="dark"], .dark)) .nl-thumb {
    background: rgba(99, 102, 241, 0.18);
    border-color: rgba(99, 102, 241, 0.35);
    color: var(--ink-700);
  }
  :global(:is([data-theme="dark"], .dark)) .nl-thumb:hover { background: rgba(99, 102, 241, 0.3); }
  :global(:is([data-theme="dark"], .dark)) .nl-error {
    background: rgba(220, 38, 38, 0.14);
    border-color: rgba(220, 38, 38, 0.4);
    color: #fca5a5;
  }

  :global(:is([data-theme="dark"], .dark)) .prefix-bar {
    background: var(--bg-soft);
    border-color: var(--line-soft);
  }
  :global(:is([data-theme="dark"], .dark)) .chip-ok { background: rgba(59, 130, 246, 0.2); color: #bfdbfe; }
  :global(:is([data-theme="dark"], .dark)) .chip-ok .chip-x { color: #bfdbfe; }
  :global(:is([data-theme="dark"], .dark)) .chip-miss { background: rgba(245, 158, 11, 0.2); color: #fde68a; }

  :global(:is([data-theme="dark"], .dark)) kbd {
    background: rgba(255, 255, 255, 0.08);
    color: var(--ink-400);
  }

  :global(:is([data-theme="dark"], .dark)) .error-card { background: rgba(220, 38, 38, 0.1); }
  :global(:is([data-theme="dark"], .dark)) .error-detail { color: #fca5a5; }

  :global(:is([data-theme="dark"], .dark)) .results-header { border-bottom-color: var(--line-soft); }
  :global(:is([data-theme="dark"], .dark)) .tab {
    background: var(--bg-soft);
    border-color: var(--line-strong);
    color: var(--ink-600);
  }
  :global(:is([data-theme="dark"], .dark)) .tab.active {
    background: var(--brand-500);
    border-color: var(--brand-500);
    color: #fff;
  }

  :global(:is([data-theme="dark"], .dark)) .bool-true { background: rgba(16, 185, 129, 0.2); color: #6ee7b7; }
  :global(:is([data-theme="dark"], .dark)) .bool-false { background: rgba(220, 38, 38, 0.2); color: #fca5a5; }
  :global(:is([data-theme="dark"], .dark)) .json-view { background: var(--bg-soft); color: var(--ink-800); }

  /* ── Dataset / organisation scope bar (ported from the Triple Browser) ──── */
  .dataset-scope-bar {
    display: flex; align-items: center; flex-wrap: wrap; gap: 0.4rem;
    padding: 0.4rem 0.6rem; margin-bottom: 0.6rem;
    background: #eff6ff; border: 1px solid #bfdbfe; border-radius: 6px;
    font-size: 0.78rem; color: #1d4ed8;
  }
  .dataset-scope-label { font-weight: 600; }
  .dataset-scope-chip {
    display: inline-flex; align-items: center; gap: 0.25rem;
    background: #dbeafe; color: #1d4ed8;
    padding: 0.1rem 0.45rem 0.1rem 0.55rem;
    border-radius: 10px; font-size: 0.75rem;
  }
  .dataset-scope-x {
    background: none; border: none; cursor: pointer;
    color: #3b82f6; display: flex; align-items: center; padding: 0;
    line-height: 1; opacity: 0.7;
  }
  .dataset-scope-x:hover { opacity: 1; color: #1d4ed8; }

  .scope-picker-wrap { position: relative; }
  .scope-add-btn {
    display: inline-flex; align-items: center; gap: 0.25rem;
    font-size: 0.75rem; color: #2563eb;
    background: transparent; border: 1px dashed #93c5fd;
    border-radius: 6px; padding: 0.15rem 0.5rem;
    cursor: pointer; line-height: 1.4;
  }
  .scope-add-btn:hover { background: #dbeafe; }
  .scope-picker {
    position: absolute; top: calc(100% + 5px); left: 0; z-index: 30;
    background: white; border: 1px solid #e2e8f0; border-radius: 10px;
    box-shadow: 0 8px 28px rgba(0,0,0,0.12);
    min-width: 220px; max-height: 280px; overflow-y: auto; padding: 0.35rem;
  }
  .scope-search {
    width: 100%; box-sizing: border-box;
    border: 1px solid #e2e8f0; border-radius: 6px;
    padding: 0.3rem 0.5rem; font-size: 0.8rem;
    margin-bottom: 0.3rem; outline: none;
  }
  .scope-search:focus { border-color: #93c5fd; }
  .scope-group-label {
    font-size: 0.68rem; font-weight: 700; letter-spacing: 0.05em;
    text-transform: uppercase; color: #94a3b8;
    padding: 0.2rem 0.4rem 0.05rem;
  }
  .scope-item {
    display: flex; align-items: center; gap: 0.4rem;
    width: 100%; padding: 0.3rem 0.5rem;
    font-size: 0.82rem; color: #1e293b;
    background: transparent; border: none;
    border-radius: 6px; cursor: pointer; text-align: left;
  }
  .scope-item:hover { background: #f1f5f9; }
  .scope-empty {
    font-size: 0.8rem; color: #94a3b8;
    padding: 0.4rem 0.5rem; text-align: center;
  }
  .scope-clear-all {
    font-size: 0.72rem; color: #94a3b8;
    background: transparent; border: none; cursor: pointer;
    margin-left: 0.1rem; padding: 0;
  }
  .scope-clear-all:hover { color: #ef4444; }
  /* Locked chip: the fixed dataset/org context in a scoped editor. */
  .dataset-scope-chip.locked {
    background: #dbeafe; color: #1d4ed8;
    padding-right: 0.55rem; font-weight: 600;
  }
  .dataset-scope-chip.locked :global(svg:last-child) { opacity: 0.6; margin-left: 0.1rem; }
  .scope-hint { color: #64748b; font-size: 0.74rem; font-style: italic; }

  /* ── Named-graph subset within the scope bar ───────────────────────────────── */
  .graph-scope-sep {
    width: 1px; align-self: stretch; margin: 0 0.15rem;
    background: #bfdbfe;
  }
  .graph-scope-wrap { position: relative; }
  .graph-scope-picker { min-width: 280px; }
  .graph-scope-item {
    display: flex; align-items: center; gap: 0.45rem;
    padding: 0.3rem 0.45rem; border-radius: 6px;
    cursor: pointer; font-size: 0.8rem;
  }
  .graph-scope-item:hover { background: #f1f5f9; }
  .graph-scope-iri {
    font-family: 'SF Mono', monospace; font-size: 0.72rem;
    color: #374151; word-break: break-all; white-space: normal;
  }
  /* Graph chips reuse .dataset-scope-chip; clamp very long IRIs. */
  .graph-chip { max-width: 16rem; overflow: hidden; white-space: nowrap; }

  :global(:is([data-theme="dark"], .dark)) .scope-picker { background: var(--bg-strong); border-color: var(--line-strong); }
  :global(:is([data-theme="dark"], .dark)) .dataset-scope-bar { background: rgba(59,130,246,0.12); border-color: rgba(59,130,246,0.3); color: #93c5fd; }
  :global(:is([data-theme="dark"], .dark)) .dataset-scope-chip { background: rgba(59,130,246,0.2); color: #bfdbfe; }
  :global(:is([data-theme="dark"], .dark)) .dataset-scope-x { color: #60a5fa; }
  :global(:is([data-theme="dark"], .dark)) .dataset-scope-x:hover { color: #93c5fd; }
  :global(:is([data-theme="dark"], .dark)) .scope-add-btn { color: #60a5fa; border-color: rgba(59,130,246,0.45); }
  :global(:is([data-theme="dark"], .dark)) .scope-add-btn:hover { background: rgba(59,130,246,0.18); }
  :global(:is([data-theme="dark"], .dark)) .scope-search { background: var(--bg-soft); border-color: var(--line-strong); color: var(--ink-900); }
  :global(:is([data-theme="dark"], .dark)) .scope-group-label,
  :global(:is([data-theme="dark"], .dark)) .scope-empty { color: var(--ink-600); }
  :global(:is([data-theme="dark"], .dark)) .scope-item { color: var(--ink-800); }
  :global(:is([data-theme="dark"], .dark)) .scope-item:hover { background: rgba(255,255,255,0.06); }
  :global(:is([data-theme="dark"], .dark)) .dataset-scope-chip.locked { background: rgba(59,130,246,0.22); color: #bfdbfe; }
  :global(:is([data-theme="dark"], .dark)) .scope-hint { color: var(--ink-500); }
  :global(:is([data-theme="dark"], .dark)) .graph-scope-sep { background: rgba(59,130,246,0.35); }
  :global(:is([data-theme="dark"], .dark)) .graph-scope-item:hover { background: rgba(255,255,255,0.06); }
  :global(:is([data-theme="dark"], .dark)) .graph-scope-iri { color: var(--ink-700); }
</style>
