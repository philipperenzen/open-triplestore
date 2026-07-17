<script>
  import { onMount, onDestroy } from 'svelte';
  import { delayedLoading } from '../lib/delayedLoading';
  import { autofocus } from '../lib/actions/autofocus.js';
  import { browseTriples, browseSuggest, browseFacets, getDataset, getOrganisation, browseResource, listDatasets, listOrganisations, listDatasetVersions, listDatasetGraphs, nlToSparql, llmHealth, getViewerFeed, getGeoStatsBatch } from '../lib/api.js';
  import { shortenIRI, downloadFile, graphResultsToElements, loadPrefixCcPrefixes, normalizeGraphRole, graphRoleLabel, detectGeoBindings, triplesToResults } from '../lib/rdf-utils.js';
  import DataTable from '../components/DataTable.svelte';
  // GraphCanvas (cytoscape) and ViewerMap (maplibre + leaflet) are loaded lazily
  // the first time the graph/map view is opened — see graphCanvasMod/viewerMapMod
  // below. The table view (default) then never pulls those vendor bundles into the
  // /browse download.
  import ContextMenu from '../components/ContextMenu.svelte';
  import FacetRail from '../components/browse/FacetRail.svelte';
  import TermDefinitionCard from '../components/ontology/TermDefinitionCard.svelte';
  import { t as i18nT } from 'svelte-i18n';
  import {
    Download, Copy, ChevronLeft, ChevronRight, Search, X,
    Network, Table2, Maximize2, Share2, Unlink, ExternalLink, Plus,
    FileText, Image, Filter, LayoutList, Building2, Database, History,
    Sparkles, SlidersHorizontal, Code2, HelpCircle, Map as MapIcon, Boxes,
  } from 'lucide-svelte';
  import { slide } from 'svelte/transition';
  import { toNTriples, toNQuads, toTurtle, toTrig } from '../lib/rdf-utils.js';
  import { navigate } from '../lib/router/index.js';
  import { copyToClipboard } from '../lib/clipboard.js';
  import PageHeader from '../components/PageHeader.svelte';
  import Select from '../components/Select.svelte';
  import Combobox from '../components/Combobox.svelte';

  // ─── View mode ────────────────────────────────────────────────────────────
  let viewMode = 'table'; // 'table' | 'graph' | 'map'
  // Map data is the WHOLE scope's geometry (not the current page): the merged
  // viewer feed of every scoped dataset — the same rich ViewerElement[] the
  // dataset viewer feeds to ViewerMap (2D vectors + to-scale 3D models).
  let mapElements = [];
  let mapLoading = false;
  let mapLoadedKey = ''; // scope signature the current mapElements reflect
  const scopeKey = () => scopedDatasetIds.slice().sort().join(',');

  async function loadMapElements() {
    const key = scopeKey();
    if (mapLoadedKey === key && mapElements.length) return; // already current
    mapLoading = true;
    try {
      const feeds = await Promise.all(
        scopedDatasetIds.map((id) =>
          getViewerFeed(id).then((d) => d?.elements || []).catch(() => [])
        )
      );
      if (scopeKey() !== key) return; // scope changed mid-flight — drop stale result
      // Dedupe by id across datasets (BOT/IFC ids could in principle recur).
      const byId = new Map();
      for (const el of feeds.flat()) if (el && el.id != null && !byId.has(el.id)) byId.set(el.id, el);
      mapElements = [...byId.values()];
      mapLoadedKey = key;
    } finally {
      if (scopeKey() === key) mapLoading = false;
    }
  }

  function switchView(mode) {
    viewMode = mode;
    syncViewToUrl(mode);
    if (mode === 'graph' && graphNodes.length === 0 && graphEdges.length === 0 && !graphLoading) {
      fetchGraphData();
    }
    if (mode === 'map') loadMapElements();
  }
  // Reload the map when the scope changes while the Map tab is already open.
  $: if (viewMode === 'map' && scopedDatasetIds && scopeKey() !== mapLoadedKey && !mapLoading) {
    loadMapElements();
  }

  // ─── Map view gating (SCOPE-aware) ────────────────────────────────────────
  // The Map tab is offered when the browse SCOPE — not just the current page —
  // carries geometry. One batched probe OR-aggregates the capability across the
  // whole scope (scopedDatasetIds already expands org items to their datasets):
  // the server unions the datasets' graphs and answers in a single request, so a
  // geometry-free scope costs one ASK no matter how many datasets it spans.
  let scopeGeoStats = null; // GeoStats | null (null = loading or failed)
  let geoStatsKey = ''; // scope signature scopeGeoStats reflects (de-dupes + races)
  let geoStatsSettledKey = ''; // scope signature whose probe has resolved
  async function loadGeoStats(k, ids) {
    let stats = null;
    try {
      stats = await getGeoStatsBatch(ids);
    } catch {
      stats = null;
    }
    if (geoStatsKey !== k) return; // scope changed mid-load — drop stale result
    scopeGeoStats = stats;
    geoStatsSettledKey = k;
  }
  $: {
    const k = scopedDatasetIds.slice().sort().join(',');
    if (k && k !== geoStatsKey) {
      geoStatsKey = k;
      scopeGeoStats = null; // fall back to pageCanMap until the new probe resolves
      loadGeoStats(k, scopedDatasetIds.slice());
    }
  }
  $: geoStatsSettled =
    scopedDatasetIds.length > 0 &&
    geoStatsSettledKey === scopedDatasetIds.slice().sort().join(',');
  $: scopeHasGeo = !!(scopeGeoStats && (scopeGeoStats.has_coordinates || scopeGeoStats.has_3d));
  $: scopeHas3d = !!(scopeGeoStats && scopeGeoStats.has_3d);
  // Per-page fallback: even before the scope probe settles, a page that itself
  // carries WGS84 rows can already offer the map (graceful; old behaviour).
  $: pageCanMap = detectGeoBindings(triplesToResults(triples));
  $: canMap = scopeHasGeo || pageCanMap;
  // Only bump the user off Map once the scope probe has SETTLED and there is
  // genuinely no geo — never mid-load (that would bounce them during the probe).
  $: if (viewMode === 'map' && geoStatsSettled && !canMap && !pageCanMap) switchView('table');

  function syncViewToUrl(mode) {
    if (typeof window === 'undefined') return;
    const url = new URL(window.location.href);
    if (mode === 'table') url.searchParams.delete('view');
    else url.searchParams.set('view', mode);
    window.history.replaceState({}, '', url);
  }

  // ─── Export modal ─────────────────────────────────────────────────────────
  let exportModalOpen = false;
  let exportScope = 'page'; // 'page' | 'all'
  let exportingAll = false;

  function closeExport() { exportModalOpen = false; exportScope = 'page'; }

  // Fetch all filtered triples via repeated SPARQL pages (no size cap)
  async function fetchAllTriples() {
    const all = [];
    let off = 0;
    const batchSize = 500;
    while (true) {
      const params = { limit: batchSize.toString(), offset: off.toString(), ...buildFilterParams() };
      const res = await browseTriples(params);
      const batch = res.triples || [];
      all.push(...batch);
      if (batch.length < batchSize) break;
      off += batchSize;
    }
    return all;
  }

  async function doExport(format) {
    let data;
    if (exportScope === 'all') {
      exportingAll = true;
      try { data = await fetchAllTriples(); } finally { exportingAll = false; }
    } else {
      data = triples;
    }
    const suffix = exportScope === 'all' ? 'all' : 'page';
    switch (format) {
      case 'csv': {
        const header = 'subject,predicate,object,datatype,language,graph';
        const rows = data.map(t => {
          return [t.subject?.value, t.predicate?.value, t.object?.value, t.object?.datatype, t.object?.language, t.graph?.value || '']
            .map(v => `"${(v || '').replace(/"/g, '""')}"`)
            .join(',');
        });
        downloadFile([header, ...rows].join('\n'), `triples-${suffix}.csv`, 'text/csv');
        break;
      }
      case 'nt':
        downloadFile(toNTriples(data), `triples-${suffix}.nt`, 'application/n-triples');
        break;
      case 'nq':
        downloadFile(toNQuads(data), `triples-${suffix}.nq`, 'application/n-quads');
        break;
      case 'ttl':
        downloadFile(toTurtle(data), `triples-${suffix}.ttl`, 'text/turtle');
        break;
      case 'trig':
        downloadFile(toTrig(data), `triples-${suffix}.trig`, 'application/trig');
        break;
    }
    closeExport();
  }

  function exportGraphPng() { graphCanvas?.exportPng(); closeExport(); }
  function exportGraphPngHiRes() { graphCanvas?.exportPngHiRes(); closeExport(); }
  function exportGraphSvg() { graphCanvas?.exportSvg(); closeExport(); }

  // ─── Graph (inline) ───────────────────────────────────────────────────────
  let graphCanvas;
  let activeLayout = 'cose-bilkent';

  // Lazily import the heavy view-only components the first time their view is
  // opened. Memoised (the `!mod` guard) so a data update inside the view never
  // re-imports and remounts the canvas — only the initial open pays the load.
  let graphCanvasMod;
  let viewerMapMod;
  $: if (viewMode === 'graph' && !graphCanvasMod) graphCanvasMod = import('../components/GraphCanvas.svelte');
  $: if (viewMode === 'map' && !viewerMapMod) viewerMapMod = import('../components/viewer/ViewerMap.svelte');

  let graphNodes = [];
  let graphEdges = [];
  let graphLoading = false;
  let graphLoadingMore = false;
  let graphOffset = 0;
  let graphHasMore = false;
  let browseExpansionCache = new Map();

  // Track expand/collapse: uri → { nodeIds: Set, edgeIds: Set }
  let browseExpandedUris = new Map();
  // Track directions expanded per uri: uri → Set<'in'|'out'>
  let browseExpandedDirs = new Map();
  // IRI currently being expanded (for loading indicator in GraphCanvas)
  let browseExpandingUri = null;
  // Transient hint shown over the graph (e.g. a blank node with nothing to expand in scope).
  let browseGraphHint = '';
  let browseGraphHintTimer = null;
  function flashGraphHint(msg) {
    browseGraphHint = msg;
    clearTimeout(browseGraphHintTimer);
    browseGraphHintTimer = setTimeout(() => { browseGraphHint = ''; }, 4000);
  }
  // Reactive set of currently expanded IRIs for GraphCanvas badge
  $: browseExpandedIris = new Set(browseExpandedUris.keys());
  // Reactive set of fully-exhausted IRIs (both in + out queried) — hides + badge
  $: browseExhaustedIris = new Set(
    [...browseExpandedDirs.entries()]
      .filter(([, dirs]) => dirs.has('in') && dirs.has('out'))
      .map(([iri]) => iri)
  );

  // Context menu state
  let browseCtxVisible = false;
  let browseCtxX = 0, browseCtxY = 0;
  let browseCtxItems = [];
  let browseCtxNodeData = null;

  async function fetchGraphData() {
    graphLoading = true;
    graphNodes = [];
    graphEdges = [];
    graphOffset = 0;
    graphHasMore = false;
    browseExpandedUris = new Map();
    browseExpandedDirs = new Map();
    browseExpansionCache = new Map();
    try {
      const params = { limit: '100', offset: '0', ...buildFilterParams() };
      const res = await browseTriples(params);
      const triples = res.triples || [];
      const { nodes, edges } = graphResultsToElements(triples, 'subject', 'predicate', 'object', 250);
      graphHasMore = nodes.some(n => n.data.nodeType === 'uri');
      graphNodes = nodes;
      graphEdges = edges;
      graphOffset = 100;
    } catch {
      graphNodes = [];
      graphEdges = [];
    } finally {
      graphLoading = false;
    }
  }

  async function loadMoreGraphTriples() {
    if (graphLoading || graphLoadingMore) return;
    if (!graphHasMore) return;
    graphLoadingMore = true;
    try {
      // Prefer expanding connected nodes not yet fully explored
      const unexplored = graphNodes.filter(n => {
        const iri = n.data?.fullIri;
        const nt = n.data?.nodeType;
        if (!iri || nt === 'literal' || nt === 'bnode') return false;
        const dirs = browseExpandedDirs.get(iri);
        return !dirs || !dirs.has('in') || !dirs.has('out');
      });
      if (unexplored.length > 0) {
        // Expand up to 10 nodes — their neighbors become new graph nodes
        for (const node of unexplored.slice(0, 10)) {
          await browseExpandUri(node.data.fullIri, 'both');
        }
        // More to load if any URI nodes still have unexplored directions
        graphHasMore = graphNodes.some(n => {
          const iri = n.data?.fullIri;
          if (!iri || n.data?.nodeType === 'literal' || n.data?.nodeType === 'bnode') return false;
          const dirs = browseExpandedDirs.get(iri);
          return !dirs || !dirs.has('in') || !dirs.has('out');
        });
      } else {
        // All visible nodes exhausted — fall back to offset pagination
        const params = { limit: '100', offset: graphOffset.toString(), ...buildFilterParams() };
        const res = await browseTriples(params);
        const triples = res.triples || [];
        const { nodes, edges } = graphResultsToElements(triples, 'subject', 'predicate', 'object', 250);
        const existingIds = new Set(graphNodes.map(n => n.data.id));
        const existingEdgeIds = new Set(graphEdges.map(e => e.data.id));
        graphNodes = [...graphNodes, ...nodes.filter(n => !existingIds.has(n.data.id))];
        graphEdges = [...graphEdges, ...edges.filter(e => !existingEdgeIds.has(e.data.id))];
        graphOffset += 100;
        graphHasMore = triples.length >= 100;
      }
    } catch {} finally {
      graphLoadingMore = false;
    }
  }

  // Merge freshly-fetched nodes/edges into the graph, deduping by id, and record
  // them under `key` (a URI or a blank-node id) plus the directions covered, so the
  // node can later be collapsed and its expand badge resolved correctly.
  function applyExpansion(key, dirsToAdd, newNodes, newEdges) {
    const existingIds = new Set(graphNodes.map(n => n.data.id));
    const existingEdgeIds = new Set(graphEdges.map(e => e.data.id));
    const nodesToAdd = newNodes.filter(n => !existingIds.has(n.data.id));
    const edgesToAdd = newEdges.filter(e => !existingEdgeIds.has(e.data.id));
    graphNodes = [...graphNodes, ...nodesToAdd];
    graphEdges = [...graphEdges, ...edgesToAdd];
    const prev = browseExpandedUris.get(key) || { nodeIds: new Set(), edgeIds: new Set() };
    browseExpandedUris = new Map(browseExpandedUris).set(key, {
      nodeIds: new Set([...prev.nodeIds, ...nodesToAdd.map(n => n.data.id)]),
      edgeIds: new Set([...prev.edgeIds, ...edgesToAdd.map(e => e.data.id)]),
    });
    const dirs = new Set(browseExpandedDirs.get(key) || []);
    for (const d of dirsToAdd) dirs.add(d);
    browseExpandedDirs = new Map(browseExpandedDirs).set(key, dirs);
  }

  async function browseExpandUri(uri, direction = 'both') {
    if (!uri || uri.startsWith('_:') || (!uri.includes('://') && !uri.startsWith('urn:'))) return;
    browseExpandingUri = uri;
    try {
      const dirsToAdd = direction === 'both' ? ['in', 'out'] : [direction];
      const cacheKey = `${uri}::${direction}`;
      if (browseExpansionCache.has(cacheKey)) {
        const { nodes, edges } = browseExpansionCache.get(cacheKey);
        applyExpansion(uri, dirsToAdd, nodes, edges);
        return;
      }

      // Fetch the node's neighbourhood through the SAME scoped browse endpoint as
      // the initial load, so dataset/org/version scope is honoured. (The global
      // /sparql endpoint ignores the browse scope, which is why expansion loaded
      // nothing when scoped to a dataset or pinned to a version.) Outgoing = exact
      // subject match, incoming = exact object match; run concurrently for "both".
      const scope = buildExpandScopeParams();
      const outPromise = (direction === 'both' || direction === 'out')
        ? browseTriples({ limit: '120', offset: '0', ...scope, filters: JSON.stringify([{ field: 'subject', value: uri, mode: 'exact' }]) })
        : Promise.resolve(null);
      const inPromise  = (direction === 'both' || direction === 'in')
        ? browseTriples({ limit: '40', offset: '0', ...scope, filters: JSON.stringify([{ field: 'object', value: uri, mode: 'exact' }]) })
        : Promise.resolve(null);
      const [outRes, inRes] = await Promise.all([outPromise, inPromise]);
      const rows = [...(outRes?.triples || []), ...(inRes?.triples || [])];
      const { nodes: newNodes, edges: newEdges } = graphResultsToElements(rows, 'subject', 'predicate', 'object', 300);
      browseExpansionCache = new Map(browseExpansionCache).set(cacheKey, { nodes: newNodes, edges: newEdges });
      applyExpansion(uri, dirsToAdd, newNodes, newEdges);
    } catch {}
    finally { browseExpandingUri = null; }
  }

  // A blank node can't be referenced by its label in SPARQL (a `_:x` there is a
  // fresh variable, not a reference to a stored node), but the scoped
  // /api/browse/resource endpoint resolves a stored blank node natively via the
  // quad store. Pass the browse scope so dataset/version-snapshot blank nodes
  // resolve in the same scope as the initial load.
  async function browseExpandBnode(bnodeId) {
    if (!bnodeId) return;
    browseExpandingUri = bnodeId;
    try {
      const cacheKey = `bnode::${bnodeId}`;
      if (browseExpansionCache.has(cacheKey)) {
        const { nodes, edges } = browseExpansionCache.get(cacheKey);
        applyExpansion(bnodeId, ['in', 'out'], nodes, edges);
        return;
      }
      const res = await browseResource(`_:${bnodeId}`, buildExpandScopeParams());
      // Mirror ResourceDetail.buildGraph: anchor each row on this blank node and
      // include its nested blank-node descriptions so they don't dead-end.
      const rows = [];
      for (const row of (res?.outgoing || []))
        rows.push({ s: { type: 'bnode', value: bnodeId }, p: row.p, o: row.o });
      for (const row of (res?.incoming || []))
        rows.push({ s: row.s, p: row.p, o: { type: 'bnode', value: bnodeId } });
      for (const [id, brows] of Object.entries(res?.bnodes || {}))
        for (const row of (brows || []))
          rows.push({ s: { type: 'bnode', value: id }, p: row.p, o: row.o });
      const { nodes, edges } = graphResultsToElements(rows);
      if (!nodes.length) { flashGraphHint($i18nT('pages.tripleBrowser.expandBnodeEmpty')); return; }
      browseExpansionCache = new Map(browseExpansionCache).set(cacheKey, { nodes, edges });
      applyExpansion(bnodeId, ['in', 'out'], nodes, edges);
    } catch {}
    finally { browseExpandingUri = null; }
  }

  function browseCollapseUri(uri) {
    const expanded = browseExpandedUris.get(uri);
    if (!expanded) return;
    const { nodeIds, edgeIds } = expanded;

    // Collect nodes/edges still owned by OTHER expansions so we don't remove them
    const otherNodeIds = new Set();
    const otherEdgeIds = new Set();
    for (const [otherUri, data] of browseExpandedUris) {
      if (otherUri === uri) continue;
      for (const id of data.nodeIds) otherNodeIds.add(id);
      for (const id of data.edgeIds) otherEdgeIds.add(id);
    }

    const removeNodes = new Set([...nodeIds].filter(id => !otherNodeIds.has(id)));
    const removeEdges = new Set([...edgeIds].filter(id => !otherEdgeIds.has(id)));

    graphNodes = graphNodes.filter(n => !removeNodes.has(n.data.id));
    // Also drop edges whose endpoints no longer exist
    const keptNodeIds = new Set(graphNodes.map(n => n.data.id));
    graphEdges = graphEdges.filter(e =>
      !removeEdges.has(e.data.id) &&
      keptNodeIds.has(e.data.source) &&
      keptNodeIds.has(e.data.target)
    );

    const next = new Map(browseExpandedUris);
    next.delete(uri);
    browseExpandedUris = next;
    const nextDirs = new Map(browseExpandedDirs);
    nextDirs.delete(uri);
    browseExpandedDirs = nextDirs;
  }

  function handleBrowseNodeExpand(e) {
    if (e.detail.fullIri) browseExpandUri(e.detail.fullIri);
    else if (e.detail.nodeType === 'bnode') browseExpandBnode(e.detail.id);
  }

  // Clicking an edge surfaces the FULL predicate definition — richer than a node
  // click (which just opens the inspector). Predicates are where the vocabulary
  // semantics live (dcat:mediaType, owl:*, …), so this is the priority surface.
  let browseEdgePredicate = null;
  function handleBrowseEdgeClick(e) {
    const iri = e?.detail?.predicate;
    if (iri) browseEdgePredicate = iri;
  }

  // Inspector panel "Open resource" → open the full resource page.
  function handleBrowseNodeOpen(e) {
    if (e.detail?.fullIri) navigate(`/resource?iri=${encodeURIComponent(e.detail.fullIri)}`);
  }

  function buildBrowseNodeMenu(data) {
    const items = [];
    if (data.nodeType === 'uri' && data.fullIri) {
      const expandedDirs = browseExpandedDirs.get(data.fullIri) || new Set();
      const hasExpanded = expandedDirs.size > 0;
      if (hasExpanded) items.push({ label: $i18nT('pages.tripleBrowser.ctxCollapse'), icon: Unlink, action: 'collapse' });
      if (!expandedDirs.has('out')) items.push({ label: $i18nT('pages.tripleBrowser.ctxExpandOutgoing'), icon: ChevronRight, action: 'expandOut' });
      if (!expandedDirs.has('in'))  items.push({ label: $i18nT('pages.tripleBrowser.ctxExpandIncoming'), icon: ChevronLeft, action: 'expandIn' });
      if (!expandedDirs.has('in') || !expandedDirs.has('out'))
        items.push({ label: $i18nT('pages.tripleBrowser.ctxExpandBoth'), icon: Plus, action: 'expandBoth' });
      items.push({ divider: true });
      items.push({ label: $i18nT('pages.tripleBrowser.ctxCopyIri'), icon: Copy, action: 'copyIri' });
    } else if (data.nodeType === 'bnode') {
      const hasExpanded = (browseExpandedDirs.get(data.id)?.size || 0) > 0;
      if (hasExpanded) items.push({ label: $i18nT('pages.tripleBrowser.ctxCollapse'), icon: Unlink, action: 'collapse' });
      else             items.push({ label: $i18nT('pages.tripleBrowser.ctxExpand'), icon: Plus, action: 'expandBnode' });
      items.push({ divider: true });
    }
    items.push({ label: $i18nT('pages.tripleBrowser.ctxRemoveFromGraph'), icon: Unlink, action: 'remove', danger: true });
    return items;
  }

  function handleBrowseNodeContextMenu(e) {
    const { data, x, y } = e.detail;
    browseCtxNodeData = data;
    browseCtxItems = buildBrowseNodeMenu(data);
    browseCtxX = x; browseCtxY = y; browseCtxVisible = true;
  }

  function handleBrowseCanvasContextMenu(e) {
    browseCtxNodeData = null;
    browseCtxItems = [
      ...(graphHasMore ? [{ label: $i18nT('pages.tripleBrowser.ctxLoadMore'), icon: Network, action: 'loadMore' }] : []),
      { label: $i18nT('pages.tripleBrowser.ctxFitAll'), icon: Maximize2, action: 'fit' },
      { divider: true },
      { label: $i18nT('pages.tripleBrowser.ctxExportPng'), icon: Share2, action: 'export' },
    ];
    browseCtxX = e.detail.x; browseCtxY = e.detail.y; browseCtxVisible = true;
  }

  function handleBrowseCtxAction(e) {
    const action = e.detail;
    if (action === 'fit')      graphCanvas?.fitAll();
    else if (action === 'export')   graphCanvas?.exportPng();
    else if (action === 'loadMore') loadMoreGraphTriples();
    else if (browseCtxNodeData) {
      const data = browseCtxNodeData;
      if      (action === 'expandOut')   browseExpandUri(data.fullIri, 'out');
      else if (action === 'expandIn')    browseExpandUri(data.fullIri, 'in');
      else if (action === 'expandBoth')  browseExpandUri(data.fullIri, 'both');
      else if (action === 'expandBnode') browseExpandBnode(data.id);
      else if (action === 'collapse')    browseCollapseUri(data.fullIri || data.id);
      else if (action === 'copyIri')     void copyToClipboard(data.fullIri);
      else if (action === 'remove')      graphCanvas?.removeNode(data.id);
    }
  }

  // ─── Table state ──────────────────────────────────────────────────────────
  let triples = [];
  let total = null;
  let hasMore = false;
  let error = '';
  let loading = false;
  let loadingCount = false;

  // Quick search box. Sent to the backend as `q` so the substring is matched
  // server-side across every column (subject, predicate, object, graph) over
  // the entire result set — not just the current page.
  let tableSearch = '';
  let tableSearchDebounce = null;
  function scheduleTableSearch() {
    if (tableSearchDebounce) clearTimeout(tableSearchDebounce);
    tableSearchDebounce = setTimeout(() => {
      tableSearchDebounce = null;
      refetchResults();
    }, 250);
  }
  function clearTableSearch() {
    if (tableSearchDebounce) { clearTimeout(tableSearchDebounce); tableSearchDebounce = null; }
    tableSearch = '';
    refetchResults();
  }
  // Render the full server page — the search is now applied upstream.
  $: filteredTriples = triples;
  // Whether the filters panel (chips + facet rail) is shown.
  let filtersOpen = true;

  // The shared global search drives node highlighting in the graph view too, so
  // "search" means the same thing everywhere. Null → fall back to the canvas's
  // own search box.
  $: graphHighlightIds = (() => {
    const q = tableSearch.trim().toLowerCase();
    if (q.length < 2) return null;
    return new Set(
      graphNodes
        .filter((n) => (n.data.label || '').toLowerCase().includes(q) || (n.data.fullIri || '').toLowerCase().includes(q))
        .map((n) => n.data.id)
    );
  })();


  // ─── Filters ──────────────────────────────────────────────────────────────
  // A classic multi-field filter form drives every view — one input per field
  // (subject/predicate/object/graph/vocabulary), each with its own match mode.
  // State lives in `fieldFilters` (declared below).
  // A single graph drill-down (from a ?graph= URL param) is kept for fetchGraphData
  // / facets scoping.
  let filterGraph = '';

  // ─── UI mode (Simple = guided for newcomers · Advanced = raw IRIs + DX) ─────
  let uiMode = 'simple';
  if (typeof localStorage !== 'undefined' && localStorage.getItem('tb_uimode') === 'advanced') uiMode = 'advanced';
  function setUiMode(m) {
    uiMode = m;
    try { localStorage.setItem('tb_uimode', m); } catch {}
    if (m === 'advanced') {
      if (llmStatus === null) llmHealth().then((s) => { llmStatus = s; }).catch(() => {});
    } else {
      // Close advanced-only panels when leaving Advanced.
      sparqlPreviewOpen = false;
    }
  }
  let sparqlPreviewOpen = false;
  let syntaxHelpOpen = false;

  // Natural-language → SPARQL (Advanced SPARQL panel).
  let llmStatus = null;     // { reachable, gateway } or null until checked
  let nlQuestion = '';
  let nlLoading = false;
  let nlError = '';
  let llmSparql = '';
  async function generateFromNl() {
    const q = nlQuestion.trim();
    if (!q || nlLoading) return;
    nlLoading = true;
    nlError = '';
    try {
      // Give the model light schema context from the in-scope facets.
      const schemaHint = [
        ...rawFacets.classes.slice(0, 25).map((c) => `class ${c.iri}`),
        ...rawFacets.properties.slice(0, 40).map((p) => `property ${p.iri}`),
      ].join('\n');
      const res = await nlToSparql(q, schemaHint);
      llmSparql = res?.sparql || '';
      if (!llmSparql) nlError = $i18nT('pages.tripleBrowser.nlNoQuery');
    } catch (e) {
      nlError = e?.message ? `${$i18nT('pages.tripleBrowser.nlGenerationFailedWith')}: ${e.message}` : $i18nT('pages.tripleBrowser.nlGenerationFailed');
    } finally {
      nlLoading = false;
    }
  }

  // ─── Facets present in scope (classes / properties / graphs) ────────────────
  let rawFacets = { classes: [], properties: [], graphs: [] };
  let facetsLoading = false;
  let railCollapsed = false;
  let graphRoleMap = {}; // graph IRI -> { role, label }
  // Merge declared graph roles onto the facet graphs for the rail's role tags.
  $: facets = {
    classes: rawFacets.classes || [],
    properties: rawFacets.properties || [],
    graphs: (rawFacets.graphs || []).map((g) => {
      const r = graphRoleMap[g.iri];
      return r ? { ...g, role: r.role, roleLabel: r.label } : g;
    }),
  };

  // Scope selector state
  let scopeItems = []; // Array<{type:'dataset'|'org', id:string, name:string}>
  let allDatasets = [];
  let allOrgs = [];
  let scopePickerOpen = false;
  let scopeSearch = '';

  let page = 0;
  let pageSize = 25;

  let backDatasetId = null;
  let backOrgId = null;
  let backContextName = null;

  // ── Per-dataset version scoping ──────────────────────────────────────────
  // Each dataset in scope (directly, or via an org) can be pinned to a version
  // snapshot. Empty / "live" means live data for that dataset.
  let dsVersions = {};        // dsId -> selected version ('' = live)
  let versionsByDs = {};      // dsId -> DatasetVersion[]
  let versionPanelOpen = false;
  const LIVE_SENTINELS = ['', 'live', 'latest', 'current'];
  const isPinned = (v) => !!v && !LIVE_SENTINELS.includes(v);

  // Dataset IDs in scope: explicit dataset items + all datasets owned by any org item.
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
  $: dsNameById = Object.fromEntries((allDatasets || []).map(d => [d.id, d.name || d.id]));
  $: scopedDatasetIds.forEach(id => ensureDsVersions(id));
  $: pinnedVersionCount = scopedDatasetIds.filter(id => isPinned(dsVersions[id])).length;

  async function ensureDsVersions(id) {
    if (versionsByDs[id]) return;
    versionsByDs = { ...versionsByDs, [id]: [] }; // mark in-flight to avoid duplicate fetches
    try {
      const vs = await listDatasetVersions(id);
      versionsByDs = { ...versionsByDs, [id]: vs || [] };
    } catch {
      versionsByDs = { ...versionsByDs, [id]: [] };
    }
  }
  function verParts(v) {
    return String(v?.version ?? '').replace(/^v/i, '').split('.').map(n => parseInt(n, 10) || 0);
  }
  function sortedVersions(id) {
    return (versionsByDs[id] || []).slice().sort((a, b) => {
      const pa = verParts(a), pb = verParts(b);
      for (let i = 0; i < Math.max(pa.length, pb.length); i++) {
        const x = pa[i] ?? 0, y = pb[i] ?? 0;
        if (x !== y) return y - x;
      }
      return 0;
    });
  }
  // Datasets in scope that actually have versions to choose from.
  $: versionableDatasets = scopedDatasetIds.filter(id => (versionsByDs[id] || []).length > 0);

  function onVersionChange() {
    refetchAll();
  }

  // ── Named-graph scope narrowing ──────────────────────────────────────────
  // When one or more datasets are in scope, the user can narrow the browse to a
  // subset of their named graphs. Selected graphs are sent to the backend as
  // exact `graph` filter chips (OR-ed together) in buildFilterParams().
  let scopeGraphs = [];       // selected graph IRIs (subset of availableScopeGraphs)
  let graphsByDs = {};        // dsId -> DatasetGraph[] — present ONLY once loaded
  let graphsInFlight = new Set(); // dsIds currently being fetched (dedupe guard)
  let scopeGraphsLoading = false;
  let graphScopeOpen = false; // graph picker dropdown open

  async function ensureDsGraphs(id) {
    // Already loaded or in-flight → nothing to do.
    if (Array.isArray(graphsByDs[id]) || graphsInFlight.has(id)) return;
    graphsInFlight.add(id);
    scopeGraphsLoading = true;
    try {
      const gs = await listDatasetGraphs(id);
      graphsByDs = { ...graphsByDs, [id]: gs || [] };
    } catch {
      graphsByDs = { ...graphsByDs, [id]: [] };
    } finally {
      graphsInFlight.delete(id);
      scopeGraphsLoading = graphsInFlight.size > 0;
    }
  }
  // Fetch graphs for every scoped dataset (deduped, cached per dataset).
  $: scopedDatasetIds.forEach(id => ensureDsGraphs(id));

  // Distinct named graphs available across all scoped datasets, with the
  // triple count summed and a private flag if any dataset marks it private.
  $: availableScopeGraphs = (() => {
    const byIri = new Map();
    for (const id of scopedDatasetIds) {
      for (const g of (graphsByDs[id] || [])) {
        if (!g.graph_iri) continue;
        const cur = byIri.get(g.graph_iri) || { graph_iri: g.graph_iri, triple_count: 0, private: false };
        cur.triple_count += g.triple_count || 0;
        cur.private = cur.private || !!g.private;
        byIri.set(g.graph_iri, cur);
      }
    }
    return [...byIri.values()].sort((a, b) => a.graph_iri.localeCompare(b.graph_iri));
  })();

  // Drop any selected graph that left scope (e.g. its dataset was removed). Only
  // prune once every scoped dataset's graph list has loaded — otherwise a restore
  // (which sets scopeGraphs before the async lists arrive) would be wiped, and the
  // guard keeps this effect from looping when nothing changed.
  $: {
    const allLoaded = scopedDatasetIds.length > 0
      && scopedDatasetIds.every(id => Array.isArray(graphsByDs[id]));
    if (allLoaded) {
      const avail = new Set(availableScopeGraphs.map(g => g.graph_iri));
      const pruned = scopeGraphs.filter(iri => avail.has(iri));
      if (pruned.length !== scopeGraphs.length) scopeGraphs = pruned;
    }
  }

  function toggleScopeGraph(iri) {
    scopeGraphs = scopeGraphs.includes(iri)
      ? scopeGraphs.filter(g => g !== iri)
      : [...scopeGraphs, iri];
    refetchResults();
  }
  function removeScopeGraph(iri) {
    scopeGraphs = scopeGraphs.filter(g => g !== iri);
    refetchResults();
  }
  function clearScopeGraphs() {
    if (!scopeGraphs.length) return;
    scopeGraphs = [];
    refetchResults();
  }


  // ── Working-state persistence (sessionStorage) ────────────────────────────
  // The browser back button (Alt+←) re-mounts this page from scratch, which used
  // to wipe the table page AND the laid-out graph (forcing a full re-fetch and
  // re-layout). We snapshot the working state per route into sessionStorage and
  // restore it on mount so returning lands exactly where the user left off.
  const STATE_VERSION = 1;
  // Persisting the full graph can be large; cap the element count and the
  // serialized size. Past the cap we save everything EXCEPT the graph payload
  // and fall back to a normal fetch when the graph view is reopened.
  const GRAPH_ELEMENT_CAP = 4000;       // nodes + edges
  const STATE_BYTE_CAP = 1_500_000;     // ~1.5 MB serialized

  function stateKey() {
    const path = (typeof window !== 'undefined' && window.location?.pathname) || '/browse';
    return `ots:tripleBrowser:${path}`;
  }

  // Map<uri,{nodeIds:Set,edgeIds:Set}> → plain object for JSON.
  function serializeExpandedUris(m) {
    const o = {};
    for (const [k, v] of m) o[k] = { nodeIds: [...v.nodeIds], edgeIds: [...v.edgeIds] };
    return o;
  }
  function deserializeExpandedUris(o) {
    const m = new Map();
    for (const k of Object.keys(o || {})) {
      m.set(k, { nodeIds: new Set(o[k].nodeIds || []), edgeIds: new Set(o[k].edgeIds || []) });
    }
    return m;
  }
  // Map<uri,Set<'in'|'out'>> → plain object.
  function serializeExpandedDirs(m) {
    const o = {};
    for (const [k, v] of m) o[k] = [...v];
    return o;
  }
  function deserializeExpandedDirs(o) {
    const m = new Map();
    for (const k of Object.keys(o || {})) m.set(k, new Set(o[k] || []));
    return m;
  }

  function buildStateSnapshot() {
    const snap = {
      v: STATE_VERSION,
      scopeItems,
      dsVersions,
      scopeGraphs,
      fieldFilters,
      facetChips,
      filterGraph,
      tableSearch,
      page,
      pageSize,
      viewMode,
      // Table rows so the page renders instantly without a fetch.
      triples,
      total,
      hasMore,
      backDatasetId,
      backOrgId,
      backContextName,
    };
    // Attach the graph only when it fits the caps (otherwise re-fetch on demand).
    const elementCount = graphNodes.length + graphEdges.length;
    if (elementCount > 0 && elementCount <= GRAPH_ELEMENT_CAP) {
      snap.graph = {
        nodes: graphNodes,
        edges: graphEdges,
        offset: graphOffset,
        hasMore: graphHasMore,
        expandedUris: serializeExpandedUris(browseExpandedUris),
        expandedDirs: serializeExpandedDirs(browseExpandedDirs),
      };
    }
    return snap;
  }

  let _saveDebounce = null;
  function saveState() {
    if (typeof sessionStorage === 'undefined') return;
    if (_saveDebounce) clearTimeout(_saveDebounce);
    _saveDebounce = setTimeout(flushSaveState, 250);
  }
  function flushSaveState() {
    if (typeof sessionStorage === 'undefined') return;
    if (_saveDebounce) { clearTimeout(_saveDebounce); _saveDebounce = null; }
    try {
      let snap = buildStateSnapshot();
      let json = JSON.stringify(snap);
      if (json.length > STATE_BYTE_CAP && snap.graph) {
        // Too big with the graph — drop it and keep the rest.
        delete snap.graph;
        json = JSON.stringify(snap);
      }
      if (json.length > STATE_BYTE_CAP) return; // still too big — skip silently
      sessionStorage.setItem(stateKey(), json);
    } catch { /* quota / serialization errors are non-fatal */ }
  }

  function loadSavedState() {
    if (typeof sessionStorage === 'undefined') return null;
    try {
      const raw = sessionStorage.getItem(stateKey());
      if (!raw) return null;
      const snap = JSON.parse(raw);
      if (!snap || snap.v !== STATE_VERSION) return null;
      return snap;
    } catch { return null; }
  }

  // Restore the snapshot into reactive state. Returns true on success.
  function restoreState(snap) {
    scopeItems = snap.scopeItems || [];
    dsVersions = snap.dsVersions || {};
    scopeGraphs = snap.scopeGraphs || [];
    fieldFilters = snap.fieldFilters || emptyFieldFilters();
    facetChips = snap.facetChips || [];
    filterGraph = snap.filterGraph || '';
    tableSearch = snap.tableSearch || '';
    page = snap.page || 0;
    pageSize = snap.pageSize || pageSize;
    viewMode = snap.viewMode || 'table';
    triples = snap.triples || [];
    total = typeof snap.total === 'number' ? snap.total : null;
    hasMore = !!snap.hasMore;
    backDatasetId = snap.backDatasetId ?? null;
    backOrgId = snap.backOrgId ?? null;
    backContextName = snap.backContextName ?? null;
    if (snap.graph) {
      graphNodes = snap.graph.nodes || [];
      graphEdges = snap.graph.edges || [];
      graphOffset = snap.graph.offset || 0;
      graphHasMore = !!snap.graph.hasMore;
      browseExpandedUris = deserializeExpandedUris(snap.graph.expandedUris);
      browseExpandedDirs = deserializeExpandedDirs(snap.graph.expandedDirs);
    }
    return true;
  }

  // Reactively re-snapshot whenever any persisted slice changes. Touch every
  // dependency so Svelte re-runs this block; the debounced write coalesces bursts.
  // Skipped until the initial mount settles to avoid clobbering a restore.
  let _persistReady = false;
  $: if (_persistReady) {
    void (scopeItems, dsVersions, scopeGraphs, fieldFilters, facetChips, filterGraph,
     tableSearch, page, pageSize, viewMode, triples, total, hasMore,
     graphNodes, graphEdges, browseExpandedUris, browseExpandedDirs);
    saveState();
  }

  // Persist synchronously when the page is being hidden/unloaded (e.g. the user
  // hits back before the debounce fires) so the snapshot is always current.
  function flushOnHide() { if (_persistReady) flushSaveState(); }
  onMount(() => {
    if (typeof window === 'undefined') return;
    window.addEventListener('pagehide', flushOnHide);
    document.addEventListener('visibilitychange', flushOnHide);
  });
  onDestroy(() => {
    if (typeof window !== 'undefined') {
      window.removeEventListener('pagehide', flushOnHide);
      document.removeEventListener('visibilitychange', flushOnHide);
    }
    flushOnHide(); // client-side route change unmounts us — capture final state
  });

  // Debounced loading: only show the skeleton once a fetch has run past the
  // motion threshold, so quick page/filter changes keep the current rows visible
  // instead of flashing a skeleton (then the new rows animate in via DataTable).
  const tableBusy = delayedLoading();
  const showSkeleton = tableBusy.show;
  $: tableBusy.set(loading);
  onDestroy(() => tableBusy.cancel());

  onMount(async () => {
    // Load prefix.cc prefixes in background (non-blocking)
    loadPrefixCcPrefixes();

    const params = new URLSearchParams(window.location.search);
    // Deep-link params override any saved state — a link should always win so it
    // lands on the requested scope/filter, not a stale snapshot. `view` alone is
    // NOT overriding: switchView() keeps it in the URL via replaceState, so a
    // restored graph session legitimately carries ?view=graph.
    const OVERRIDE_PARAMS = ['graph', 'subject', 'predicate', 'object', 'dataset', 'org', 'version'];
    const hasOverride = OVERRIDE_PARAMS.some(p => params.get(p));

    // ── Restore path: no overriding deep-link + a saved snapshot for this route.
    const saved = hasOverride ? null : loadSavedState();
    if (saved && restoreState(saved)) {
      // Sync the URL's ?view to the restored mode (switchView normally owns this).
      syncViewToUrl(viewMode);
      // Load the picker inventories + the rail facets (scope-derived, cheap, and
      // not part of the persisted snapshot). Crucially we DON'T re-fetch triples,
      // the count, or the graph — those were restored from the snapshot, so the
      // table page and the laid-out graph appear immediately.
      listDatasets().then(ds => { allDatasets = ds || []; }).catch(() => {});
      listOrganisations().then(os => { allOrgs = os || []; }).catch(() => {});
      fetchFacets();
      loadGraphRoles();
      if (uiMode === 'advanced') llmHealth().then((s) => { llmStatus = s; }).catch(() => {});
      // If the graph view was active but the snapshot dropped the (too-large)
      // graph payload, fall back to a fresh fetch so the view isn't empty.
      if (viewMode === 'graph' && graphNodes.length === 0 && graphEdges.length === 0) {
        fetchGraphData();
      }
      _persistReady = true; // begin saving on subsequent changes
      return;
    }

    // ── Normal path (fresh load or deep-link) ───────────────────────────────
    // A ?graph= drill-down stays a dedicated single-graph scope; subject/
    // predicate/object deep-links become exact filter chips.
    if (params.get('graph')) filterGraph = params.get('graph');
    for (const f of ['subject', 'predicate', 'object']) {
      const v = params.get(f);
      if (v) fieldFilters[f] = { value: v, mode: 'exact' };
    }
    fieldFilters = { ...fieldFilters };
    const v = params.get('view');
    if (v === 'graph') viewMode = v;
    backDatasetId = params.get('dataset') || null;
    backOrgId = params.get('org') || null;
    // Carry a ?version= pin from an explore-tile link for the single dataset.
    const urlVersion = params.get('version');
    if (urlVersion && backDatasetId) {
      dsVersions = { ...dsVersions, [backDatasetId]: urlVersion };
      ensureDsVersions(backDatasetId);
    }
    // Pre-populate scope from URL params
    if (backDatasetId) {
      scopeItems = [{ type: 'dataset', id: backDatasetId, name: backDatasetId }];
      getDataset(backDatasetId)
        .then(d => { backContextName = d?.name ?? backDatasetId; scopeItems = [{ type: 'dataset', id: backDatasetId, name: backContextName }]; })
        .catch(() => { backContextName = backDatasetId; });
    } else if (backOrgId) {
      scopeItems = [{ type: 'org', id: backOrgId, name: backOrgId }];
      getOrganisation(backOrgId)
        .then(o => { backContextName = o?.name ?? backOrgId; scopeItems = [{ type: 'org', id: backOrgId, name: backContextName }]; })
        .catch(() => { backContextName = backOrgId; });
    }
    // Load all available datasets/orgs for the scope picker
    listDatasets().then(ds => { allDatasets = ds || []; }).catch(() => {});
    listOrganisations().then(os => { allOrgs = os || []; }).catch(() => {});
    fetchTriples();
    // Compute the exact total up-front so users see "Page X of Y" immediately
    // (and the Last/jump-to-page controls work). The backend caps the count.
    fetchExactCount();
    // Facets + graph roles power the always-visible rail across every view.
    fetchFacets();
    loadGraphRoles();
    if (uiMode === 'advanced') llmHealth().then((s) => { llmStatus = s; }).catch(() => {});
    if (viewMode === 'graph') fetchGraphData();
    _persistReady = true; // start persisting once the initial load is in flight
  });

  // Build the params common to both the page fetch and the count fetch so the
  // backend filter set stays consistent.
  // Scope (dataset/org) + per-dataset version params, shared by every fetch path.
  function buildScopeParams() {
    const params = {};
    const dsIds = scopeItems.filter(s => s.type === 'dataset').map(s => s.id);
    const orgIds = scopeItems.filter(s => s.type === 'org').map(s => s.id);
    if (dsIds.length === 1 && orgIds.length === 0) {
      params.dataset_id = dsIds[0]; // backward compat — single dataset
    } else if (dsIds.length > 1) {
      params.dataset_ids = dsIds.join(',');
      if (orgIds.length > 0) params.org_id = orgIds[0]; // first org included via org_id
    } else if (dsIds.length === 0 && orgIds.length === 1) {
      params.org_id = orgIds[0];
    } else if (dsIds.length === 0 && orgIds.length > 1) {
      params.org_id = orgIds[0]; // MVP: only first org (multi-org backend support is future work)
    }
    // Per-dataset version pins (datasetId:version) for any scoped dataset.
    const verPairs = scopedDatasetIds
      .filter(id => isPinned(dsVersions[id]))
      .map(id => `${id}:${dsVersions[id]}`);
    if (verPairs.length) params.versions = verPairs.join(',');
    return params;
  }

  // Scope-only params for graph expansion: dataset/org + version pins (+ single-graph
  // drill-down). Deliberately omits the user's content chips and quick-search, so
  // expanding a node reveals ALL of its edges within scope rather than re-narrowing it.
  function buildExpandScopeParams() {
    const params = buildScopeParams();
    if (filterGraph) params.graph = filterGraph;
    return params;
  }

  // One value per field (subject/predicate/object/graph/vocabulary), each with its
  // own match mode (contains/exact/regex). `activeChips` is the array form sent to
  // the backend and used for facet selection + the SPARQL preview; `currentChips()`
  // reads it synchronously so a refetch fired in the same tick isn't stale.
  const FILTER_FIELDS = ['subject', 'predicate', 'object', 'graph', 'vocabulary'];
  const FILTER_MODES = ['contains', 'exact', 'regex'];
  const MODE_GLYPH = { contains: '≈', exact: '=', regex: '.*' };
  $: MODE_LABEL = { contains: $i18nT('pages.tripleBrowser.modeContains'), exact: $i18nT('pages.tripleBrowser.modeExact'), regex: $i18nT('pages.tripleBrowser.modeRegex') };
  $: FIELD_LABEL = { subject: $i18nT('pages.tripleBrowser.subject'), predicate: $i18nT('pages.tripleBrowser.predicate'), object: $i18nT('pages.tripleBrowser.object'), graph: $i18nT('pages.tripleBrowser.graph'), vocabulary: $i18nT('pages.tripleBrowser.vocabulary') };
  $: FIELD_PLACEHOLDER = {
    subject: $i18nT('pages.tripleBrowser.subjectPlaceholder'), predicate: $i18nT('pages.tripleBrowser.predicatePlaceholder'), object: $i18nT('pages.tripleBrowser.objectPlaceholder'),
    graph: $i18nT('pages.tripleBrowser.graphFilterPlaceholder'), vocabulary: $i18nT('pages.tripleBrowser.vocabularyPlaceholder'),
  };
  const emptyFieldFilters = () => ({
    subject:    { value: '', mode: 'contains', neg: false },
    predicate:  { value: '', mode: 'contains', neg: false },
    object:     { value: '', mode: 'contains', neg: false },
    graph:      { value: '', mode: 'contains', neg: false },
    vocabulary: { value: '', mode: 'contains', neg: false },
  });
  let fieldFilters = emptyFieldFilters();
  // Facets are a multi-select: each selected facet contributes a chip here, and
  // these merge with the typed form fields. Clicking a facet toggles its chip.
  let facetChips = [];
  const chipEq = (a, b) => a.field === b.field && a.value === b.value && a.mode === b.mode && !!a.neg === !!b.neg;
  const chipsFromFields = (ff) => FILTER_FIELDS
    .filter((f) => ff[f].value && ff[f].value.trim())
    .map((f) => ({ field: f, value: ff[f].value.trim(), mode: ff[f].mode, neg: !!ff[f].neg }));
  function combineChips(ff = fieldFilters, fc = facetChips) {
    const merged = chipsFromFields(ff);
    for (const c of fc) if (!merged.some((x) => chipEq(x, c))) merged.push(c);
    return merged;
  }
  $: activeChips = combineChips(fieldFilters, facetChips);
  const currentChips = () => combineChips();

  function buildFilterParams() {
    const params = {};
    Object.assign(params, buildScopeParams());
    if (filterGraph) params.graph = filterGraph;
    // Scope-graph selection → exact `graph` filter chips. The backend ORs chips
    // on the same field, so multiple selected graphs narrow the browse to their
    // union; they AND against the user's typed/facet chips. ACL is enforced
    // server-side (a graph chip forces the ?g-binding, access-checked branch).
    const graphChips = scopeGraphs.map(iri => ({ field: 'graph', value: iri, mode: 'exact' }));
    const active = [...currentChips(), ...graphChips];
    if (active.length) params.filters = JSON.stringify(active);
    const q = tableSearch.trim();
    if (q) params.q = q;
    return params;
  }

  // ── Shared refetch paths ───────────────────────────────────────────────────
  // Results-only: table rows + count (+ graph sample). Used when the query
  // narrows but the set of available facets in scope is unchanged (q, chips).
  function refetchResults() {
    page = 0;
    total = null;
    fetchTriples();
    fetchExactCount();
    if (viewMode === 'graph') fetchGraphData();
  }
  // Full: also re-scan facets and graph roles. Used when scope/version changes.
  function refetchAll() {
    loadGraphRoles();
    fetchFacets();
    refetchResults();
  }

  // ── Filter field interactions ────────────────────────────────────────────────
  let _filterDebounce = null;
  function onFilterInput(field) {
    scheduleFieldSuggest(field);
    if (_filterDebounce) clearTimeout(_filterDebounce);
    _filterDebounce = setTimeout(() => refetchResults(), 300);
  }
  function applyFiltersNow() {
    if (_filterDebounce) clearTimeout(_filterDebounce);
    refetchResults();
  }
  // Cycle a field's match mode: contains → exact → regex → contains.
  function cycleFieldMode(field) {
    const cur = fieldFilters[field].mode;
    fieldFilters[field].mode = FILTER_MODES[(FILTER_MODES.indexOf(cur) + 1) % FILTER_MODES.length];
    fieldFilters = { ...fieldFilters };
    if (fieldFilters[field].value.trim()) refetchResults();
  }
  // Toggle a field's negation. When on, rows that MATCH the value are excluded
  // (the backend wraps the clause in `!(…)`), turning any mode into a "not
  // equal" / "not contains" / "not matching" exclusion to filter elements out.
  function toggleFieldNeg(field) {
    fieldFilters[field].neg = !fieldFilters[field].neg;
    fieldFilters = { ...fieldFilters };
    if (fieldFilters[field].value.trim()) refetchResults();
  }
  function clearField(field) {
    fieldFilters[field].value = '';
    fieldFilters = { ...fieldFilters };
    refetchResults();
  }
  // Toggle a facet selection: if the facet's chip(s) are already selected, remove
  // them; otherwise add them. Lets the sidebar act as a multi-select.
  function toggleFacet(chips) {
    const allPresent = chips.every((nc) => facetChips.some((c) => chipEq(c, nc)));
    if (allPresent) {
      facetChips = facetChips.filter((c) => !chips.some((nc) => chipEq(c, nc)));
    } else {
      const merged = [...facetChips];
      for (const nc of chips) if (!merged.some((c) => chipEq(c, nc))) merged.push(nc);
      facetChips = merged;
    }
    refetchResults();
  }
  function handleFacetAdd(e) { toggleFacet(e.detail); }

  // ── Per-field autosuggest (datalists) ────────────────────────────────────────
  let fieldSuggestions = { subject: [], predicate: [], object: [], graph: [], vocabulary: [] };
  let _sugTimers = {};
  function scheduleFieldSuggest(field) {
    clearTimeout(_sugTimers[field]);
    const q = fieldFilters[field].value.trim();
    if (q.length < 2) { fieldSuggestions[field] = []; fieldSuggestions = { ...fieldSuggestions }; return; }
    _sugTimers[field] = setTimeout(async () => {
      fieldSuggestions[field] = await fieldSuggest(field, q);
      fieldSuggestions = { ...fieldSuggestions };
    }, 300);
  }
  // Substring-match the in-scope facets (the backend SUGGEST only prefix-matches,
  // useless for the local part); fall back to the backend for subjects/objects.
  async function fieldSuggest(field, value) {
    const q = value.toLowerCase();
    const matchFacet = (list) => (list || [])
      .map((x) => x.iri)
      .filter((iri) => iri && (iri.toLowerCase().includes(q) || (shortenIRI(iri) || '').toLowerCase().includes(q)))
      .slice(0, 12);
    if (field === 'predicate') { const l = matchFacet(rawFacets.properties); if (l.length) return l; }
    else if (field === 'graph') { const l = matchFacet(rawFacets.graphs); if (l.length) return l; }
    else if (field === 'vocabulary') {
      const ns = new Set();
      for (const t of [...(rawFacets.classes || []), ...(rawFacets.properties || [])]) {
        if (!t.iri) continue;
        const h = t.iri.lastIndexOf('#'), s = t.iri.lastIndexOf('/');
        const n = t.iri.slice(0, Math.max(h, s) + 1);
        if (n && (n.toLowerCase().includes(q) || (shortenIRI(n) || '').toLowerCase().includes(q))) ns.add(n);
      }
      return [...ns].slice(0, 12);
    } else if (field === 'object') {
      const local = matchFacet(rawFacets.classes);
      try {
        const res = await browseSuggest(field, value, 12);
        const remote = (res.values || []).map((v) => v.value).filter(Boolean);
        return [...new Set([...local, ...remote])].slice(0, 12);
      } catch { return local; }
    }
    try {
      const res = await browseSuggest(field, value, 12);
      return (res.values || []).map((v) => v.value).filter(Boolean);
    } catch { return []; }
  }

  // ── Facets ───────────────────────────────────────────────────────────────────
  async function fetchFacets() {
    const seq = ++_facetsSeq;
    facetsLoading = true;
    try {
      const res = await browseFacets(buildScopeParams());
      if (seq !== _facetsSeq) return; // superseded
      rawFacets = {
        classes: res.classes || [],
        properties: res.properties || [],
        graphs: res.graphs || [],
      };
    } catch {
      if (seq === _facetsSeq) rawFacets = { classes: [], properties: [], graphs: [] };
    } finally {
      if (seq === _facetsSeq) facetsLoading = false;
    }
  }

  // Declared graph roles for every dataset in scope (instances/model/vocabulary/…).
  async function loadGraphRoles() {
    const ids = scopedDatasetIds;
    if (!ids.length) { graphRoleMap = {}; return; }
    const map = {};
    await Promise.all(ids.map(async (id) => {
      try {
        const gs = await listDatasetGraphs(id);
        for (const g of gs || []) {
          const r = normalizeGraphRole(g.graph_role);
          if (g.graph_iri && r) map[g.graph_iri] = { role: r, label: graphRoleLabel(r) };
        }
      } catch {}
    }));
    graphRoleMap = map;
  }

  // ── Live SPARQL preview (Advanced / DX) ──────────────────────────────────────
  function chipExpr(varName, c) {
    if (c.mode === 'regex') return `REGEX(STR(?${varName}), "${c.value}", "i")`;
    if (c.mode === 'exact') {
      const isLiteral = c.field === 'object' && !/^(https?:|urn:)/.test(c.value);
      return isLiteral ? `str(?${varName}) = "${c.value}"` : `?${varName} = <${c.value}>`;
    }
    return `CONTAINS(LCASE(STR(?${varName})), LCASE("${c.value}"))`;
  }
  // A vocabulary value matches when the namespace appears in subject/predicate/object.
  function vocabExpr(c) {
    const per = (v) => c.mode === 'regex' ? `REGEX(STR(?${v}), "${c.value}", "i")`
      : c.mode === 'exact' ? `STRSTARTS(STR(?${v}), "${c.value}")`
      : `CONTAINS(LCASE(STR(?${v})), LCASE("${c.value}"))`;
    return `(${['s', 'p', 'o'].map(per).join(' || ')})`;
  }
  $: sparqlPreview = (() => {
    const byVar = { subject: 's', predicate: 'p', object: 'o', graph: 'g' };
    const lines = ['SELECT ?s ?p ?o ?g WHERE {', '  GRAPH ?g { ?s ?p ?o .'];
    for (const c of activeChips) {
      const e = c.field === 'vocabulary' ? vocabExpr(c) : chipExpr(byVar[c.field], c);
      lines.push(`    FILTER(${c.neg ? `!(${e})` : e})`);
    }
    const q = tableSearch.trim();
    if (q) {
      const n = `LCASE("${q}")`;
      lines.push(`    FILTER(CONTAINS(LCASE(STR(?s)), ${n}) || CONTAINS(LCASE(STR(?p)), ${n}) || CONTAINS(LCASE(STR(?o)), ${n}))`);
    }
    lines.push('  }');
    lines.push(`} LIMIT ${pageSize}`);
    return lines.join('\n');
  })();
  function copySparql() { void copyToClipboard(sparqlPreview); }
  function openInSparqlEditor() { navigate(`/sparql?query=${encodeURIComponent(sparqlPreview)}`); }

  // Monotonic request tokens: facet clicks / chip edits can fire overlapping
  // fetches, so only the newest response for each path is allowed to apply.
  let _triplesSeq = 0;
  let _countSeq = 0;
  let _facetsSeq = 0;

  async function fetchTriples() {
    const seq = ++_triplesSeq;
    loading = true;
    error = '';
    try {
      const params = {
        limit:  pageSize.toString(),
        offset: (page * pageSize).toString(),
        ...buildFilterParams(),
      };

      const res = await browseTriples(params);
      if (seq !== _triplesSeq) return; // superseded by a newer fetch
      triples = res.triples || [];
      hasMore = !!res.hasMore;
      if (typeof res.total === 'number') total = res.total;
    } catch (e) {
      if (seq !== _triplesSeq) return;
      error = `${e.status ? `HTTP ${e.status}: ` : ''}${e.message}`;
      triples = [];
      hasMore = false;
    } finally {
      if (seq === _triplesSeq) loading = false;
    }
  }

  async function fetchExactCount() {
    const seq = ++_countSeq;
    loadingCount = true;
    try {
      const params = {
        limit:  pageSize.toString(),
        offset: (page * pageSize).toString(),
        count:  'true',
        ...buildFilterParams(),
      };
      const res = await browseTriples(params);
      if (seq !== _countSeq) return; // superseded
      // An explicit count always resolves to a number — when the caller has no
      // graphs in scope the backend reports 0 rather than omitting the field, so
      // the badge shows "0 triples" instead of staying on "Show total".
      total = typeof res.total === 'number' ? res.total : 0;
    } catch {}
    finally { if (seq === _countSeq) loadingCount = false; }
  }

  function clearFilters() {
    fieldFilters = emptyFieldFilters();
    facetChips = [];
    fieldSuggestions = { subject: [], predicate: [], object: [], graph: [], vocabulary: [] };
    filterGraph = '';
    tableSearch = '';
    scopeGraphs = []; // graph narrowing is part of the active filter set
    refetchResults();
  }

  // Dataset IDs implied by a given scope-item array (datasets + org-owned). Mirrors
  // the reactive `scopedDatasetIds` but for an arbitrary `items` snapshot, so scope
  // mutations can prune selected graphs synchronously before refetching.
  function datasetIdsForScope(items) {
    const ids = new Set();
    for (const s of items) {
      if (s.type === 'dataset') ids.add(s.id);
      else if (s.type === 'org') {
        for (const d of allDatasets) {
          if (d.owner_type === 'organisation' && String(d.owner_id) === String(s.id)) ids.add(d.id);
        }
      }
    }
    return [...ids];
  }
  // Keep only selected graphs that still belong to one of `dsIds`. Skip pruning
  // while any target dataset's graph list is still loading — the reactive prune
  // ($: block) finishes the job once everything has arrived, so we never drop a
  // still-valid graph on stale (unloaded) data.
  function pruneScopeGraphsTo(dsIds) {
    if (dsIds.some(id => !Array.isArray(graphsByDs[id]))) return;
    const avail = new Set();
    for (const id of dsIds) for (const g of (graphsByDs[id] || [])) if (g.graph_iri) avail.add(g.graph_iri);
    scopeGraphs = scopeGraphs.filter(iri => avail.has(iri));
  }

  function clearDatasetScope() {
    scopeItems = [];
    scopeGraphs = []; // no scope → no graph narrowing
    refetchAll();
  }

  function removeScopeItem(item) {
    scopeItems = scopeItems.filter(s => !(s.type === item.type && s.id === item.id));
    pruneScopeGraphsTo(datasetIdsForScope(scopeItems)); // drop graphs that left scope
    refetchAll();
  }

  function addScopeItem(item) {
    if (!scopeItems.some(s => s.type === item.type && s.id === item.id)) {
      scopeItems = [...scopeItems, item];
      refetchAll();
    }
    scopePickerOpen = false;
    scopeSearch = '';
  }

  // Svelte action: call callback when a click happens outside the node
  function clickOutside(node, callback) {
    const handle = (e) => { if (!node.contains(e.target)) callback(); };
    document.addEventListener('click', handle, true);
    return { destroy() { document.removeEventListener('click', handle, true); } };
  }

  function nextPage() { page++; fetchTriples(); }
  function prevPage() { if (page > 0) { page--; fetchTriples(); } }
  function lastPage() {
    if (totalPages == null) return;
    page = totalPages - 1;
    fetchTriples();
  }
  function firstPage() { if (page > 0) { page = 0; fetchTriples(); } }
  function gotoPage(n) {
    if (totalPages == null) return;
    const target = Math.max(0, Math.min(totalPages - 1, n - 1));
    if (target === page) return;
    page = target;
    fetchTriples();
  }

  $: totalPages = total != null ? Math.max(1, Math.ceil(total / pageSize)) : null;
  $: hasFilters = activeChips.length > 0 || !!filterGraph || scopeGraphs.length > 0;
  $: hasActiveFilters = hasFilters || !!tableSearch.trim();
</script>

<div class="browser">
  <PageHeader
    title={$i18nT('pages.tripleBrowser.title')}
    breadcrumbs={backOrgId
      ? [{ label: $i18nT('pages.tripleBrowser.organisations'), href: '/organisations' }, { label: backContextName ?? '…', href: '/organisations/' + backOrgId }, { label: $i18nT('pages.tripleBrowser.title') }]
      : backDatasetId
        ? [{ label: $i18nT('pages.tripleBrowser.datasets'), href: '/datasets' }, { label: backContextName ?? '…', href: '/datasets/' + backDatasetId }, { label: $i18nT('pages.tripleBrowser.title') }]
        : [{ label: $i18nT('pages.tripleBrowser.datasets'), href: '/datasets' }, { label: $i18nT('pages.tripleBrowser.title') }]}
  />

  <div class="card browser-card">
    <!-- ─── Shared header ──────────────────────────────────────────────────── -->
    <div class="card-header">
      <div class="view-toggle">
        <button class="vtoggle-btn" class:vtoggle-active={viewMode === 'table'}
          on:click={() => switchView('table')} title={$i18nT('pages.tripleBrowser.tableView')}
        ><Table2 size={14} /><span class="vtoggle-label">{$i18nT('pages.tripleBrowser.tableLabel')}</span></button>
        <button class="vtoggle-btn" class:vtoggle-active={viewMode === 'graph'}
          on:click={() => switchView('graph')} title={$i18nT('pages.tripleBrowser.graphView')}
        ><Network size={14} /><span class="vtoggle-label">{$i18nT('pages.tripleBrowser.graph')}</span></button>
        {#if canMap}
          <button class="vtoggle-btn" class:vtoggle-active={viewMode === 'map'}
            on:click={() => switchView('map')} title={$i18nT('pages.tripleBrowser.mapView')}
          >{#if scopeHas3d}<Boxes size={14} />{:else}<MapIcon size={14} />{/if}<span class="vtoggle-label">{scopeHas3d ? $i18nT('pages.tripleBrowser.mapAnd3dLabel') : $i18nT('pages.tripleBrowser.mapLabel')}</span></button>
        {/if}
      </div>

      <span class="count-badge">
        {#if viewMode === 'table'}
          {#if loading}…{:else if total != null}
            {total.toLocaleString()} {$i18nT('pages.tripleBrowser.triples')}
          {:else}
            <button class="count-reveal-btn" on:click={fetchExactCount} disabled={loadingCount} title={$i18nT('pages.tripleBrowser.computeExactCount')}>
              {loadingCount ? $i18nT('pages.tripleBrowser.counting') : $i18nT('pages.tripleBrowser.showTotal')}
            </button>
          {/if}
          {#if hasFilters} · {$i18nT('pages.tripleBrowser.filteredWord')}{/if}
        {:else if viewMode === 'graph'}
          {(graphLoading || graphLoadingMore) ? '…' : `${graphNodes.length} ${$i18nT('pages.tripleBrowser.nodes')} · ${graphEdges.length} ${$i18nT('pages.tripleBrowser.edges')}`}
        {/if}
      </span>

      <div class="header-spacer"></div>

      <div class="mode-toggle" role="tablist" aria-label={$i18nT('pages.tripleBrowser.interfaceMode')}>
        <button class="mode-btn" class:mode-active={uiMode === 'simple'}
          on:click={() => setUiMode('simple')} role="tab" aria-selected={uiMode === 'simple'}
          title={$i18nT('pages.tripleBrowser.simpleModeHint')}>
          <Sparkles size={13} /> <span class="mode-label">{$i18nT('pages.tripleBrowser.simpleMode')}</span>
        </button>
        <button class="mode-btn" class:mode-active={uiMode === 'advanced'}
          on:click={() => setUiMode('advanced')} role="tab" aria-selected={uiMode === 'advanced'}
          title={$i18nT('pages.tripleBrowser.advancedModeHint')}>
          <SlidersHorizontal size={13} /> <span class="mode-label">{$i18nT('pages.tripleBrowser.advancedMode')}</span>
        </button>
      </div>

      {#if versionableDatasets.length > 0}
        <div class="version-ctl" use:clickOutside={() => versionPanelOpen = false}>
          <button class="btn btn-sm" class:version-pinned={pinnedVersionCount > 0}
            on:click|stopPropagation={() => versionPanelOpen = !versionPanelOpen}
            title={$i18nT('pages.tripleBrowser.versionSnapshotHint')}>
            <History size={14} /> {pinnedVersionCount > 0 ? `${$i18nT('pages.tripleBrowser.versions')} (${pinnedVersionCount})` : $i18nT('pages.tripleBrowser.versions')}
          </button>
          {#if versionPanelOpen}
            <div class="version-panel" on:click|stopPropagation role="presentation" transition:slide={{ duration: 120 }}>
              <div class="vp-head">{$i18nT('pages.tripleBrowser.versionPerDataset')}</div>
              {#each versionableDatasets as id (id)}
                <div class="vp-row">
                  <span class="vp-name" title={dsNameById[id] || id}>{dsNameById[id] || id}</span>
                  <Select size="sm"
                    class="vp-select {isPinned(dsVersions[id]) ? 'vp-pinned' : ''}"
                    value={dsVersions[id] || ''}
                    on:change={(e) => { dsVersions = { ...dsVersions, [id]: e.detail }; onVersionChange(); }}
                    options={[{ value: '', label: $i18nT('pages.tripleBrowser.liveCurrent') }, ...sortedVersions(id).map(v => ({ value: v.version, label: `v${v.version}${v.status && v.status !== 'published' ? ` · ${v.status}` : ''}` }))]} />
                </div>
              {/each}
            </div>
          {/if}
        </div>
      {/if}

      <button class="btn btn-sm btn-export" on:click={() => exportModalOpen = true}>
        <Download size={14} /> {$i18nT('system.export')}
      </button>
    </div>

    <!-- ─── Shared scope · search · filters (apply to every view) ──────────── -->
    <!-- Scope bar: shown when scope items present or picker is open -->
      {#if scopeItems.length > 0 || scopePickerOpen}
        <div class="dataset-scope-bar">
          <LayoutList size={13} />
          <span class="dataset-scope-label">{$i18nT('pages.tripleBrowser.scopeLabel')}</span>
          {#each scopeItems as item}
            <span class="dataset-scope-chip">
              {#if item.type === 'org'}<Building2 size={11} />{:else}<Database size={11} />{/if}
              {item.name}
              <button class="dataset-scope-x" on:click={() => removeScopeItem(item)} title={$i18nT('pages.tripleBrowser.removeScope')}><X size={11} /></button>
            </span>
          {/each}
          <div class="scope-picker-wrap" use:clickOutside={() => { scopePickerOpen = false; scopeSearch = ''; }}>
            <button class="scope-add-btn" on:click|stopPropagation={() => { scopePickerOpen = !scopePickerOpen; scopeSearch = ''; }}>
              <Plus size={12} /> {$i18nT('system.add')}
            </button>
            {#if scopePickerOpen}
              <div class="scope-picker" transition:slide={{ duration: 120 }}>
                <input
                  class="scope-search"
                  placeholder={$i18nT('pages.tripleBrowser.searchEllipsis')}
                  bind:value={scopeSearch}
                  on:click|stopPropagation
                  use:autofocus
                />
                {#if allOrgs.filter(o => !scopeItems.some(s => s.type === 'org' && s.id === o.id) && (!scopeSearch || o.name.toLowerCase().includes(scopeSearch.toLowerCase()))).length > 0}
                  <div class="scope-group-label">{$i18nT('pages.tripleBrowser.organisations')}</div>
                  {#each allOrgs.filter(o => !scopeItems.some(s => s.type === 'org' && s.id === o.id) && (!scopeSearch || o.name.toLowerCase().includes(scopeSearch.toLowerCase()))) as org}
                    <button class="scope-item" on:click={() => addScopeItem({ type: 'org', id: org.id, name: org.name })}>
                      <Building2 size={12} /> {org.name}
                    </button>
                  {/each}
                {/if}
                {#if allDatasets.filter(d => !scopeItems.some(s => s.type === 'dataset' && s.id === d.id) && (!scopeSearch || d.name.toLowerCase().includes(scopeSearch.toLowerCase()))).length > 0}
                  <div class="scope-group-label">{$i18nT('pages.tripleBrowser.datasets')}</div>
                  {#each allDatasets.filter(d => !scopeItems.some(s => s.type === 'dataset' && s.id === d.id) && (!scopeSearch || d.name.toLowerCase().includes(scopeSearch.toLowerCase()))) as ds}
                    <button class="scope-item" on:click={() => addScopeItem({ type: 'dataset', id: ds.id, name: ds.name })}>
                      <Database size={12} /> {ds.name}
                    </button>
                  {/each}
                {/if}
                {#if allOrgs.length === 0 && allDatasets.length === 0}
                  <div class="scope-empty">{$i18nT('system.loading')}</div>
                {:else if allOrgs.filter(o => !scopeItems.some(s => s.type === 'org' && s.id === o.id) && (!scopeSearch || o.name.toLowerCase().includes(scopeSearch.toLowerCase()))).length === 0 && allDatasets.filter(d => !scopeItems.some(s => s.type === 'dataset' && s.id === d.id) && (!scopeSearch || d.name.toLowerCase().includes(scopeSearch.toLowerCase()))).length === 0}
                  <div class="scope-empty">{$i18nT('pages.tripleBrowser.noResultsShort')}</div>
                {/if}
              </div>
            {/if}
          </div>
          {#if scopeItems.length > 0}
            <button class="scope-clear-all" on:click={clearDatasetScope}>{$i18nT('pages.tripleBrowser.clearAll')}</button>
          {/if}

          <!-- GRAPHS: narrow the browse to named graphs of the scoped datasets. -->
          {#if scopedDatasetIds.length > 0 && (availableScopeGraphs.length > 0 || scopeGraphs.length > 0 || scopeGraphsLoading)}
            <div class="scope-graph-row">
              <Network size={13} />
              <span class="dataset-scope-label">{$i18nT('pages.tripleBrowser.graph')}:</span>
              {#each scopeGraphs as iri}
                <span class="dataset-scope-chip graph-scope-chip" title={iri}>
                  <Network size={11} />
                  {shortenIRI(iri)}
                  <button class="dataset-scope-x" on:click={() => removeScopeGraph(iri)} title={$i18nT('pages.tripleBrowser.removeScope')}><X size={11} /></button>
                </span>
              {/each}
              {#if availableScopeGraphs.length > 0}
                <div class="scope-picker-wrap" use:clickOutside={() => { graphScopeOpen = false; }}>
                  <button class="scope-add-btn" on:click|stopPropagation={() => { graphScopeOpen = !graphScopeOpen; }}>
                    <Plus size={12} /> {$i18nT('system.add')}
                  </button>
                  {#if graphScopeOpen}
                    <div class="scope-picker" transition:slide={{ duration: 120 }}>
                      <div class="scope-group-label">{$i18nT('pages.tripleBrowser.graph')}</div>
                      {#each availableScopeGraphs as g}
                        <button class="scope-item graph-scope-item" class:graph-scope-selected={scopeGraphs.includes(g.graph_iri)}
                          on:click|stopPropagation={() => toggleScopeGraph(g.graph_iri)} title={g.graph_iri}>
                          <span class="gsi-check">{scopeGraphs.includes(g.graph_iri) ? '✓' : ''}</span>
                          <Network size={12} />
                          <span class="gsi-name">{shortenIRI(g.graph_iri)}</span>
                          {#if typeof g.triple_count === 'number' && g.triple_count > 0}
                            <span class="gsi-count">{g.triple_count.toLocaleString()}</span>
                          {/if}
                        </button>
                      {/each}
                    </div>
                  {/if}
                </div>
              {:else if scopeGraphsLoading}
                <span class="scope-empty">{$i18nT('system.loading')}</span>
              {/if}
              {#if scopeGraphs.length > 0}
                <button class="scope-clear-all" on:click={clearScopeGraphs}>{$i18nT('pages.tripleBrowser.clearAll')}</button>
              {/if}
            </div>
          {/if}
        </div>
      {:else}
        <div class="scope-add-row">
          <button class="scope-add-btn-outline" on:click={() => { scopePickerOpen = true; }}>
            <LayoutList size={12} /> {$i18nT('pages.tripleBrowser.addScope')}
          </button>
        </div>
      {/if}

      <!-- Global free-text search (q): identical in Simple & Advanced. Supports
           AND / OR / XOR / NOT, parentheses and "quoted phrases" across all columns. -->
      <div class="table-search-bar">
        <div class="table-search-input-wrap">
          <Search size={13} class="search-icon" />
          <input
            class="table-search-input"
            placeholder={$i18nT('pages.tripleBrowser.globalSearchPlaceholder')}
            bind:value={tableSearch}
            on:input={scheduleTableSearch}
            on:keydown={(e) => { if (e.key === 'Enter') { e.preventDefault(); if (tableSearchDebounce) { clearTimeout(tableSearchDebounce); tableSearchDebounce = null; } refetchResults(); } }}
          />
          {#if tableSearch}
            <button class="search-clear" on:click={clearTableSearch} title={$i18nT('pages.tripleBrowser.clearSearch')}><X size={12} /></button>
          {/if}
        </div>
        <div class="syntax-help-wrap" use:clickOutside={() => (syntaxHelpOpen = false)}>
          <button class="syntax-help-btn" on:click|stopPropagation={() => (syntaxHelpOpen = !syntaxHelpOpen)} title={$i18nT('pages.tripleBrowser.searchSyntaxHelp')} aria-label={$i18nT('pages.tripleBrowser.searchSyntaxHelp')}>
            <HelpCircle size={15} />
          </button>
          {#if syntaxHelpOpen}
            <div class="syntax-help" transition:slide={{ duration: 120 }}>
              <div class="sh-title">{$i18nT('pages.tripleBrowser.searchSyntax')}</div>
              <ul class="sh-list">
                <li><code>bridge tunnel</code> — {$i18nT('pages.tripleBrowser.syntaxBothWords')} (<b>AND</b>)</li>
                <li><code>bridge OR tunnel</code> — {$i18nT('pages.tripleBrowser.syntaxEitherWord')}</li>
                <li><code>bridge XOR tunnel</code> — {$i18nT('pages.tripleBrowser.syntaxOneNotBoth')}</li>
                <li><code>bridge NOT concrete</code> — {$i18nT('pages.tripleBrowser.syntaxExcludeWord')}</li>
                <li><code>(steel OR concrete) bridge</code> — {$i18nT('pages.tripleBrowser.syntaxGroupParens')}</li>
                <li><code>"steel bridge"</code> — {$i18nT('pages.tripleBrowser.syntaxExactPhrase')}</li>
              </ul>
              <p class="sh-note">{$i18nT('pages.tripleBrowser.syntaxNote')}</p>
              <a class="sh-link" href="/docs/search-syntax" on:click={() => (syntaxHelpOpen = false)}>{$i18nT('pages.tripleBrowser.fullDocumentation')}</a>
            </div>
          {/if}
        </div>
        <button class="btn-adv-toggle" class:btn-adv-active={filtersOpen} on:click={() => filtersOpen = !filtersOpen} title={$i18nT('pages.tripleBrowser.fieldFilters')}>
          <Filter size={13} /> {$i18nT('pages.tripleBrowser.filters')}
          {#if activeChips.length}<span class="adv-badge">{activeChips.length}</span>{/if}
          {filtersOpen ? '▲' : '▼'}
        </button>
        {#if uiMode === 'advanced'}
          <button class="btn-adv-toggle" class:btn-adv-active={sparqlPreviewOpen} on:click={() => sparqlPreviewOpen = !sparqlPreviewOpen} title={$i18nT('pages.tripleBrowser.showEquivalentSparql')}>
            <Code2 size={13} /> SPARQL
          </button>
        {/if}
        {#if hasActiveFilters}
          <button class="btn btn-sm btn-ghost" on:click={clearFilters} title={$i18nT('pages.tripleBrowser.clearSearchAndFilters')}><X size={13}/> {$i18nT('system.clear')}</button>
        {/if}
      </div>

      {#if filtersOpen}
        <div class="filter-form" transition:slide={{ duration: 140 }}>
          {#each FILTER_FIELDS as f}
            <div class="ff-row">
              <span class="ff-label">{FIELD_LABEL[f]}</span>
              <button type="button" class="ff-mode ff-mode-{fieldFilters[f].mode}"
                on:click={() => cycleFieldMode(f)}
                title={`${$i18nT('pages.tripleBrowser.matchPrefix')}: ${MODE_LABEL[fieldFilters[f].mode]} — ${$i18nT('pages.tripleBrowser.clickToChange')}`}>
                {MODE_GLYPH[fieldFilters[f].mode]}
              </button>
              <button type="button" class="ff-neg" class:ff-neg-on={fieldFilters[f].neg}
                on:click={() => toggleFieldNeg(f)}
                aria-pressed={fieldFilters[f].neg}
                title={fieldFilters[f].neg ? $i18nT('pages.tripleBrowser.negOnHint') : $i18nT('pages.tripleBrowser.negOffHint')}>
                {$i18nT('pages.tripleBrowser.modeNot')}
              </button>
              <div class="ff-input-wrap">
                <Combobox
                  class="ff-input"
                  suggestions={(fieldSuggestions[f] || []).map(s => s)}
                  placeholder={fieldFilters[f].mode === 'regex' ? $i18nT('pages.tripleBrowser.regexPlaceholder') : FIELD_PLACEHOLDER[f]}
                  bind:value={fieldFilters[f].value}
                  on:input={() => onFilterInput(f)}
                  on:change={applyFiltersNow}
                />
                {#if fieldFilters[f].value}
                  <button class="ff-clear" on:click={() => clearField(f)} aria-label={`${$i18nT('system.clear')} ${FIELD_LABEL[f]}`}><X size={12} /></button>
                {/if}
              </div>
            </div>
          {/each}
          <p class="ff-hint">
            {$i18nT('pages.tripleBrowser.ffHintPart1')} <b>AND</b>. {$i18nT('pages.tripleBrowser.ffHintPart2')} <b>{MODE_GLYPH.contains}/{MODE_GLYPH.exact}/{MODE_GLYPH.regex}</b> {$i18nT('pages.tripleBrowser.ffHintPart3')} {$i18nT('pages.tripleBrowser.ffHintNeg')}
          </p>
        </div>
      {/if}

      {#if uiMode === 'advanced' && sparqlPreviewOpen}
        <div class="sparql-preview">
          <!-- Natural-language → SPARQL via the configured LLM endpoint -->
          <div class="nl-row">
            <Sparkles size={13} class="nl-icon" />
            <input
              class="nl-input"
              placeholder={llmStatus && !llmStatus.reachable ? $i18nT('pages.tripleBrowser.llmOfflinePlaceholder') : $i18nT('pages.tripleBrowser.nlAskPlaceholder')}
              bind:value={nlQuestion}
              disabled={llmStatus && !llmStatus.reachable}
              on:keydown={(e) => { if (e.key === 'Enter') generateFromNl(); }}
            />
            <button class="btn btn-sm" on:click={generateFromNl} disabled={nlLoading || !nlQuestion.trim() || (llmStatus && !llmStatus.reachable)}>
              {nlLoading ? $i18nT('pages.tripleBrowser.generating') : $i18nT('pages.tripleBrowser.generate')}
            </button>
            {#if llmStatus}
              <span class="nl-status" class:offline={!llmStatus.reachable} title={llmStatus.reachable ? `${$i18nT('pages.tripleBrowser.llmOnline')} (${llmStatus.gateway})` : `${$i18nT('pages.tripleBrowser.llmOffline')} (${llmStatus.gateway})`}>{llmStatus.reachable ? '● LLM' : '○ LLM'}</span>
            {/if}
          </div>
          {#if nlError}<p class="nl-error">{nlError}</p>{/if}
          {#if llmSparql}
            <div class="sparql-preview-head">
              <span>{$i18nT('pages.tripleBrowser.generatedFromQuestion')}</span>
              <div class="sparql-preview-actions">
                <button class="btn btn-sm btn-ghost" on:click={() => copyToClipboard(llmSparql)}><Copy size={12}/> {$i18nT('system.copy')}</button>
                <button class="btn btn-sm" on:click={() => navigate(`/sparql?query=${encodeURIComponent(llmSparql)}`)}><ExternalLink size={12}/> {$i18nT('pages.tripleBrowser.openInEditor')}</button>
              </div>
            </div>
            <pre class="sparql-preview-code">{llmSparql}</pre>
          {/if}

          <div class="sparql-preview-head">
            <span>{$i18nT('pages.tripleBrowser.equivalentSparql')}</span>
            <div class="sparql-preview-actions">
              <button class="btn btn-sm btn-ghost" on:click={copySparql}><Copy size={12}/> {$i18nT('system.copy')}</button>
              <button class="btn btn-sm" on:click={openInSparqlEditor}><ExternalLink size={12}/> {$i18nT('pages.tripleBrowser.openInEditor')}</button>
            </div>
          </div>
          <pre class="sparql-preview-code">{sparqlPreview}</pre>
        </div>
      {/if}

      <!-- Facet rail (left) + the active view (right) -->
      <div class="browser-body" class:body-graph={viewMode === 'graph'} class:body-map={viewMode === 'map'}>
        <FacetRail
          facets={facets}
          loading={facetsLoading}
          chips={activeChips}
          bind:collapsed={railCollapsed}
          on:addchips={handleFacetAdd}
        />
        <div class="view-pane">

          <!-- Table view -->
          {#if viewMode === 'table'}
      {#if error}
        <div class="error-box">
          <div><strong>{$i18nT('pages.tripleBrowser.errorLoading')}</strong><br />{error}</div>
          <button class="btn btn-sm btn-ghost" on:click={fetchTriples}>{$i18nT('system.retry')}</button>
        </div>
      {/if}

      {#if $showSkeleton}
        <div class="skeleton-rows">
          {#each Array(10) as _}
            <div class="skeleton-row">
              <div class="skel skel-a"></div>
              <div class="skel skel-b"></div>
              <div class="skel skel-c"></div>
              <div class="skel skel-d"></div>
            </div>
          {/each}
        </div>
      {:else if loading && filteredTriples.length === 0}
        <!-- A fetch is in flight but hasn't crossed the skeleton delay yet and we
             have no prior rows to keep showing: hold a calm placeholder rather
             than flashing the "store empty" state. -->
        <div class="table-placeholder" aria-hidden="true"></div>
      {:else if filteredTriples.length === 0}
        <div class="empty-state">
          {#if tableSearch}
            <p>{$i18nT('pages.tripleBrowser.noMatchSearchPre')} "<strong>{tableSearch}</strong>" {$i18nT('pages.tripleBrowser.noMatchSearchPost')}</p>
            <p class="hint">{$i18nT('pages.tripleBrowser.searchSubstringHint')}</p>
            <button class="btn btn-sm" on:click={clearTableSearch}>{$i18nT('pages.tripleBrowser.clearSearch')}</button>
          {:else if hasFilters}
            <p>{$i18nT('pages.tripleBrowser.noMatchFilters')}</p>
            <button class="btn btn-sm" on:click={clearFilters}>{$i18nT('pages.tripleBrowser.clearFilters')}</button>
          {:else}
            <p>{$i18nT('pages.tripleBrowser.storeEmpty')} <a href="/import">{$i18nT('pages.tripleBrowser.importToStart')}</a> {$i18nT('pages.tripleBrowser.toGetStarted')}</p>
          {/if}
        </div>
      {:else}
        <DataTable
          mode="triples"
          triples={filteredTriples}
          loading={loading}
          emptyText={$i18nT('pages.tripleBrowser.noMatchFilters')}
        />

        <div class="pagination">
          {#if totalPages != null && totalPages > 1}
            <button class="btn btn-sm" on:click={firstPage} disabled={page === 0} title={$i18nT('pages.tripleBrowser.firstPage')}>«</button>
            <button class="btn btn-sm" on:click={prevPage} disabled={page === 0}><ChevronLeft size={14} /> {$i18nT('pages.tripleBrowser.prev')}</button>
            <span class="page-info">
              {$i18nT('pages.tripleBrowser.page')}
              <input class="page-jump-input" type="number" min="1" max={totalPages} value={page + 1}
                on:change={(e) => gotoPage(parseInt(e.currentTarget.value, 10))}
                on:keydown={(e) => e.key === 'Enter' && gotoPage(parseInt(e.currentTarget.value, 10))}
                aria-label={$i18nT('pages.tripleBrowser.jumpToPage')} />
              {$i18nT('pages.tripleBrowser.of')} {totalPages.toLocaleString()}
            </span>
            <button class="btn btn-sm" on:click={nextPage} disabled={page >= totalPages - 1}>{$i18nT('pages.tripleBrowser.next')} <ChevronRight size={14} /></button>
            <button class="btn btn-sm" on:click={lastPage} disabled={page >= totalPages - 1} title={$i18nT('pages.tripleBrowser.lastPage')}>»</button>
          {:else if hasMore || page > 0}
            <button class="btn btn-sm" on:click={prevPage} disabled={page === 0}><ChevronLeft size={14} /> {$i18nT('pages.tripleBrowser.prev')}</button>
            <span class="page-info">
              {$i18nT('pages.tripleBrowser.page')} {page + 1}
              {#if loadingCount}
                <span class="page-info-pending"> · {$i18nT('pages.tripleBrowser.countingInline')}</span>
              {:else}
                <button class="page-info-reveal" on:click={fetchExactCount} title={$i18nT('pages.tripleBrowser.computeTotalPageCount')}>{$i18nT('pages.tripleBrowser.showLastPage')}</button>
              {/if}
            </span>
            <button class="btn btn-sm" on:click={nextPage} disabled={!hasMore}>{$i18nT('pages.tripleBrowser.next')} <ChevronRight size={14} /></button>
          {/if}
          <div class="page-size-control">
            <label for="page-size-select" class="page-size-label">{$i18nT('pages.tripleBrowser.perPage')}</label>
            <Select id="page-size-select" size="sm"
              bind:value={pageSize}
              on:change={() => { page = 0; fetchTriples(); }}
              options={[
                { value: 25, label: '25' },
                { value: 50, label: '50' },
                { value: 100, label: '100' },
              ]}
            />
          </div>
        </div>
      {/if}

    <!-- ─── Graph view ──────────────────────────────────────────────────────── -->
    {:else if viewMode === 'graph'}

      <div class="graph-area">
        {#if graphNodes.length === 0 && !graphLoading && !graphLoadingMore}
          <div class="graph-empty">
            <Network size={52} strokeWidth={1} />
            <p class="graph-empty-title">{$i18nT('pages.tripleBrowser.noGraphData')}</p>
            <p class="graph-empty-sub">{$i18nT('pages.tripleBrowser.noGraphDataSub')}</p>
          </div>
        {:else}
          {#await graphCanvasMod then GC}
            {#if GC}
              <svelte:component this={GC.default}
                bind:this={graphCanvas}
                nodes={graphNodes}
                edges={graphEdges}
                layout={activeLayout}
                height="100%"
                loading={graphLoading && graphNodes.length === 0}
                loadingMore={graphLoadingMore}
                highlightIds={graphHighlightIds}
                expandedNodes={browseExpandedIris}
                expandingNode={browseExpandingUri}
                exhaustedNodes={browseExhaustedIris}
                on:nodeExpand={handleBrowseNodeExpand}
                on:nodeOpen={handleBrowseNodeOpen}
                on:edgeClick={handleBrowseEdgeClick}
                on:nodeContextMenu={handleBrowseNodeContextMenu}
                on:canvasContextMenu={handleBrowseCanvasContextMenu}
              />
            {/if}
          {/await}
        {/if}
        {#if browseGraphHint}
          <div class="graph-hint" role="status">{browseGraphHint}</div>
        {/if}
        {#if browseEdgePredicate}
          <div class="edge-card">
            <button class="edge-card-x" on:click={() => (browseEdgePredicate = null)} title={$i18nT('system.close')} aria-label={$i18nT('system.close')}>✕</button>
            <TermDefinitionCard iri={browseEdgePredicate} variant="rich" />
          </div>
        {/if}
      </div>

    <!-- ─── Map view ─────────────────────────────────────────────────────────── -->
    {:else if viewMode === 'map'}
      <div class="map-area">
        {#if mapElements.length > 0}
          {#await viewerMapMod then VM}
            {#if VM}
              <svelte:component this={VM.default} elements={mapElements} height="100%"
                on:select={(e) => e.detail.id && !e.detail.id.startsWith('row:') && !e.detail.id.startsWith('_:') && navigate(`/resource?iri=${encodeURIComponent(e.detail.id)}`)} />
            {/if}
          {/await}
        {:else if mapLoading}
          <div class="graph-empty">
            <MapIcon size={52} strokeWidth={1} />
            <p class="graph-empty-title">{$i18nT('pages.tripleBrowser.mapLoading')}</p>
          </div>
        {:else}
          <div class="graph-empty">
            <MapIcon size={52} strokeWidth={1} />
            <p class="graph-empty-title">{$i18nT('pages.tripleBrowser.noMappable')}</p>
            <p class="graph-empty-sub">{$i18nT('pages.tripleBrowser.noMappableSub')}</p>
          </div>
        {/if}
      </div>

    {/if}
        </div><!-- /view-pane -->
      </div><!-- /browser-body -->
  </div>
</div>

<!-- Browse graph context menu -->
<ContextMenu
  bind:visible={browseCtxVisible}
  bind:x={browseCtxX}
  bind:y={browseCtxY}
  items={browseCtxItems}
  on:action={handleBrowseCtxAction}
/>

<!-- Export modal -->
{#if exportModalOpen}
  <!-- svelte-ignore a11y-click-events-have-key-events -->
  <!-- svelte-ignore a11y-no-static-element-interactions -->
  <div class="modal-overlay" on:click={closeExport}>
    <div class="modal" on:click|stopPropagation role="dialog" aria-modal="true" tabindex="-1">
      <div class="modal-header">
        <h3 class="modal-title">{$i18nT('system.export')}</h3>
        <button class="modal-close" on:click={closeExport}><X size={16}/></button>
      </div>
      <div class="modal-body">

        <!-- Scope selector -->
        <p class="modal-section-label">{$i18nT('pages.tripleBrowser.scopeHeading')}</p>
        <div class="scope-row">
          <button class="scope-btn" class:scope-active={exportScope === 'page'}
            on:click={() => exportScope = 'page'}>
            {$i18nT('pages.tripleBrowser.currentPageCount', { values: { count: triples.length } })}
          </button>
          <button class="scope-btn" class:scope-active={exportScope === 'all'}
            on:click={() => exportScope = 'all'}>
            {hasFilters ? $i18nT('pages.tripleBrowser.allMatchingFiltered') : $i18nT('pages.tripleBrowser.allMatching')}
          </button>
        </div>
        {#if exportingAll}
          <p class="muted-tip" style="margin-top:.4rem">{$i18nT('pages.tripleBrowser.fetchingAllTriples')}</p>
        {/if}

        <!-- Data formats -->
        <p class="modal-section-label" style="margin-top:1rem">{$i18nT('pages.tripleBrowser.rdfTabularData')}</p>
        <div class="export-btn-grid">
          <button class="export-opt" on:click={() => doExport('csv')} disabled={exportingAll}>
            <FileText size={20} />
            <span class="eo-name">CSV</span>
            <span class="eo-desc">{$i18nT('pages.tripleBrowser.eoSpreadsheet')}</span>
          </button>
          <button class="export-opt" on:click={() => doExport('nt')} disabled={exportingAll}>
            <FileText size={20} />
            <span class="eo-name">N-Triples</span>
            <span class="eo-desc">.nt</span>
          </button>
          <button class="export-opt" on:click={() => doExport('nq')} disabled={exportingAll}>
            <FileText size={20} />
            <span class="eo-name">N-Quads</span>
            <span class="eo-desc">{$i18nT('pages.tripleBrowser.eoNqWithGraphs')}</span>
          </button>
          <button class="export-opt" on:click={() => doExport('ttl')} disabled={exportingAll}>
            <FileText size={20} />
            <span class="eo-name">Turtle</span>
            <span class="eo-desc">.ttl</span>
          </button>
          <button class="export-opt" on:click={() => doExport('trig')} disabled={exportingAll}>
            <FileText size={20} />
            <span class="eo-name">TriG</span>
            <span class="eo-desc">{$i18nT('pages.tripleBrowser.eoTrigNamedGraphs')}</span>
          </button>
        </div>

        <!-- Image formats — only in graph view -->
        {#if viewMode === 'graph' && graphNodes.length > 0}
          <p class="modal-section-label" style="margin-top:1rem">{$i18nT('pages.tripleBrowser.graphImage')}</p>
          <div class="export-btn-grid">
            <button class="export-opt" on:click={exportGraphPng}>
              <Image size={20} />
              <span class="eo-name">PNG 2×</span>
              <span class="eo-desc">{$i18nT('pages.tripleBrowser.eoStandardQuality')}</span>
            </button>
            <button class="export-opt" on:click={exportGraphPngHiRes}>
              <Image size={20} />
              <span class="eo-name">PNG 4×</span>
              <span class="eo-desc">{$i18nT('pages.tripleBrowser.eoPrintHiRes')}</span>
            </button>
            <button class="export-opt" on:click={exportGraphSvg}>
              <Image size={20} />
              <span class="eo-name">SVG</span>
              <span class="eo-desc">{$i18nT('pages.tripleBrowser.eoVectorWrapper')}</span>
            </button>
          </div>
        {/if}
      </div>
    </div>
  </div>
{/if}

<style>
  .browser { display: flex; flex-direction: column; gap: 1rem; }

  .breadcrumb {
    display: none;
  }

  /* ─── Card & header ──────────────────────────────────────────────────────── */
  .browser-card { padding: 0; overflow: hidden; display: flex; flex-direction: column; }

  .card-header {
    display: flex; align-items: center; gap: 0.5rem;
    padding: 0.55rem 0.75rem;
    border-bottom: 1px solid #e2e8f0;
    background: #fff;
    flex-shrink: 0;
  }
  .header-spacer { flex: 1; }

  /* ─── Simple / Advanced mode toggle ──────────────────────────────────────── */
  .mode-toggle { display: flex; border: 1px solid #e2e8f0; border-radius: 7px; overflow: hidden; }
  .mode-btn {
    display: flex; align-items: center; gap: 5px; height: 30px; padding: 0 10px;
    border: none; background: #f8fafc; cursor: pointer; color: #94a3b8;
    font-size: 0.78rem; font-weight: 500; transition: background 0.12s, color 0.12s;
  }
  .mode-btn + .mode-btn { border-left: 1px solid #e2e8f0; }
  .mode-btn:hover { background: #eef2ff; color: #4f46e5; }
  .mode-active { background: #4f46e5 !important; color: #fff !important; }

  /* ─── Chips area + facet rail body ───────────────────────────────────────── */
  /* ─── Multi-field filter form ────────────────────────────────────────────── */
  .filter-form {
    display: grid; grid-template-columns: repeat(auto-fit, minmax(280px, 1fr));
    gap: 0.4rem 0.9rem; align-items: center;
    padding: 0.6rem 0.75rem; background: #f8fafc; border-bottom: 1px solid #e2e8f0;
  }
  .ff-row { display: flex; align-items: center; gap: 0.4rem; }
  .ff-label {
    flex: 0 0 78px; font-size: 0.76rem; font-weight: 600; color: #475569; text-align: right;
  }
  .ff-mode {
    flex: 0 0 auto; min-width: 26px; height: 26px; padding: 0 6px;
    border-radius: 6px; cursor: pointer; font-weight: 700; font-size: 0.78rem;
    display: inline-flex; align-items: center; justify-content: center;
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  }
  /* contains = dashed/fuzzy · exact = solid indigo · regex = purple mono */
  .ff-mode-contains { background: #fff; border: 1px dashed #cbd5e1; color: #64748b; }
  .ff-mode-exact { background: #eef2ff; border: 1px solid #c7d2fe; color: #4f46e5; }
  .ff-mode-regex { background: #f5f3ff; border: 1px solid #ddd6fe; color: #7c3aed; }
  .ff-mode:hover { filter: brightness(0.97); }
  /* NOT toggle — off: ghost; on: red, marking the field as an exclusion filter. */
  .ff-neg {
    flex: 0 0 auto; height: 26px; padding: 0 7px; border-radius: 6px; cursor: pointer;
    font-weight: 700; font-size: 0.64rem; letter-spacing: 0.04em;
    display: inline-flex; align-items: center; justify-content: center;
    background: #fff; border: 1px dashed #cbd5e1; color: #94a3b8;
  }
  .ff-neg:hover { border-color: #fca5a5; color: #ef4444; }
  .ff-neg-on { background: #fef2f2; border: 1px solid #fca5a5; color: #dc2626; }
  .ff-input-wrap { position: relative; display: flex; align-items: center; flex: 1; min-width: 0; }
  .ff-input {
    width: 100%; box-sizing: border-box;
    padding: 0.3rem 1.6rem 0.3rem 0.5rem !important;
    border: 1px solid #cbd5e1; border-radius: 6px; font-size: 0.78rem; background: #fff; color: #333;
  }
  .ff-input:focus { outline: none; border-color: #6366f1; background: #fff; }
  .ff-clear {
    position: absolute; right: 5px; background: none; border: none; cursor: pointer;
    color: #94a3b8; display: inline-flex; align-items: center; padding: 1px;
  }
  .ff-clear:hover { color: #64748b; }
  .ff-hint { grid-column: 1 / -1; font-size: 0.72rem; color: #94a3b8; margin: 0.1rem 0 0; }

  .sparql-preview { background: #0f172a; border-bottom: 1px solid #1e293b; }
  .sparql-preview-head {
    display: flex; align-items: center; justify-content: space-between;
    padding: 0.4rem 0.75rem; color: #cbd5e1; font-size: 0.72rem; font-weight: 600;
  }
  .sparql-preview-actions { display: flex; gap: 0.4rem; }
  .sparql-preview-code {
    margin: 0; padding: 0 0.75rem 0.6rem; color: #e2e8f0; font-size: 0.74rem;
    line-height: 1.45; white-space: pre-wrap; word-break: break-all;
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  }

  /* ─── Natural-language → SPARQL row ──────────────────────────────────────── */
  .nl-row { display: flex; align-items: center; gap: 0.4rem; padding: 0.5rem 0.75rem 0.2rem; position: relative; }
  .nl-row :global(.nl-icon) { color: #a5b4fc; flex-shrink: 0; }
  .nl-input {
    flex: 1; min-width: 0; font-size: 0.78rem; padding: 0.3rem 0.5rem;
    border: 1px solid #334155; border-radius: 5px; background: #1e293b; color: #e2e8f0;
  }
  .nl-input::placeholder { color: #64748b; }
  .nl-input:focus { outline: none; border-color: #6366f1; }
  .nl-status { font-size: 0.7rem; font-weight: 700; color: #34d399; white-space: nowrap; }
  .nl-status.offline { color: #f87171; }
  .nl-error { color: #fca5a5; font-size: 0.74rem; padding: 0.1rem 0.75rem 0.3rem; margin: 0; }

  /* ─── Search-syntax help popover ─────────────────────────────────────────── */
  .syntax-help-wrap { position: relative; display: inline-flex; }
  .syntax-help-btn {
    display: inline-flex; align-items: center; justify-content: center;
    background: transparent; border: none; cursor: pointer; color: #94a3b8; padding: 2px;
  }
  .syntax-help-btn:hover { color: #2563eb; }
  .syntax-help {
    position: absolute; top: calc(100% + 6px); right: 0; z-index: 40; width: 320px;
    background: #fff; border: 1px solid #e2e8f0; border-radius: 10px;
    box-shadow: 0 10px 30px rgba(0,0,0,0.14); padding: 0.6rem 0.75rem;
  }
  .sh-title { font-size: 0.74rem; font-weight: 700; color: #334155; margin-bottom: 0.35rem; }
  .sh-list { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 0.25rem; }
  .sh-list li { font-size: 0.74rem; color: #475569; }
  .sh-list code { background: #f1f5f9; border-radius: 4px; padding: 0 4px; color: #0f172a; font-size: 0.72rem; }
  .sh-note { font-size: 0.68rem; color: #94a3b8; margin: 0.4rem 0 0.3rem; }
  .sh-link { font-size: 0.74rem; color: #2563eb; text-decoration: none; font-weight: 600; }
  .sh-link:hover { text-decoration: underline; }

  /* Table view: cap the body so a long facet list scrolls inside the rail (whose
     .rail-body is overflow-y:auto) instead of stretching the rail past the table. */
  .browser-body { display: flex; align-items: stretch; min-height: 0; max-height: 72vh; }
  /* Graph view: bound the row to the viewport so the facet rail and the graph
     both fill the available height — the rail then scrolls internally and the
     graph grows to take the rest of the space (rather than a fixed 62vh box).
     Reset the table-mode max-height so the graph can grow taller than 72vh. */
  .browser-body.body-graph { height: calc(100vh - 235px); min-height: 460px; max-height: none; }
  /* Map view: same viewport-bounded layout as the graph so the ViewerMap fills
     the remaining height (it sizes itself to 100% of .map-area). */
  .browser-body.body-map { height: calc(100vh - 235px); min-height: 460px; max-height: none; }
  .view-pane { flex: 1; min-width: 0; min-height: 0; display: flex; flex-direction: column; }

  /* ─── View toggle ────────────────────────────────────────────────────────── */
  .view-toggle {
    display: flex; border: 1px solid #e2e8f0; border-radius: 7px; overflow: hidden;
  }
  .vtoggle-btn {
    display: flex; align-items: center; justify-content: center; gap: 5px;
    height: 30px; padding: 0 10px;
    border: none; background: #f8fafc; cursor: pointer;
    color: #94a3b8; font-size: 0.78rem; font-weight: 500;
    transition: background 0.12s, color 0.12s;
  }
  .vtoggle-label { white-space: nowrap; }
  .vtoggle-btn + .vtoggle-btn { border-left: 1px solid #e2e8f0; }
  .vtoggle-btn:hover { background: #eff6ff; color: #2563eb; }
  .vtoggle-active { background: #3b82f6 !important; color: #fff !important; }

  /* ─── Count badge ────────────────────────────────────────────────────────── */
  .count-badge {
    font-size: 0.82rem; color: #475569;
    background: #f1f5f9; padding: 2px 10px;
    border-radius: 20px; font-weight: 600; white-space: nowrap;
  }
  .count-reveal-btn {
    background: transparent; border: none; padding: 0; cursor: pointer;
    color: #2563eb; font-weight: 600; font-size: inherit; text-decoration: underline;
  }
  .count-reveal-btn:disabled { color: #94a3b8; cursor: wait; text-decoration: none; }

  /* ─── Export button ──────────────────────────────────────────────────────── */
  .btn-export {
    display: flex; align-items: center; gap: 5px;
    background: #2563eb; color: #fff; border: none;
    padding: 5px 12px; border-radius: 6px; cursor: pointer;
    font-size: 0.8rem; font-weight: 600; transition: background 0.12s;
    box-shadow: none;
  }
  .btn-export:hover { background: #1d4ed8; }

  /* ─── Table search bar ───────────────────────────────────────────────────── */
  .table-search-bar {
    display: flex; align-items: center; gap: 0.5rem;
    padding: 0.5rem 0.75rem;
    background: #f8fafc;
    border-bottom: 1px solid #e2e8f0;
  }
  .table-search-input-wrap {
    position: relative; display: flex; align-items: center; flex: 1;
  }
  .table-search-input-wrap :global(.search-icon) {
    position: absolute; left: 10px; color: #94a3b8; pointer-events: none; flex-shrink: 0;
  }
  .table-search-input {
    width: 100%; box-sizing: border-box;
    /* override the global input reset (higher specificity) so the icon clears the text */
    padding: 0.4rem 0.6rem !important;
    padding-left: 2.1rem !important;
    padding-right: 2rem !important;
    border: 1px solid #cbd5e1; border-radius: 6px;
    font-size: 0.8rem; background: #fff; color: #333;
  }
  .table-search-input:focus { outline: none; border-color: #3b82f6; background: #eff6ff; }
  .search-clear {
    position: absolute; right: 6px;
    background: none; border: none; cursor: pointer; color: #94a3b8;
    display: flex; align-items: center; padding: 2px;
  }
  .search-clear:hover { color: #64748b; }

  /* ─── Dataset scope chip ─────────────────────────────────────────────────── */
  /* Per-dataset version control (shared header) */
  .version-ctl { position: relative; }
  .version-ctl .version-pinned {
    color: #92400e; background: #fef3c7; border-color: #fde68a;
  }
  .version-panel {
    position: absolute; right: 0; top: calc(100% + 4px);
    z-index: 30; min-width: 280px; max-width: 360px;
    background: #fff; border: 1px solid var(--line-soft, #e5e7eb);
    border-radius: 8px; box-shadow: 0 10px 30px rgba(0,0,0,0.12);
    padding: 0.5rem;
  }
  .vp-head {
    font-size: 0.72rem; font-weight: 700; text-transform: uppercase;
    letter-spacing: 0.05em; color: var(--ink-400, #94a3b8); padding: 0.25rem 0.35rem 0.4rem;
  }
  .vp-row { display: flex; align-items: center; gap: 0.5rem; padding: 0.2rem 0.35rem; }
  .vp-name {
    flex: 1; min-width: 0; font-size: 0.82rem; color: var(--ink-700, #334155);
    overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
  }
  .vp-select {
    flex: 0 0 auto; max-width: 55%; font-size: 0.78rem; padding: 0.2rem 0.4rem;
    border: 1px solid var(--line-soft, #d0d7de); border-radius: 4px; background: #f6f8fa; cursor: pointer;
  }
  .vp-select.vp-pinned { color: #92400e; background: #fef3c7; border-color: #fde68a; font-weight: 600; }

  .dataset-scope-bar {
    display: flex; align-items: center; flex-wrap: wrap; gap: 0.4rem;
    padding: 0.35rem 1rem;
    background: #eff6ff;
    border-bottom: 1px solid #bfdbfe;
    font-size: 0.78rem;
    color: #1d4ed8;
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

  /* GRAPHS sub-row inside the scope bar — wraps to its own line. */
  .scope-graph-row {
    flex-basis: 100%;
    display: flex; align-items: center; flex-wrap: wrap; gap: 0.4rem;
    padding-top: 0.3rem; margin-top: 0.2rem;
    border-top: 1px dashed #bfdbfe;
  }
  .graph-scope-chip { background: #e0e7ff; color: #3730a3; }
  .graph-scope-chip .dataset-scope-x { color: #6366f1; }
  .graph-scope-chip .dataset-scope-x:hover { color: #3730a3; }
  .graph-scope-item { justify-content: flex-start; gap: 0.4rem; }
  .gsi-check { width: 0.9em; flex: 0 0 auto; color: #4f46e5; font-weight: 700; text-align: center; }
  .gsi-name { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .gsi-count {
    flex: 0 0 auto; font-size: 0.68rem; color: #94a3b8;
    background: #f1f5f9; border-radius: 8px; padding: 0 6px; font-variant-numeric: tabular-nums;
  }
  .graph-scope-selected { background: #eef2ff; }

  /* Scope picker */
  .scope-picker-wrap { position: relative; }
  .scope-add-btn {
    display: inline-flex; align-items: center; gap: 0.25rem;
    font-size: 0.75rem; color: #2563eb;
    background: transparent;
    border: 1px dashed #93c5fd;
    border-radius: 6px; padding: 0.15rem 0.5rem;
    cursor: pointer; line-height: 1.4;
  }
  .scope-add-btn:hover { background: #dbeafe; }
  .scope-picker {
    position: absolute; top: calc(100% + 5px); left: 0; z-index: 30;
    background: white;
    border: 1px solid #e2e8f0;
    border-radius: 10px;
    box-shadow: 0 8px 28px rgba(0,0,0,0.12);
    min-width: 220px; max-height: 280px;
    overflow-y: auto; padding: 0.35rem;
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
  .scope-add-row {
    padding: 0.3rem 1rem;
    border-bottom: 1px solid #f1f5f9;
  }
  .scope-add-btn-outline {
    display: inline-flex; align-items: center; gap: 0.3rem;
    font-size: 0.75rem; color: #64748b;
    background: transparent; border: 1px dashed #cbd5e1;
    border-radius: 6px; padding: 0.2rem 0.6rem;
    cursor: pointer;
  }
  .scope-add-btn-outline:hover { border-color: #93c5fd; color: #2563eb; }

  .btn-adv-toggle {
    display: flex; align-items: center; gap: 5px;
    white-space: nowrap; padding: 5px 10px;
    border: 1px solid #cbd5e1; border-radius: 5px;
    background: #fff; color: #475569; cursor: pointer;
    font-size: 0.78rem; font-weight: 500;
    transition: all 0.12s;
  }
  .btn-adv-toggle:hover { background: #eff6ff; border-color: #93c5fd; color: #2563eb; }
  .btn-adv-active { border-color: #93c5fd; color: #2563eb; background: #eff6ff; }
  .adv-badge {
    display: inline-flex; align-items: center; justify-content: center;
    min-width: 16px; height: 16px; padding: 0 3px;
    background: #2563eb; color: #fff;
    border-radius: 8px; font-size: 0.65rem; font-weight: 700;
    line-height: 1;
  }

  /* ─── Advanced filters panel ─────────────────────────────────────────────── */
  .advanced-filters {
    display: flex; flex-direction: column; gap: 0.5rem;
    padding: 0.75rem 1rem;
    background: #f8fafc;
    border-bottom: 1px solid #e2e8f0;
  }
  .af-row { display: flex; align-items: center; gap: 0.5rem; }
  .af-label {
    width: 80px; font-size: 0.72rem; font-weight: 700;
    text-transform: uppercase; color: #94a3b8; letter-spacing: 0.4px;
    flex-shrink: 0;
  }
  .af-input {
    flex: 1; padding: 0.3rem 0.5rem;
    border: 1px solid #cbd5e1; border-radius: 4px;
    font-size: 0.8rem; background: #fff; color: #333;
  }
  .af-input:focus { outline: none; border-color: #3b82f6; }
  .af-input-contains { border-color: #93c5fd; background: #f0f7ff; }
  .af-input-contains:focus { border-color: #2563eb; background: #fff; }
  .af-actions { display: flex; gap: 0.5rem; margin-top: 0.25rem; }
  .af-mode-row {
    display: flex; align-items: center; gap: 0.6rem;
    flex-wrap: wrap;
  }
  .af-mode-toggle {
    display: inline-flex;
    border: 1px solid #cbd5e1;
    border-radius: 5px;
    overflow: hidden;
  }
  .af-mode-btn {
    padding: 0.25rem 0.7rem;
    background: #fff;
    border: none;
    font-size: 0.78rem;
    color: #475569;
    cursor: pointer;
  }
  .af-mode-btn:hover { background: #f1f5f9; }
  .af-mode-active { background: #2563eb; color: #fff; }
  .af-mode-active:hover { background: #1d4ed8; }
  .af-mode-hint { font-size: 0.72rem; color: #64748b; }
  .af-mode-hint-contains { color: #2563eb; font-weight: 500; }
  .af-iri-error {
    display: flex; align-items: baseline; flex-wrap: wrap; gap: 0.35rem;
    padding: 0.45rem 0.65rem;
    background: #fff7ed; border: 1px solid #fed7aa; border-radius: 5px;
    font-size: 0.76rem; color: #92400e; line-height: 1.4;
  }
  .af-iri-switch {
    margin-left: 0.1rem;
    background: none; border: 1px solid #f97316; border-radius: 4px;
    color: #c2410c; font-size: 0.74rem; padding: 0.15rem 0.5rem;
    cursor: pointer; white-space: nowrap;
  }
  .af-iri-switch:hover { background: #fff7ed; }

  /* Table rendering now lives in the shared DataTable component. */

  /* ─── Pagination ─────────────────────────────────────────────────────────── */
  .pagination {
    display: flex; align-items: center; justify-content: center; gap: 0.75rem;
    padding: 0.6rem 0.75rem; border-top: 1px solid #f0f0f0; flex-wrap: wrap;
  }
  .page-info { font-size: 0.85rem; color: #666; display: inline-flex; align-items: center; gap: 0.35rem; }
  .page-jump-input {
    width: 4.5em;
    padding: 2px 4px;
    font-size: 0.8rem;
    border: 1px solid #cbd5e1;
    border-radius: 4px;
    text-align: center;
    -moz-appearance: textfield;
    appearance: textfield;
  }
  .page-jump-input::-webkit-outer-spin-button,
  .page-jump-input::-webkit-inner-spin-button { -webkit-appearance: none; margin: 0; }
  .page-info-pending { color: #94a3b8; font-style: italic; font-size: 0.78rem; }
  .page-info-reveal {
    background: none; border: none; color: #2563eb; cursor: pointer;
    font-size: 0.78rem; padding: 0; text-decoration: underline;
  }
  .page-info-reveal:hover { color: #1d4ed8; }
  .page-size-control { display: flex; align-items: center; gap: 0.35rem; margin-left: auto; }
  .page-size-label { font-size: 0.78rem; color: #64748b; }
  .page-size-select {
    font-size: 0.78rem; padding: 2px 6px;
    border: 1px solid #cbd5e1; border-radius: 4px; background: #fff; color: #333; cursor: pointer;
  }

  .graph-area { height: 62vh; min-height: 360px; position: relative; overflow: hidden; }
  /* Inside the viewport-bounded body the graph fills the remaining height. */
  .body-graph .graph-area { height: auto; flex: 1 1 auto; min-height: 0; }
  /* Map view shares the graph's framing: a fixed box by default, growing to
     fill the viewport-bounded body when the Map tab is active. */
  .map-area { height: 62vh; min-height: 360px; position: relative; overflow: hidden; display: flex; }
  .body-map .map-area { height: auto; flex: 1 1 auto; min-height: 0; }
  .graph-hint {
    position: absolute; left: 50%; bottom: 16px; transform: translateX(-50%);
    background: rgba(15, 23, 42, 0.92); color: #f1f5f9; font-size: 0.78rem;
    padding: 0.4rem 0.8rem; border-radius: 8px; pointer-events: none; z-index: 5;
    box-shadow: 0 4px 14px rgba(0, 0, 0, 0.25);
  }
  /* Edge-click predicate detail — anchored top-right of the graph, scrollable. */
  .edge-card {
    position: absolute; top: 12px; right: 12px; width: 340px; max-width: calc(100% - 24px);
    max-height: calc(100% - 24px); overflow: auto; z-index: 6;
    background: #fff; border: 1px solid #e2e8f0; border-radius: 10px;
    box-shadow: 0 8px 30px rgba(15, 23, 42, 0.18); padding: 0.7rem 0.8rem;
  }
  .edge-card-x {
    position: absolute; top: 6px; right: 8px; background: none; border: none;
    color: #94a3b8; cursor: pointer; font-size: 0.9rem; line-height: 1; padding: 2px;
  }
  .edge-card-x:hover { color: #475569; }
  :global(html.dark) .edge-card { background: #0f172a; border-color: #1e293b; box-shadow: 0 8px 30px rgba(0, 0, 0, 0.5); }
  .graph-loading, .graph-empty {
    position: absolute; inset: 0;
    display: flex; flex-direction: column; align-items: center; justify-content: center;
    gap: 0.75rem; color: #94a3b8;
  }
  .graph-spinner {
    width: 32px; height: 32px; border: 3px solid #e2e8f0; border-top-color: #3b82f6;
    border-radius: 50%; animation: spin 0.7s linear infinite;
  }
  @keyframes spin { to { transform: rotate(360deg); } }
  .graph-empty-title { font-size: 1rem; font-weight: 600; color: #475569; margin: 0; }
  .graph-empty-sub { font-size: 0.82rem; color: #94a3b8; margin: 0; text-align: center; max-width: 280px; }


  /* ─── Error, empty, skeleton ─────────────────────────────────────────────── */
  .error-box {
    display: flex; align-items: center; justify-content: space-between; gap: 1rem;
    background: #fef0f0; border: 1px solid #f5c6cb; border-left: 3px solid #d94a4a;
    border-radius: 6px; padding: 0.75rem 1rem;
    font-size: 0.875rem; color: #721c24;
  }
  .empty-state { padding: 3rem; text-align: center; color: #888; }
  .empty-state .hint { font-size: 0.85rem; color: #aaa; margin: 0.25rem 0 0.75rem; }
  .empty-state a { color: #4a90d9; }
  .muted-tip { font-size: 0.8rem; color: #94a3b8; margin: 0.25rem 0; }

  /* Calm holder shown during a sub-delay first load (no skeleton, no flash). */
  .table-placeholder { min-height: 220px; }
  .skeleton-rows { padding: 0.5rem 0; }
  .skeleton-row { display: flex; gap: 1rem; padding: 0.55rem 0.75rem; border-bottom: 1px solid #f0f0f0; align-items: center; }
  .skel { height: 12px; border-radius: 6px; background: linear-gradient(90deg, #e8e8e8 25%, #f5f5f5 50%, #e8e8e8 75%); background-size: 200% 100%; animation: shimmer 1.4s infinite; }
  @keyframes shimmer { 0% { background-position: 200% 0; } 100% { background-position: -200% 0; } }
  .skel-a { flex: 2.5; } .skel-b { flex: 1.5; } .skel-c { flex: 3; } .skel-d { flex: 1; }

  /* ─── Buttons ────────────────────────────────────────────────────────────── */

  /* ─── Export modal ───────────────────────────────────────────────────────── */
  .modal-overlay {
    position: fixed; inset: 0; background: rgba(0,0,0,0.4);
    display: flex; align-items: center; justify-content: center;
    z-index: 9999;
  }
  .modal {
    background: #fff; border-radius: 12px; width: 420px; max-width: 95vw;
    box-shadow: 0 20px 60px rgba(0,0,0,0.2);
  }
  .modal-header {
    display: flex; align-items: center; justify-content: space-between;
    padding: 1rem 1.25rem; border-bottom: 1px solid #e2e8f0;
  }
  .modal-title { margin: 0; font-size: 1.05rem; font-weight: 700; color: #1e293b; }
  .modal-close {
    background: none; border: none; cursor: pointer; color: #94a3b8;
    display: flex; align-items: center; border-radius: 4px; padding: 2px;
  }
  .modal-close:hover { color: #475569; background: #f1f5f9; }
  .modal-body { padding: 1rem 1.25rem 1.5rem; }
  .modal-section-label {
    font-size: 0.72rem; font-weight: 700; text-transform: uppercase;
    color: #94a3b8; letter-spacing: 0.5px; margin: 0 0 0.5rem;
  }
  .export-btn-grid { display: flex; gap: 0.75rem; }
  .export-opt {
    flex: 1; display: flex; flex-direction: column; align-items: center;
    gap: 0.35rem; padding: 1rem 0.5rem;
    border: 1.5px solid #e2e8f0; border-radius: 10px; background: #f8fafc;
    cursor: pointer; color: #475569; transition: all 0.15s;
  }
  .export-opt:hover { border-color: #3b82f6; background: #eff6ff; color: #2563eb; }
  .eo-name { font-size: 0.9rem; font-weight: 700; }
  .eo-desc { font-size: 0.72rem; color: #94a3b8; }
  .export-opt:hover .eo-desc { color: #60a5fa; }
  .export-opt:disabled { opacity: 0.5; cursor: not-allowed; }

  /* Scope row */
  .scope-row { display: flex; gap: 0.5rem; }
  .scope-btn {
    flex: 1; padding: 0.45rem 0.75rem;
    border: 1.5px solid #e2e8f0; border-radius: 7px; background: #f8fafc;
    cursor: pointer; font-size: 0.8rem; color: #475569;
    transition: all 0.12s; text-align: center;
  }
  .scope-btn:hover { border-color: #93c5fd; background: #eff6ff; color: #2563eb; }
  .scope-active { border-color: #3b82f6 !important; background: #eff6ff !important; color: #2563eb !important; font-weight: 600; }

  /* ─── Dark theme overrides ───────────────────────────────────────────────── */
  /* Page-scoped styles above hardcode light colours; Svelte's scoping class
     out-specifies the global theme tokens, so we re-map them here. The
     `:global(:is(...))` prefix lifts specificity back above the scoped rules
     while keeping the descendant selectors scoped to this component. */
  :global(:is([data-theme="dark"], .dark)) .card-header { background: var(--bg-strong); border-bottom-color: var(--line-soft); }
  :global(:is([data-theme="dark"], .dark)) .mode-toggle,
  :global(:is([data-theme="dark"], .dark)) .view-toggle { border-color: var(--line-strong); }
  :global(:is([data-theme="dark"], .dark)) .mode-btn,
  :global(:is([data-theme="dark"], .dark)) .vtoggle-btn { background: rgba(255,255,255,0.03); color: var(--ink-600); }
  :global(:is([data-theme="dark"], .dark)) .mode-btn + .mode-btn,
  :global(:is([data-theme="dark"], .dark)) .vtoggle-btn + .vtoggle-btn { border-left-color: var(--line-soft); }
  :global(:is([data-theme="dark"], .dark)) .mode-btn:hover { background: rgba(99,102,241,0.15); color: #a5b4fc; }
  :global(:is([data-theme="dark"], .dark)) .vtoggle-btn:hover { background: rgba(59,130,246,0.15); color: #60a5fa; }

  :global(:is([data-theme="dark"], .dark)) .filter-form,
  :global(:is([data-theme="dark"], .dark)) .table-search-bar,
  :global(:is([data-theme="dark"], .dark)) .advanced-filters { background: rgba(255,255,255,0.02); border-bottom-color: var(--line-soft); }
  :global(:is([data-theme="dark"], .dark)) .ff-label { color: var(--ink-700); }
  :global(:is([data-theme="dark"], .dark)) .ff-mode-contains { background: var(--bg-strong); border-color: var(--line-strong); color: var(--ink-600); }
  :global(:is([data-theme="dark"], .dark)) .ff-mode-exact { background: rgba(99,102,241,0.18); border-color: rgba(99,102,241,0.45); color: #a5b4fc; }
  :global(:is([data-theme="dark"], .dark)) .ff-mode-regex { background: rgba(124,58,237,0.18); border-color: rgba(124,58,237,0.45); color: #c4b5fd; }
  :global(:is([data-theme="dark"], .dark)) .ff-neg { background: var(--bg-strong); border-color: var(--line-strong); color: var(--ink-600); }
  :global(:is([data-theme="dark"], .dark)) .ff-neg-on { background: rgba(220,38,38,0.20); border-color: rgba(248,113,113,0.5); color: #fca5a5; }
  :global(:is([data-theme="dark"], .dark)) .ff-input,
  :global(:is([data-theme="dark"], .dark)) .table-search-input { background: var(--bg-strong); border-color: var(--line-strong); color: var(--ink-900); }
  :global(:is([data-theme="dark"], .dark)) .ff-input:focus { background: var(--bg-strong); border-color: var(--brand-500); }
  :global(:is([data-theme="dark"], .dark)) .table-search-input:focus { background: var(--bg-soft); border-color: var(--brand-500); }
  :global(:is([data-theme="dark"], .dark)) .ff-clear,
  :global(:is([data-theme="dark"], .dark)) .search-clear,
  :global(:is([data-theme="dark"], .dark)) .table-search-input-wrap :global(.search-icon),
  :global(:is([data-theme="dark"], .dark)) .ff-hint { color: var(--ink-600); }
  :global(:is([data-theme="dark"], .dark)) .ff-clear:hover,
  :global(:is([data-theme="dark"], .dark)) .search-clear:hover { color: var(--ink-500); }

  :global(:is([data-theme="dark"], .dark)) .syntax-help-btn { color: var(--ink-600); }
  :global(:is([data-theme="dark"], .dark)) .syntax-help-btn:hover { color: #60a5fa; }
  :global(:is([data-theme="dark"], .dark)) .syntax-help { background: var(--bg-strong); border-color: var(--line-strong); }
  :global(:is([data-theme="dark"], .dark)) .sh-title { color: var(--ink-800); }
  :global(:is([data-theme="dark"], .dark)) .sh-list li { color: var(--ink-700); }
  :global(:is([data-theme="dark"], .dark)) .sh-list code { background: rgba(255,255,255,0.06); color: var(--ink-800); }
  :global(:is([data-theme="dark"], .dark)) .sh-note { color: var(--ink-600); }
  :global(:is([data-theme="dark"], .dark)) .sh-link { color: #60a5fa; }

  :global(:is([data-theme="dark"], .dark)) .count-badge { background: rgba(255,255,255,0.06); color: var(--ink-700); }
  :global(:is([data-theme="dark"], .dark)) .count-reveal-btn,
  :global(:is([data-theme="dark"], .dark)) .page-info-reveal { color: #60a5fa; }
  :global(:is([data-theme="dark"], .dark)) .count-reveal-btn:disabled { color: var(--ink-600); }
  :global(:is([data-theme="dark"], .dark)) .page-info-reveal:hover { color: #93c5fd; }

  :global(:is([data-theme="dark"], .dark)) .version-ctl .version-pinned,
  :global(:is([data-theme="dark"], .dark)) .vp-select.vp-pinned { color: #fcd34d; background: rgba(245,158,11,0.18); border-color: rgba(245,158,11,0.45); }
  :global(:is([data-theme="dark"], .dark)) .version-panel,
  :global(:is([data-theme="dark"], .dark)) .scope-picker { background: var(--bg-strong); border-color: var(--line-strong); }

  :global(:is([data-theme="dark"], .dark)) .dataset-scope-bar { background: rgba(59,130,246,0.12); border-bottom-color: rgba(59,130,246,0.3); color: #93c5fd; }
  :global(:is([data-theme="dark"], .dark)) .dataset-scope-chip { background: rgba(59,130,246,0.2); color: #bfdbfe; }
  :global(:is([data-theme="dark"], .dark)) .dataset-scope-x { color: #60a5fa; }
  :global(:is([data-theme="dark"], .dark)) .dataset-scope-x:hover { color: #93c5fd; }
  :global(:is([data-theme="dark"], .dark)) .scope-graph-row { border-top-color: rgba(59,130,246,0.3); }
  :global(:is([data-theme="dark"], .dark)) .graph-scope-chip { background: rgba(99,102,241,0.22); color: #c7d2fe; }
  :global(:is([data-theme="dark"], .dark)) .graph-scope-chip .dataset-scope-x { color: #a5b4fc; }
  :global(:is([data-theme="dark"], .dark)) .gsi-check { color: #a5b4fc; }
  :global(:is([data-theme="dark"], .dark)) .gsi-count { background: rgba(255,255,255,0.06); color: var(--ink-600); }
  :global(:is([data-theme="dark"], .dark)) .graph-scope-selected { background: rgba(99,102,241,0.16); }
  :global(:is([data-theme="dark"], .dark)) .scope-add-btn { color: #60a5fa; border-color: rgba(59,130,246,0.45); }
  :global(:is([data-theme="dark"], .dark)) .scope-add-btn:hover { background: rgba(59,130,246,0.18); }
  :global(:is([data-theme="dark"], .dark)) .scope-group-label,
  :global(:is([data-theme="dark"], .dark)) .scope-empty { color: var(--ink-600); }
  :global(:is([data-theme="dark"], .dark)) .scope-item { color: var(--ink-800); }
  :global(:is([data-theme="dark"], .dark)) .scope-item:hover { background: rgba(255,255,255,0.06); }
  :global(:is([data-theme="dark"], .dark)) .scope-add-row { border-bottom-color: var(--line-soft); }
  :global(:is([data-theme="dark"], .dark)) .scope-add-btn-outline { color: var(--ink-600); border-color: var(--line-strong); }
  :global(:is([data-theme="dark"], .dark)) .scope-add-btn-outline:hover { border-color: var(--brand-500); color: #60a5fa; }

  :global(:is([data-theme="dark"], .dark)) .btn-adv-toggle { background: var(--bg-strong); border-color: var(--line-strong); color: var(--ink-700); }
  :global(:is([data-theme="dark"], .dark)) .btn-adv-toggle:hover,
  :global(:is([data-theme="dark"], .dark)) .btn-adv-active { background: rgba(59,130,246,0.12); border-color: var(--brand-500); color: #60a5fa; }

  :global(:is([data-theme="dark"], .dark)) .af-input { background: var(--bg-strong); border-color: var(--line-strong); color: var(--ink-900); }
  :global(:is([data-theme="dark"], .dark)) .af-input-contains { background: rgba(59,130,246,0.1); border-color: rgba(59,130,246,0.45); }
  :global(:is([data-theme="dark"], .dark)) .af-input-contains:focus { background: var(--bg-strong); }
  :global(:is([data-theme="dark"], .dark)) .af-mode-toggle { border-color: var(--line-strong); }
  :global(:is([data-theme="dark"], .dark)) .af-mode-btn { background: var(--bg-strong); color: var(--ink-700); }
  :global(:is([data-theme="dark"], .dark)) .af-mode-btn:hover { background: rgba(255,255,255,0.06); }
  :global(:is([data-theme="dark"], .dark)) .af-mode-hint { color: var(--ink-600); }
  :global(:is([data-theme="dark"], .dark)) .af-mode-hint-contains { color: #60a5fa; }
  :global(:is([data-theme="dark"], .dark)) .af-iri-error { background: rgba(245,158,11,0.12); border-color: rgba(245,158,11,0.4); color: #fcd34d; }
  :global(:is([data-theme="dark"], .dark)) .af-iri-switch { border-color: rgba(249,115,22,0.5); color: #fdba74; }
  :global(:is([data-theme="dark"], .dark)) .af-iri-switch:hover { background: rgba(249,115,22,0.12); }

  /* Dark table overrides moved to DataTable. */

  :global(:is([data-theme="dark"], .dark)) .pagination { border-top-color: var(--line-soft); }
  :global(:is([data-theme="dark"], .dark)) .page-info { color: var(--ink-600); }
  :global(:is([data-theme="dark"], .dark)) .page-size-label { color: var(--ink-600); }

  :global(:is([data-theme="dark"], .dark)) .graph-spinner { border-color: var(--line-strong); border-top-color: var(--brand-500); }
  :global(:is([data-theme="dark"], .dark)) .graph-empty-title { color: var(--ink-700); }

  :global(:is([data-theme="dark"], .dark)) .error-box { background: rgba(220,38,38,0.14); border-color: rgba(220,38,38,0.4); border-left-color: #f87171; color: #fecaca; }
  :global(:is([data-theme="dark"], .dark)) .empty-state { color: var(--ink-600); }
  :global(:is([data-theme="dark"], .dark)) .empty-state .hint { color: var(--ink-500); }
  :global(:is([data-theme="dark"], .dark)) .empty-state a,
  :global(:is([data-theme="dark"], .dark)) .skeleton-row { border-bottom-color: var(--line-soft); }
  :global(:is([data-theme="dark"], .dark)) .empty-state a { color: #60a5fa; }
  :global(:is([data-theme="dark"], .dark)) .skel { background: linear-gradient(90deg, rgba(255,255,255,0.04) 25%, rgba(255,255,255,0.08) 50%, rgba(255,255,255,0.04) 75%); background-size: 200% 100%; }

  :global(:is([data-theme="dark"], .dark)) .modal { background: var(--bg-strong); }
  :global(:is([data-theme="dark"], .dark)) .modal-header { border-bottom-color: var(--line-strong); }
  :global(:is([data-theme="dark"], .dark)) .modal-title { color: var(--ink-900); }
  :global(:is([data-theme="dark"], .dark)) .modal-close:hover { color: var(--ink-700); background: rgba(255,255,255,0.06); }
  :global(:is([data-theme="dark"], .dark)) .export-opt,
  :global(:is([data-theme="dark"], .dark)) .scope-btn { background: rgba(255,255,255,0.03); border-color: var(--line-strong); color: var(--ink-700); }
  :global(:is([data-theme="dark"], .dark)) .export-opt:hover,
  :global(:is([data-theme="dark"], .dark)) .scope-btn:hover { background: rgba(59,130,246,0.12); border-color: var(--brand-500); color: #60a5fa; }
</style>
