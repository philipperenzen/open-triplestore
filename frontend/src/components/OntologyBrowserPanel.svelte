<script lang="ts">
  import { onMount } from 'svelte';
  import { t } from 'svelte-i18n';
  import { parseTurtle } from '../lib/ontology/loader.js';
  import OntologyModelViewer from './OntologyModelViewer.svelte';
  import GraphCanvas from './GraphCanvas.svelte';
  import ContextMenu from './ContextMenu.svelte';
  import { sparqlQuery } from '../lib/api.js';
  import { shortenIRI, graphResultsToElements } from '../lib/rdf-utils.js';
  import RdfTerm from './RdfTerm.svelte';
  import { navigate } from '../lib/router/index.js';
  import {
    Loader2, Network, Table2, BookOpen, Shapes, ChevronLeft, ChevronRight,
    Search, X, Copy, Unlink, Plus, Maximize2, Share2,
  } from 'lucide-svelte';
  import { copyToClipboard } from '../lib/clipboard.js';

  export let graphIri: string = '';
  export let subGraphs: string[] = [];
  export const title: string = '';
  export let versionLabel: string = '';
  export let rawDataUrl: string | null = null;

  type Mode = 'ontology' | 'graph' | 'tabular' | 'schema';
  let activeMode: Mode = 'ontology';

  // ── Loaded local store (rawDataUrl path) ────────────────────────────────────
  let loadedStore: any = null;
  let storeLoaded = false;
  let storeError = '';
  let storePromise: Promise<void> | null = null;

  function ensureStoreLoaded(): Promise<void> {
    if (storeLoaded) return Promise.resolve();
    if (!rawDataUrl) return Promise.resolve();
    if (storePromise) return storePromise;
    storeError = '';
    storePromise = (async () => {
      try {
        const resp = await fetch(rawDataUrl, { credentials: 'include' });
        if (!resp.ok) throw new Error(`HTTP ${resp.status}: ${resp.statusText}`);
        const turtle = await resp.text();
        const res = await parseTurtle(turtle);
        loadedStore = res.store;
        storeLoaded = true;
      } catch (e: any) {
        storeError = e?.message || $t('components.ontologyBrowserPanel.failedToLoadData');
      }
    })();
    return storePromise;
  }

  function n3TermToBinding(term: any): any {
    if (!term) return { type: 'uri', value: '' };
    if (term.termType === 'BlankNode') return { type: 'bnode', value: term.value };
    if (term.termType === 'Literal') {
      const b: any = { type: 'literal', value: term.value };
      if (term.language) b['xml:lang'] = term.language;
      if (term.datatype?.value) b.datatype = term.datatype.value;
      return b;
    }
    return { type: 'uri', value: term.value };
  }

  $: scopeFromClause = [graphIri, ...(subGraphs || [])]
    .filter(Boolean)
    .map((g) => `FROM <${g}>`)
    .join('\n');

  async function switchMode(mode: Mode) {
    activeMode = mode;
    if (mode === 'tabular' && triples.length === 0) await loadTriples(0);
    if (mode === 'schema'  && classCounts.length === 0 && !classLoading) await fetchClassDistribution();
    if (mode === 'graph'   && graphNodes.length === 0 && !graphLoading)  await fetchGraphData();
  }

  onMount(() => {
    if (rawDataUrl) ensureStoreLoaded();
  });

  function openResource(iri: string) {
    const qs = new URLSearchParams({ iri });
    if (graphIri) qs.set('graph', graphIri);
    navigate(`/resource?${qs.toString()}`);
  }

  // ── Tabular mode ────────────────────────────────────────────────────────────
  let triples: any[] = [];
  let tripleLoading = false;
  let tripleOffset = 0;
  let tripleHasMore = false;
  let tripleFilter = '';
  const PAGE = 200;

  async function loadTriples(offset = 0) {
    tripleLoading = true;
    tripleOffset = offset;
    try {
      if (rawDataUrl) {
        await ensureStoreLoaded();
        if (loadedStore) {
          const needle = tripleFilter.toLowerCase();
          const allQuads = loadedStore.getQuads(null, null, null, null);
          const filtered = needle
            ? allQuads.filter((q: any) =>
                q.subject.value.toLowerCase().includes(needle) ||
                q.predicate.value.toLowerCase().includes(needle) ||
                q.object.value.toLowerCase().includes(needle)
              )
            : allQuads;
          tripleHasMore = filtered.length > offset + PAGE;
          triples = filtered.slice(offset, offset + PAGE).map((q: any) => ({
            s: n3TermToBinding(q.subject),
            p: n3TermToBinding(q.predicate),
            o: n3TermToBinding(q.object),
          }));
        }
      } else {
        const escaped = tripleFilter.replace(/\\/g, '\\\\').replace(/"/g, '\\"');
        const filter = escaped
          ? `FILTER(CONTAINS(STR(?s), "${escaped}") || CONTAINS(STR(?p), "${escaped}") || CONTAINS(STR(?o), "${escaped}"))`
          : '';
        const res = await sparqlQuery(
          `SELECT ?s ?p ?o\n${scopeFromClause}\nWHERE { ?s ?p ?o . ${filter} } ORDER BY ?s ?p LIMIT ${PAGE + 1} OFFSET ${offset}`
        );
        const rows: any[] = res?.results?.bindings || [];
        tripleHasMore = rows.length > PAGE;
        triples = rows.slice(0, PAGE);
      }
    } catch {
      triples = [];
    }
    tripleLoading = false;
  }

  function shortIri(iri: string) { try { return shortenIRI(iri); } catch { return iri; } }

  // ── Schema: classes & properties distribution ───────────────────────────────
  const RDF_TYPE = 'http://www.w3.org/1999/02/22-rdf-syntax-ns#type';
  let classCounts: { cls: string; count: number }[] = [];
  let propCounts:  { prop: string; count: number }[] = [];
  let classLoading = false;
  let classesTab: 'classes' | 'properties' = 'classes';
  let classesSearch = '';

  function classesMatches(iri: string, q: string) {
    if (!q) return true;
    const needle = q.toLowerCase();
    if (iri.toLowerCase().includes(needle)) return true;
    try {
      const short = shortenIRI(iri);
      return !!short && short.toLowerCase().includes(needle);
    } catch { return false; }
  }
  $: filteredClassCounts = classCounts.filter(c => classesMatches(c.cls,  classesSearch));
  $: filteredPropCounts  = propCounts .filter(c => classesMatches(c.prop, classesSearch));

  async function fetchClassDistribution() {
    classLoading = true;
    classCounts = [];
    propCounts = [];
    try {
      if (rawDataUrl) {
        await ensureStoreLoaded();
        if (loadedStore) {
          const classMap = new Map<string, Set<string>>();
          const propMap = new Map<string, number>();
          const quads = loadedStore.getQuads(null, null, null, null);
          for (const q of quads) {
            const p = q.predicate.value;
            if (p === RDF_TYPE) {
              if (q.object.termType === 'NamedNode') {
                const cls = q.object.value;
                let subs = classMap.get(cls);
                if (!subs) { subs = new Set(); classMap.set(cls, subs); }
                subs.add(q.subject.value);
              }
            } else {
              propMap.set(p, (propMap.get(p) || 0) + 1);
            }
          }
          classCounts = [...classMap.entries()]
            .map(([cls, subs]) => ({ cls, count: subs.size }))
            .sort((a, b) => b.count - a.count)
            .slice(0, 100);
          propCounts = [...propMap.entries()]
            .map(([prop, count]) => ({ prop, count }))
            .sort((a, b) => b.count - a.count)
            .slice(0, 100);
        }
      } else {
        const clsQ  = `SELECT ?cls (COUNT(DISTINCT ?s) AS ?c)\n${scopeFromClause}\nWHERE { ?s a ?cls } GROUP BY ?cls ORDER BY DESC(?c) LIMIT 100`;
        const propQ = `SELECT ?p (COUNT(*) AS ?c)\n${scopeFromClause}\nWHERE { ?s ?p ?o . FILTER(?p != <${RDF_TYPE}>) } GROUP BY ?p ORDER BY DESC(?c) LIMIT 100`;
        const [clsRes, propRes] = await Promise.all([sparqlQuery(clsQ), sparqlQuery(propQ)]);
        classCounts = (clsRes?.results?.bindings || [])
          .map((b: any) => ({ cls: b.cls?.value, count: parseInt(b.c?.value || '0', 10) }))
          .filter((x: any) => x.cls);
        propCounts = (propRes?.results?.bindings || [])
          .map((b: any) => ({ prop: b.p?.value, count: parseInt(b.c?.value || '0', 10) }))
          .filter((x: any) => x.prop);
      }
    } catch { classCounts = []; propCounts = []; }
    finally { classLoading = false; }
  }

  // ── Graph view ──────────────────────────────────────────────────────────────
  let graphCanvas: any;
  let activeLayout = 'cose-bilkent';
  let graphNodes: any[] = [];
  let graphEdges: any[] = [];
  let graphLoading = false;
  let graphLoadingMore = false;
  let graphOffset = 0;
  let graphHasMore = false;
  let browseExpansionCache = new Map<string, { nodes: any[]; edges: any[] }>();
  let browseExpandedUris = new Map<string, { nodeIds: Set<string>; edgeIds: Set<string> }>();
  let browseExpandedDirs = new Map<string, Set<'in' | 'out'>>();
  let browseExpandingUri: string | null = null;
  $: browseExpandedIris = new Set(browseExpandedUris.keys());
  $: browseExhaustedIris = new Set(
    [...browseExpandedDirs.entries()]
      .filter(([, dirs]) => dirs.has('in') && dirs.has('out'))
      .map(([iri]) => iri)
  );

  let browseCtxVisible = false;
  let browseCtxX = 0, browseCtxY = 0;
  let browseCtxItems: any[] = [];
  let browseCtxNodeData: any = null;

  // Fetch a page of triples within the ontology's graph scope, returning
  // bindings in SPARQL JSON shape. Works for both rawDataUrl and SPARQL paths.
  async function fetchScopedBindings(limit: number, offset: number): Promise<any[]> {
    if (rawDataUrl) {
      await ensureStoreLoaded();
      if (!loadedStore) return [];
      const quads = loadedStore.getQuads(null, null, null, null);
      return quads.slice(offset, offset + limit).map((q: any) => ({
        s: n3TermToBinding(q.subject),
        p: n3TermToBinding(q.predicate),
        o: n3TermToBinding(q.object),
      }));
    }
    try {
      const res = await sparqlQuery(
        `SELECT ?s ?p ?o\n${scopeFromClause}\nWHERE { ?s ?p ?o } LIMIT ${limit} OFFSET ${offset}`
      );
      return res?.results?.bindings || [];
    } catch { return []; }
  }

  async function fetchOutgoing(uri: string, limit = 80): Promise<any[]> {
    if (rawDataUrl) {
      await ensureStoreLoaded();
      if (!loadedStore) return [];
      const quads = loadedStore.getQuads(null, null, null, null) as any[];
      return quads
        .filter((q) => q.subject.value === uri)
        .slice(0, limit)
        .map((q) => ({
          s: { type: 'uri', value: uri },
          p: n3TermToBinding(q.predicate),
          o: n3TermToBinding(q.object),
        }));
    }
    try {
      const res = await sparqlQuery(`SELECT ?p ?o\n${scopeFromClause}\nWHERE { <${uri}> ?p ?o } LIMIT ${limit}`);
      return (res?.results?.bindings || []).map((row: any) => ({
        s: { type: 'uri', value: uri }, p: row.p, o: row.o,
      }));
    } catch { return []; }
  }

  async function fetchIncoming(uri: string, limit = 30): Promise<any[]> {
    if (rawDataUrl) {
      await ensureStoreLoaded();
      if (!loadedStore) return [];
      const quads = loadedStore.getQuads(null, null, null, null) as any[];
      return quads
        .filter((q) => q.object.termType === 'NamedNode' && q.object.value === uri)
        .slice(0, limit)
        .map((q) => ({
          s: n3TermToBinding(q.subject),
          p: n3TermToBinding(q.predicate),
          o: { type: 'uri', value: uri },
        }));
    }
    try {
      const res = await sparqlQuery(`SELECT ?s ?p\n${scopeFromClause}\nWHERE { ?s ?p <${uri}> } LIMIT ${limit}`);
      return (res?.results?.bindings || []).map((row: any) => ({
        s: row.s, p: row.p, o: { type: 'uri', value: uri },
      }));
    } catch { return []; }
  }

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
      const bindings = await fetchScopedBindings(100, 0);
      const { nodes, edges } = graphResultsToElements(bindings, 's', 'p', 'o', 250);
      graphHasMore = nodes.some((n) => n.data.nodeType === 'uri');
      graphNodes = nodes;
      graphEdges = edges;
      graphOffset = 100;
    } catch {
      graphNodes = []; graphEdges = [];
    } finally {
      graphLoading = false;
    }
  }

  async function loadMoreGraphTriples() {
    if (graphLoading || graphLoadingMore || !graphHasMore) return;
    graphLoadingMore = true;
    try {
      const unexplored = graphNodes.filter((n) => {
        const iri = n.data?.fullIri;
        const nt = n.data?.nodeType;
        if (!iri || nt === 'literal' || nt === 'bnode') return false;
        const dirs = browseExpandedDirs.get(iri);
        return !dirs || !dirs.has('in') || !dirs.has('out');
      });
      if (unexplored.length > 0) {
        for (const node of unexplored.slice(0, 10)) {
          await browseExpandUri(node.data.fullIri, 'both');
        }
        graphHasMore = graphNodes.some((n) => {
          const iri = n.data?.fullIri;
          if (!iri || n.data?.nodeType === 'literal' || n.data?.nodeType === 'bnode') return false;
          const dirs = browseExpandedDirs.get(iri);
          return !dirs || !dirs.has('in') || !dirs.has('out');
        });
      } else {
        const bindings = await fetchScopedBindings(100, graphOffset);
        const { nodes, edges } = graphResultsToElements(bindings, 's', 'p', 'o', 250);
        const existingIds = new Set(graphNodes.map((n) => n.data.id));
        const existingEdgeIds = new Set(graphEdges.map((e) => e.data.id));
        graphNodes = [...graphNodes, ...nodes.filter((n) => !existingIds.has(n.data.id))];
        graphEdges = [...graphEdges, ...edges.filter((e) => !existingEdgeIds.has(e.data.id))];
        graphOffset += 100;
        graphHasMore = bindings.length >= 100;
      }
    } catch {} finally {
      graphLoadingMore = false;
    }
  }

  async function browseExpandUri(uri: string, direction: 'in' | 'out' | 'both' = 'both') {
    if (!uri || uri.startsWith('_:') || (!uri.includes('://') && !uri.startsWith('urn:'))) return;
    browseExpandingUri = uri;
    try {
      const cacheKey = `${uri}::${direction}`;
      const applyElements = (newNodes: any[], newEdges: any[]) => {
        const existingIds = new Set(graphNodes.map((n) => n.data.id));
        const existingEdgeIds = new Set(graphEdges.map((e) => e.data.id));
        const nodesToAdd = newNodes.filter((n) => !existingIds.has(n.data.id));
        const edgesToAdd = newEdges.filter((e) => !existingEdgeIds.has(e.data.id));
        graphNodes = [...graphNodes, ...nodesToAdd];
        graphEdges = [...graphEdges, ...edgesToAdd];
        const prev = browseExpandedUris.get(uri) || { nodeIds: new Set(), edgeIds: new Set() };
        browseExpandedUris = new Map(browseExpandedUris).set(uri, {
          nodeIds: new Set([...prev.nodeIds, ...nodesToAdd.map((n) => n.data.id)]),
          edgeIds: new Set([...prev.edgeIds, ...edgesToAdd.map((e) => e.data.id)]),
        });
        const dirs = new Set(browseExpandedDirs.get(uri) || []);
        if (direction === 'both') { dirs.add('in'); dirs.add('out'); } else dirs.add(direction);
        browseExpandedDirs = new Map(browseExpandedDirs).set(uri, dirs);
      };

      if (browseExpansionCache.has(cacheKey)) {
        const { nodes, edges } = browseExpansionCache.get(cacheKey)!;
        applyElements(nodes, edges);
        return;
      }

      const outPromise = (direction === 'both' || direction === 'out') ? fetchOutgoing(uri) : Promise.resolve([]);
      const inPromise  = (direction === 'both' || direction === 'in')  ? fetchIncoming(uri) : Promise.resolve([]);
      const [outBindings, inBindings] = await Promise.all([outPromise, inPromise]);
      const { nodes: newNodes, edges: newEdges } = graphResultsToElements([...outBindings, ...inBindings], 's', 'p', 'o', 250);
      browseExpansionCache = new Map(browseExpansionCache).set(cacheKey, { nodes: newNodes, edges: newEdges });
      applyElements(newNodes, newEdges);
    } catch {} finally {
      browseExpandingUri = null;
    }
  }

  function browseCollapseUri(uri: string) {
    const expanded = browseExpandedUris.get(uri);
    if (!expanded) return;
    const { nodeIds, edgeIds } = expanded;
    const otherNodeIds = new Set<string>();
    const otherEdgeIds = new Set<string>();
    for (const [otherUri, data] of browseExpandedUris) {
      if (otherUri === uri) continue;
      for (const id of data.nodeIds) otherNodeIds.add(id);
      for (const id of data.edgeIds) otherEdgeIds.add(id);
    }
    const removeNodes = new Set([...nodeIds].filter((id) => !otherNodeIds.has(id)));
    const removeEdges = new Set([...edgeIds].filter((id) => !otherEdgeIds.has(id)));
    graphNodes = graphNodes.filter((n) => !removeNodes.has(n.data.id));
    const keptNodeIds = new Set(graphNodes.map((n) => n.data.id));
    graphEdges = graphEdges.filter((e) =>
      !removeEdges.has(e.data.id) && keptNodeIds.has(e.data.source) && keptNodeIds.has(e.data.target)
    );
    const next = new Map(browseExpandedUris); next.delete(uri); browseExpandedUris = next;
    const nextDirs = new Map(browseExpandedDirs); nextDirs.delete(uri); browseExpandedDirs = nextDirs;
  }

  function handleBrowseNodeExpand(e: CustomEvent) {
    if (e.detail.fullIri) browseExpandUri(e.detail.fullIri);
  }

  function buildBrowseNodeMenu(data: any) {
    const items: any[] = [];
    if (data.nodeType === 'uri' && data.fullIri) {
      const expandedDirs = browseExpandedDirs.get(data.fullIri) || new Set();
      const hasExpanded = expandedDirs.size > 0;
      if (hasExpanded) items.push({ label: $t('components.ontologyBrowserPanel.collapse'), icon: Unlink, action: 'collapse' });
      if (!expandedDirs.has('out')) items.push({ label: $t('components.ontologyBrowserPanel.expandOutgoing'), icon: ChevronRight, action: 'expandOut' });
      if (!expandedDirs.has('in'))  items.push({ label: $t('components.ontologyBrowserPanel.expandIncoming'), icon: ChevronLeft,  action: 'expandIn'  });
      if (!expandedDirs.has('in') || !expandedDirs.has('out'))
        items.push({ label: $t('components.ontologyBrowserPanel.expandBoth'), icon: Plus, action: 'expandBoth' });
      items.push({ divider: true });
      items.push({ label: $t('components.ontologyBrowserPanel.openResource'), icon: BookOpen, action: 'openResource' });
      items.push({ label: $t('components.ontologyBrowserPanel.copyIri'), icon: Copy, action: 'copyIri' });
    }
    items.push({ label: $t('components.ontologyBrowserPanel.removeFromGraph'), icon: Unlink, action: 'remove', danger: true });
    return items;
  }

  function handleBrowseNodeContextMenu(e: CustomEvent) {
    const { data, x, y } = e.detail;
    browseCtxNodeData = data;
    browseCtxItems = buildBrowseNodeMenu(data);
    browseCtxX = x; browseCtxY = y; browseCtxVisible = true;
  }

  function handleBrowseCanvasContextMenu(e: CustomEvent) {
    browseCtxNodeData = null;
    browseCtxItems = [
      ...(graphHasMore ? [{ label: $t('components.ontologyBrowserPanel.load100MoreTriples'), icon: Network, action: 'loadMore' }] : []),
      { label: $t('components.ontologyBrowserPanel.fitAll'), icon: Maximize2, action: 'fit' },
      { divider: true },
      { label: $t('components.ontologyBrowserPanel.exportPng'), icon: Share2, action: 'export' },
    ];
    browseCtxX = e.detail.x; browseCtxY = e.detail.y; browseCtxVisible = true;
  }

  function handleBrowseCtxAction(e: CustomEvent) {
    const action = e.detail;
    if      (action === 'fit')       graphCanvas?.fitAll();
    else if (action === 'export')    graphCanvas?.exportPng();
    else if (action === 'loadMore')  loadMoreGraphTriples();
    else if (browseCtxNodeData) {
      const data = browseCtxNodeData;
      if      (action === 'expandOut')    browseExpandUri(data.fullIri, 'out');
      else if (action === 'expandIn')     browseExpandUri(data.fullIri, 'in');
      else if (action === 'expandBoth')   browseExpandUri(data.fullIri, 'both');
      else if (action === 'collapse')     browseCollapseUri(data.fullIri);
      else if (action === 'openResource') openResource(data.fullIri);
      else if (action === 'copyIri')      void copyToClipboard(data.fullIri);
      else if (action === 'remove')       graphCanvas?.removeNode(data.id);
    }
  }
</script>

<div class="ob-panel">
  <!-- Mode tabs -->
  <div class="ob-tabs">
    <button class="ob-tab" class:ob-active={activeMode === 'ontology'} on:click={() => switchMode('ontology')}>
      <BookOpen size={14} /> {$t('components.ontologyBrowserPanel.tabOntology')}
    </button>
    <button class="ob-tab" class:ob-active={activeMode === 'graph'} on:click={() => switchMode('graph')}>
      <Network size={14} /> {$t('components.ontologyBrowserPanel.tabGraph')}
    </button>
    <button class="ob-tab" class:ob-active={activeMode === 'tabular'} on:click={() => switchMode('tabular')}>
      <Table2 size={14} /> {$t('components.ontologyBrowserPanel.tabTabular')}
    </button>
    <button class="ob-tab" class:ob-active={activeMode === 'schema'} on:click={() => switchMode('schema')}>
      <Shapes size={14} /> {$t('components.ontologyBrowserPanel.tabSchema')}
    </button>

    <span class="ob-status">
      {#if activeMode === 'graph' && (graphLoading || graphLoadingMore)}
        <Loader2 size={12} class="animate-spin" /> {$t('components.ontologyBrowserPanel.loadingLower')}
      {:else if activeMode === 'graph'}
        {$t('components.ontologyBrowserPanel.nodesEdges', { values: { nodes: graphNodes.length, edges: graphEdges.length } })}
      {:else if activeMode === 'schema' && !classLoading}
        {$t('components.ontologyBrowserPanel.classesProps', { values: { classes: classCounts.length, props: propCounts.length } })}
      {/if}
    </span>

    {#if versionLabel}
      <span class="ob-version-label">v{versionLabel}</span>
    {/if}
  </div>

  <!-- Panel body -->
  <div class="ob-body">
    {#if activeMode === 'ontology'}
      <OntologyModelViewer {graphIri} {subGraphs} {versionLabel} initialTab="classes" preloadedStore={loadedStore} />

    {:else if activeMode === 'graph'}
      <div class="ob-graph-area">
        {#if graphNodes.length === 0 && !graphLoading && !graphLoadingMore}
          <div class="ob-graph-empty">
            <Network size={52} strokeWidth={1} />
            <p class="ob-graph-empty-title">{$t('components.ontologyBrowserPanel.noGraphData')}</p>
            <p class="ob-graph-empty-sub">{storeError || $t('components.ontologyBrowserPanel.noGraphDataHint')}</p>
          </div>
        {:else}
          <GraphCanvas
            bind:this={graphCanvas}
            nodes={graphNodes}
            edges={graphEdges}
            layout={activeLayout}
            height="100%"
            loading={graphLoading && graphNodes.length === 0}
            loadingMore={graphLoadingMore}
            expandedNodes={browseExpandedIris}
            expandingNode={browseExpandingUri}
            exhaustedNodes={browseExhaustedIris}
            on:nodeExpand={handleBrowseNodeExpand}
            on:nodeOpen={(e) => e.detail.fullIri && openResource(e.detail.fullIri)}
            on:nodeContextMenu={handleBrowseNodeContextMenu}
            on:canvasContextMenu={handleBrowseCanvasContextMenu}
          />
        {/if}
      </div>

    {:else if activeMode === 'tabular'}
      <div class="ob-table-bar">
        <div class="ob-filter">
          <Search size={13} />
          <input
            type="text"
            placeholder={$t('components.ontologyBrowserPanel.filterByIri')}
            bind:value={tripleFilter}
            on:keydown={(e) => e.key === 'Enter' && loadTriples(0)}
          />
          {#if tripleFilter}
            <button class="ob-clear" on:click={() => { tripleFilter = ''; loadTriples(0); }}><X size={12} /></button>
          {/if}
          <button class="ob-go" on:click={() => loadTriples(0)}>{$t('components.ontologyBrowserPanel.go')}</button>
        </div>
        {#if triples.length}
          <div class="ob-page-info">
            <span>{$t('components.ontologyBrowserPanel.rowsRange', { values: { from: tripleOffset + 1, to: tripleOffset + triples.length } })}</span>
            <button class="ob-page-btn" disabled={tripleOffset === 0} on:click={() => loadTriples(tripleOffset - PAGE)}>
              <ChevronLeft size={14} />
            </button>
            <button class="ob-page-btn" disabled={!tripleHasMore} on:click={() => loadTriples(tripleOffset + PAGE)}>
              <ChevronRight size={14} />
            </button>
          </div>
        {/if}
      </div>

      {#if tripleLoading}
        <div class="ob-loading"><Loader2 size={20} class="animate-spin" /> {$t('components.ontologyBrowserPanel.loadingTriples')}</div>
      {:else if triples.length === 0}
        <div class="ob-empty">{tripleFilter ? $t('components.ontologyBrowserPanel.noTriplesFilter') : $t('components.ontologyBrowserPanel.noTriples')}</div>
      {:else}
        <div class="ob-table-wrap">
          <table class="ob-table">
            <thead><tr><th>{$t('components.ontologyBrowserPanel.subject')}</th><th>{$t('components.ontologyBrowserPanel.predicate')}</th><th>{$t('components.ontologyBrowserPanel.object')}</th></tr></thead>
            <tbody>
              {#each triples as row}
                <tr>
                  <td><RdfTerm term={row.s} /></td>
                  <td><RdfTerm term={row.p} /></td>
                  <td><RdfTerm term={row.o} /></td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
      {/if}

    {:else if activeMode === 'schema'}
      <div class="classes-tabs">
        <button class="ctab" class:ctab-active={classesTab === 'classes'}
          on:click={() => classesTab = 'classes'}>
          {$t('components.ontologyBrowserPanel.classesLabel')} ({filteredClassCounts.length}{classesSearch ? `/${classCounts.length}` : ''})
        </button>
        <button class="ctab" class:ctab-active={classesTab === 'properties'}
          on:click={() => classesTab = 'properties'}>
          {$t('components.ontologyBrowserPanel.propertiesLabel')} ({filteredPropCounts.length}{classesSearch ? `/${propCounts.length}` : ''})
        </button>
        <div class="classes-search-wrap">
          <input
            type="search"
            class="classes-search"
            placeholder={classesTab === 'classes' ? $t('components.ontologyBrowserPanel.searchClassesPlaceholder') : $t('components.ontologyBrowserPanel.searchPropertiesPlaceholder')}
            bind:value={classesSearch}
            aria-label={classesTab === 'classes' ? $t('components.ontologyBrowserPanel.searchClassesAria') : $t('components.ontologyBrowserPanel.searchPropertiesAria')}
          />
          {#if classesSearch}
            <button class="classes-search-clear" on:click={() => classesSearch = ''} title={$t('components.ontologyBrowserPanel.clearSearch')} aria-label={$t('components.ontologyBrowserPanel.clearSearch')}>
              <X size={12} />
            </button>
          {/if}
        </div>
      </div>

      <div class="classes-body">
        {#if classLoading}
          <p class="muted-tip"><Loader2 size={14} class="animate-spin" /> {$t('components.ontologyBrowserPanel.computingDistribution')}</p>
        {:else if classesTab === 'classes'}
          {#if classCounts.length === 0}
            <div class="ob-empty"><p>{$t('components.ontologyBrowserPanel.noRdfTypeBefore')}<code>rdf:type</code>{$t('components.ontologyBrowserPanel.noRdfTypeAfter')}</p></div>
          {:else if filteredClassCounts.length === 0}
            <div class="ob-empty"><p>{$t('components.ontologyBrowserPanel.noClassesMatch')}<code>{classesSearch}</code>.</p></div>
          {:else}
            {@const maxCount = Math.max(...filteredClassCounts.map(c => c.count))}
            <ol class="class-bars">
              {#each filteredClassCounts as c}
                <li class="class-bar-row">
                  <button class="class-bar-label" title={c.cls} on:click={() => openResource(c.cls)}>
                    <span class="class-bar-name">{shortIri(c.cls)}</span>
                    <span class="class-bar-count">{c.count.toLocaleString()}</span>
                  </button>
                  <div class="class-bar-track"><div class="class-bar-fill cls-fill" style="width: {(c.count / maxCount) * 100}%"></div></div>
                </li>
              {/each}
            </ol>
          {/if}
        {:else}
          {#if propCounts.length === 0}
            <div class="ob-empty"><p>{$t('components.ontologyBrowserPanel.noProperties')}</p></div>
          {:else if filteredPropCounts.length === 0}
            <div class="ob-empty"><p>{$t('components.ontologyBrowserPanel.noPropertiesMatch')}<code>{classesSearch}</code>.</p></div>
          {:else}
            {@const maxCount = Math.max(...filteredPropCounts.map(c => c.count))}
            <ol class="class-bars">
              {#each filteredPropCounts as c}
                <li class="class-bar-row">
                  <button class="class-bar-label" title={c.prop} on:click={() => openResource(c.prop)}>
                    <span class="class-bar-name">{shortIri(c.prop)}</span>
                    <span class="class-bar-count">{c.count.toLocaleString()}</span>
                  </button>
                  <div class="class-bar-track"><div class="class-bar-fill prop-fill" style="width: {(c.count / maxCount) * 100}%"></div></div>
                </li>
              {/each}
            </ol>
          {/if}
        {/if}
      </div>
    {/if}
  </div>
</div>

<!-- Graph context menu -->
<ContextMenu
  bind:visible={browseCtxVisible}
  bind:x={browseCtxX}
  bind:y={browseCtxY}
  items={browseCtxItems}
  on:action={handleBrowseCtxAction}
/>

<style>
  .ob-panel {
    border: 1px solid var(--line-soft, #e2e8f0);
    border-radius: 12px;
    background: white;
    overflow: hidden;
  }

  .ob-tabs {
    display: flex;
    align-items: center;
    gap: 0;
    border-bottom: 1px solid var(--line-soft, #e2e8f0);
    background: var(--bg-soft, #f8fafc);
    padding: 0 0.75rem;
  }
  .ob-tab {
    display: inline-flex; align-items: center; gap: 0.35rem;
    padding: 0.6rem 0.9rem;
    font-size: 0.82rem; font-weight: 500;
    border: none; background: transparent;
    color: var(--ink-500, #64748b);
    cursor: pointer;
    border-bottom: 2px solid transparent;
    transition: color 0.12s;
    white-space: nowrap;
    margin-bottom: -1px;
  }
  .ob-tab:hover { color: var(--ink-800, #1e293b); }
  .ob-tab.ob-active {
    color: var(--brand-600, #4f46e5);
    border-bottom-color: var(--brand-500, #6366f1);
    background: white;
  }

  .ob-status {
    display: inline-flex; align-items: center; gap: 0.3rem;
    margin-left: 0.6rem;
    font-size: 0.72rem; color: var(--ink-400, #94a3b8);
    white-space: nowrap;
  }
  .ob-version-label {
    margin-left: auto;
    font-size: 0.72rem; font-weight: 600;
    color: var(--ink-400, #94a3b8);
    padding: 0 0.5rem; white-space: nowrap;
  }

  .ob-body { min-height: 300px; }

  .ob-loading, .ob-empty {
    display: flex; align-items: center; justify-content: center; gap: 0.5rem;
    padding: 3rem 1rem; color: var(--ink-400, #94a3b8); font-size: 0.875rem;
  }

  /* Graph view — scale with the viewport so it feels generous on big screens. */
  .ob-graph-area {
    position: relative;
    height: clamp(460px, 72vh, 820px);
  }
  .ob-graph-empty {
    display: flex; flex-direction: column; align-items: center; justify-content: center;
    height: 100%; color: var(--ink-400, #94a3b8); gap: 0.4rem;
  }
  .ob-graph-empty-title { margin: 0.4rem 0 0; font-weight: 600; color: var(--ink-600); }
  .ob-graph-empty-sub { margin: 0; font-size: 0.82rem; }

  /* Tabular */
  .ob-table-bar {
    display: flex; align-items: center; justify-content: space-between;
    gap: 0.75rem; padding: 0.6rem 0.75rem;
    border-bottom: 1px solid var(--line-soft, #e2e8f0);
    background: var(--bg-soft, #f8fafc);
    flex-wrap: wrap;
  }
  .ob-filter {
    display: flex; align-items: center; gap: 0.4rem;
    padding: 0.3rem 0.6rem; border: 1px solid var(--line-soft, #d1d5db);
    border-radius: 8px; background: white;
    flex: 1; min-width: 180px; color: var(--ink-400);
  }
  .ob-filter input {
    flex: 1; border: none; outline: none; font-size: 0.82rem;
    background: transparent; color: var(--ink-800);
  }
  .ob-clear { border: none; background: transparent; cursor: pointer; color: var(--ink-400); display: flex; align-items: center; }
  .ob-go {
    border: none; background: var(--brand-500, #6366f1); color: white;
    border-radius: 5px; padding: 0.2rem 0.5rem;
    font-size: 0.75rem; font-weight: 600; cursor: pointer;
  }
  .ob-page-info {
    display: flex; align-items: center; gap: 0.35rem;
    font-size: 0.78rem; color: var(--ink-400); white-space: nowrap;
  }
  .ob-page-btn {
    border: 1px solid var(--line-soft); background: white; border-radius: 5px;
    padding: 0.15rem 0.3rem; cursor: pointer; display: flex; align-items: center;
  }
  .ob-page-btn:disabled { opacity: 0.4; cursor: not-allowed; }

  .ob-table-wrap { overflow-x: auto; max-height: 68vh; overflow-y: auto; }
  .ob-table { width: 100%; border-collapse: collapse; font-size: 0.8rem; }
  .ob-table th {
    position: sticky; top: 0;
    background: var(--bg-soft, #f8fafc);
    border-bottom: 1px solid var(--line-soft, #e2e8f0);
    padding: 0.45rem 0.75rem; text-align: left;
    font-weight: 600; font-size: 0.75rem; color: var(--ink-500); z-index: 1;
  }
  .ob-table td {
    padding: 0.35rem 0.75rem; border-bottom: 1px solid var(--line-soft, #f1f5f9);
    vertical-align: top; max-width: 300px; overflow: hidden; text-overflow: ellipsis;
  }
  .ob-table tr:last-child td { border-bottom: none; }
  .ob-table tr:hover td { background: #f8fafc; }

  /* Schema (classes/properties distribution) */
  .classes-tabs {
    display: flex; gap: 0;
    border-bottom: 1px solid #e2e8f0;
    padding: 0 1rem; background: #f8fafc; flex-shrink: 0;
  }
  .ctab {
    padding: 0.6rem 1rem; border: none; background: transparent; cursor: pointer;
    font-size: 0.85rem; font-weight: 500; color: #64748b;
    border-bottom: 2px solid transparent; margin-bottom: -1px;
    transition: all 0.12s;
  }
  .ctab:hover { color: #2563eb; }
  .ctab-active { color: #2563eb; border-bottom-color: #2563eb; font-weight: 600; }

  .classes-search-wrap {
    margin-left: auto; align-self: center;
    position: relative; padding: 0.4rem 0;
  }
  .classes-search {
    width: 260px; padding: 0.3rem 1.6rem 0.3rem 0.55rem;
    border: 1px solid #d0d7de; border-radius: 4px;
    font-size: 0.8rem; background: #fff; color: #1f2937;
  }
  .classes-search:focus { outline: 2px solid #93c5fd; outline-offset: 0; border-color: #2563eb; }
  .classes-search-clear {
    position: absolute; right: 4px; top: 50%;
    transform: translateY(-50%);
    background: none; border: none; cursor: pointer;
    padding: 3px; border-radius: 3px; color: #94a3b8;
    display: inline-flex; align-items: center;
  }
  .classes-search-clear:hover { background: #f1f5f9; color: #1f2937; }

  .classes-body { padding: 1rem 1.25rem; overflow-y: auto; max-height: 65vh; }
  .muted-tip { display: inline-flex; align-items: center; gap: 0.4rem; color: #64748b; font-size: 0.85rem; }
  .class-bars { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 6px; }
  .class-bar-row { display: flex; flex-direction: column; gap: 4px; }
  .class-bar-label {
    display: flex; align-items: baseline; justify-content: space-between;
    background: transparent; border: none; padding: 2px 0; cursor: pointer;
    color: #334155; font-size: 0.85rem; text-align: left; width: 100%;
  }
  .class-bar-label:hover .class-bar-name { color: #2563eb; text-decoration: underline; }
  .class-bar-name { font-weight: 600; }
  .class-bar-count { font-variant-numeric: tabular-nums; font-size: 0.78rem; color: #64748b; }
  .class-bar-track { background: #f1f5f9; height: 6px; border-radius: 999px; overflow: hidden; }
  .class-bar-fill { height: 100%; }
  .cls-fill  { background: linear-gradient(90deg, #3b82f6, #6d28d9); }
  .prop-fill { background: linear-gradient(90deg, #059669, #0891b2); }

  :global(:is([data-theme="dark"], .dark)) .ob-panel { background: var(--bg-strong); }
  :global(:is([data-theme="dark"], .dark)) .ob-tab.ob-active { background: var(--bg-strong); }
  :global(:is([data-theme="dark"], .dark)) .ob-filter { background: var(--bg-soft); }
  :global(:is([data-theme="dark"], .dark)) .ob-page-btn { background: var(--bg-soft); }
  :global(:is([data-theme="dark"], .dark)) .ob-table tr:hover td { background: rgba(255,255,255,0.04); }
  :global(:is([data-theme="dark"], .dark)) .classes-tabs { background: var(--bg-soft); border-color: var(--line-strong); }
  :global(:is([data-theme="dark"], .dark)) .ctab:hover { color: #93c5fd; }
  :global(:is([data-theme="dark"], .dark)) .ctab-active { color: #93c5fd; border-bottom-color: #93c5fd; }
  :global(:is([data-theme="dark"], .dark)) .classes-search { background: var(--bg-soft); border-color: var(--line-strong); color: var(--ink-900); }
  :global(:is([data-theme="dark"], .dark)) .classes-search-clear:hover { background: rgba(255,255,255,0.06); color: var(--ink-900); }
  :global(:is([data-theme="dark"], .dark)) .class-bar-label { color: var(--ink-800); }
  :global(:is([data-theme="dark"], .dark)) .class-bar-label:hover .class-bar-name { color: #93c5fd; }
  :global(:is([data-theme="dark"], .dark)) .class-bar-track { background: rgba(255,255,255,0.08); }
</style>
