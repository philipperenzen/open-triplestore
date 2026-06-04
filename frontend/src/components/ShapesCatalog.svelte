<script>
  // The "Shapes" catalog: graph-first discovery of every SHACL shape in the
  // store (dedicated shape graphs AND shapes joined with instance data/models).
  // Real stores hold tens of thousands of shapes, so we list the source graphs
  // (with counts) and lazy-load a graph's shapes only when it's opened.
  // Two modes:
  //   • page mode — compose picks into a NEW shape graph or an existing one;
  //     register a discovered shapes-bearing graph in place.
  //   • picker mode (`picker` + `targetGraphId`) — add picks to one shape graph
  //     (used from the editor's "Add existing shapes"); emits `imported`.
  import { onMount, createEventDispatcher } from 'svelte';
  import {
    listShapesCatalog, listShapeGraphs, createShapeGraph, importShapesIntoGraph, registerShapeGraph,
    getShapeGraphTurtle,
  } from '../lib/api.js';
  import { Search, X, Plus, Check, Database, FileCode, Layers, FolderInput, BookmarkPlus, Loader2, ChevronRight, ChevronDown, ExternalLink } from 'lucide-svelte';
  import { navigate } from '../lib/router/index.js';
  import { openPendingViewerTab, showShapesInViewer, viewerConfigured } from '../lib/graphViewer.ts';
  import Select from './Select.svelte';
  import { toastError, toastSuccess } from '../lib/toast.ts';

  export let picker = false;
  export let targetGraphId = '';     // shape-graph id to import into (picker mode)
  export let excludeGraphIri = '';   // hide this graph (the target's own backing graph)

  const dispatch = createEventDispatcher();
  const SEP = '\0';

  let graphs = [];                   // summary rows: {graph, node_count, property_count, total, registered, shape_graph_id, shape_graph_name}
  let shapesByGraph = {};            // graph IRI → shapes[] (lazy)
  let expanded = new Set();
  let loadingGraph = new Set();
  let shapeGraphs = [];              // existing shape graphs (add-to-existing)
  let loading = false;
  let error = '';
  let busy = false;

  let search = '';
  let selected = new Set();
  let addTargetId = '';
  // Shape-type filter: see node shapes, property shapes, or both. Backed by the
  // catalog's per-shape `kind` ('node' | 'property') and per-graph counts.
  let kindFilter = 'all'; // 'all' | 'node' | 'property'

  // Per-graph "Open in graph viewer" loading state (keyed by graph IRI).
  let openingViewer = new Set();

  onMount(reload);

  async function reload() {
    loading = true;
    error = '';
    try {
      const [summary, sgs] = await Promise.all([listShapesCatalog(), listShapeGraphs().catch(() => [])]);
      graphs = (summary?.graphs || []).filter((g) => !(picker && (g.shape_graph_id === targetGraphId || g.graph === excludeGraphIri)));
      shapeGraphs = sgs || [];
    } catch (e) {
      error = e.message;
    } finally {
      loading = false;
    }
  }

  async function toggleExpand(g) {
    if (expanded.has(g.graph)) {
      expanded.delete(g.graph);
      expanded = new Set(expanded);
      return;
    }
    expanded.add(g.graph);
    expanded = new Set(expanded);
    if (!shapesByGraph[g.graph]) {
      loadingGraph.add(g.graph); loadingGraph = new Set(loadingGraph);
      try {
        const res = await listShapesCatalog(g.graph);
        shapesByGraph = { ...shapesByGraph, [g.graph]: res?.shapes || [] };
      } catch (e) {
        toastError(e.message);
      } finally {
        loadingGraph.delete(g.graph); loadingGraph = new Set(loadingGraph);
      }
    }
  }

  const key = (graph, shape) => `${graph}${SEP}${shape}`;
  function toggle(graph, shape) {
    const k = key(graph, shape);
    if (selected.has(k)) selected.delete(k); else selected.add(k);
    selected = new Set(selected);
  }
  function selectedRefs() {
    return [...selected].map((k) => {
      const [source_graph, shape] = k.split(SEP);
      return { source_graph, shape };
    });
  }

  function shortIRI(iri) { const m = String(iri).match(/[^#/]+$/); return m ? m[0] : iri; }

  // Hide graphs that hold none of the selected shape kind, so "Node shapes" /
  // "Property shapes" only surfaces graphs that actually have them. NB: the
  // kindFilter check is inlined (not factored into a helper) so Svelte tracks it
  // as a dependency of this reactive block and re-filters when the toggle flips.
  $: filteredGraphs = graphs.filter((g) => {
    if (kindFilter === 'node' && !((g.node_count || 0) > 0)) return false;
    if (kindFilter === 'property' && !((g.property_count || 0) > 0)) return false;
    if (search.trim()) return `${g.graph} ${g.shape_graph_name || ''}`.toLowerCase().includes(search.trim().toLowerCase());
    return true;
  });

  // Totals for the summary line, respecting the active kind filter.
  $: totalNode = graphs.reduce((a, g) => a + (g.node_count || 0), 0);
  $: totalProperty = graphs.reduce((a, g) => a + (g.property_count || 0), 0);

  function visibleShapes(graph) {
    let all = shapesByGraph[graph] || [];
    if (kindFilter !== 'all') all = all.filter((s) => s.kind === kindFilter);
    if (!search.trim()) return all;
    const q = search.trim().toLowerCase();
    return all.filter((s) => [s.shape, s.label || '', ...(s.target_classes || [])].join(' ').toLowerCase().includes(q));
  }

  async function openGraphInViewer(g) {
    if (!g.shape_graph_id || openingViewer.has(g.graph)) return;
    // Pre-open synchronously to dodge the popup blocker (window.open after an
    // await is treated as a blocked popup), then navigate once Turtle loads.
    const win = openPendingViewerTab();
    openingViewer.add(g.graph); openingViewer = new Set(openingViewer);
    try {
      const ttl = await getShapeGraphTurtle(g.shape_graph_id);
      if (!ttl || !ttl.trim()) { toastError('No shapes to open in this graph.'); win?.close(); return; }
      showShapesInViewer(win, ttl);
    } catch (e) {
      toastError(e.message); win?.close();
    } finally {
      openingViewer.delete(g.graph); openingViewer = new Set(openingViewer);
    }
  }

  function selectAllVisible(graph, on) {
    for (const s of visibleShapes(graph)) { if (on) selected.add(key(graph, s.shape)); else selected.delete(key(graph, s.shape)); }
    selected = new Set(selected);
  }
  const allVisibleSelected = (graph) => { const v = visibleShapes(graph); return v.length > 0 && v.every((s) => selected.has(key(graph, s.shape))); };

  async function addToTarget() {
    if (!selected.size) return;
    busy = true;
    try {
      const res = await importShapesIntoGraph(targetGraphId, selectedRefs());
      toastSuccess(`Added ${res.imported} shape${res.imported === 1 ? '' : 's'} (v${res.version})`);
      selected = new Set();
      dispatch('imported', res);
    } catch (e) { toastError(e.message); } finally { busy = false; }
  }

  async function createFromSelection() {
    if (!selected.size) return;
    const name = (prompt('Name for the new shape graph:', 'New shape graph') || '').trim();
    if (!name) return;
    busy = true;
    try {
      const sg = await createShapeGraph({ name, visibility: 'private' });
      const res = await importShapesIntoGraph(sg.id, selectedRefs());
      toastSuccess(`Created "${sg.name}" with ${res.imported} shape${res.imported === 1 ? '' : 's'}`);
      dispatch('created', sg);
      navigate(`/shacl/shapes/${sg.id}`);
    } catch (e) { toastError(e.message); } finally { busy = false; }
  }

  async function addToExisting() {
    if (!selected.size || !addTargetId) { toastError('Pick a shape graph to add into'); return; }
    busy = true;
    try {
      const res = await importShapesIntoGraph(addTargetId, selectedRefs());
      toastSuccess(`Added ${res.imported} shape${res.imported === 1 ? '' : 's'}`);
      navigate(`/shacl/shapes/${addTargetId}`);
    } catch (e) { toastError(e.message); } finally { busy = false; }
  }

  async function registerGraph(g) {
    const name = (prompt('Name for this shape graph:', shortIRI(g.graph)) || '').trim();
    if (!name) return;
    busy = true;
    try {
      const sg = await registerShapeGraph({ graph_iri: g.graph, name, visibility: 'private' });
      toastSuccess(`Registered "${sg.name}"`);
      await reload();
    } catch (e) { toastError(e.message); } finally { busy = false; }
  }
</script>

<div class="catalog">
  <div class="cat-toolbar">
    <div class="search-wrap">
      <Search size={14} />
      <input class="search-input" placeholder="Filter graphs — and shapes within an open graph…" bind:value={search} />
      {#if search}<button class="icon-btn" on:click={() => (search = '')} title="Clear"><X size={13} /></button>{/if}
    </div>
    <div class="kind-seg" role="group" aria-label="Filter by shape type">
      <button class="seg" class:active={kindFilter === 'all'} on:click={() => (kindFilter = 'all')} title="Show both node and property shapes">Both</button>
      <button class="seg" class:active={kindFilter === 'node'} on:click={() => (kindFilter = 'node')} title="Show only node shapes (sh:NodeShape)"><span class="kind kind-node">N</span> Node</button>
      <button class="seg" class:active={kindFilter === 'property'} on:click={() => (kindFilter = 'property')} title="Show only property shapes (sh:PropertyShape)"><span class="kind kind-property">P</span> Property</button>
    </div>
    {#if graphs.length}
      <span class="summary-line">
        {filteredGraphs.length} graph{filteredGraphs.length === 1 ? '' : 's'} ·
        {#if kindFilter === 'node'}{totalNode.toLocaleString()} node shape{totalNode === 1 ? '' : 's'}
        {:else if kindFilter === 'property'}{totalProperty.toLocaleString()} property shape{totalProperty === 1 ? '' : 's'}
        {:else}{(totalNode + totalProperty).toLocaleString()} shapes ({totalNode.toLocaleString()} node · {totalProperty.toLocaleString()} property){/if}
      </span>
    {/if}
  </div>

  {#if selected.size}
    <div class="action-bar">
      <span class="sel-count"><Check size={13} /> {selected.size} selected</span>
      {#if picker}
        <button class="btn btn-sm" on:click={addToTarget} disabled={busy}>
          {#if busy}<Loader2 size={13} class="spin" />{:else}<FolderInput size={13} />{/if} Add to this shape graph
        </button>
      {:else}
        <button class="btn btn-sm" on:click={createFromSelection} disabled={busy}><Plus size={13} /> New shape graph from selection</button>
        <div class="add-existing">
          <Select bind:value={addTargetId} options={[{ value: '', label: 'Add to existing…' }, ...shapeGraphs.map((s) => ({ value: s.id, label: s.name }))]} />
          <button class="btn btn-sm btn-ghost" on:click={addToExisting} disabled={busy || !addTargetId}><FolderInput size={13} /> Add</button>
        </div>
      {/if}
      <button class="btn btn-sm btn-ghost" on:click={() => (selected = new Set())}>Clear</button>
    </div>
  {/if}

  {#if error}<div class="error">{error}</div>{/if}
  {#if loading}
    <div class="placeholder"><Loader2 size={22} class="spin" /> Loading shape sources…</div>
  {:else if filteredGraphs.length === 0}
    <div class="placeholder">
      <Layers size={36} strokeWidth={1.2} />
      <p>{graphs.length === 0 ? 'No SHACL shapes found in any graph yet.' : 'No graphs match your filter.'}</p>
    </div>
  {:else}
    {#each filteredGraphs as g (g.graph)}
      <div class="graph-group">
        <div class="group-head">
          <button class="expander" on:click={() => toggleExpand(g)} title="Show shapes">
            {#if expanded.has(g.graph)}<ChevronDown size={14} />{:else}<ChevronRight size={14} />{/if}
          </button>
          <Layers size={13} class="grp-icon" />
          <code class="grp-iri" title={g.graph} on:click={() => toggleExpand(g)} role="presentation">{shortIRI(g.graph)}</code>
          {#if kindFilter === 'node'}
            <span class="grp-counts">{(g.node_count || 0).toLocaleString()} <span class="dim">node shape{g.node_count === 1 ? '' : 's'}</span></span>
          {:else if kindFilter === 'property'}
            <span class="grp-counts">{(g.property_count || 0).toLocaleString()} <span class="dim">property shape{g.property_count === 1 ? '' : 's'}</span></span>
          {:else}
            <span class="grp-counts">{(g.total || 0).toLocaleString()} <span class="dim">({g.node_count} node{g.property_count ? ` · ${g.property_count} prop` : ''})</span></span>
          {/if}
          {#if g.registered}
            <span class="chip chip-reg"><FileCode size={10} /> {g.shape_graph_name}</span>
            {#if !picker && viewerConfigured()}
              <button class="btn btn-xs btn-ghost" on:click={() => openGraphInViewer(g)} disabled={openingViewer.has(g.graph)} title="Open these shapes in the configured graph viewer">
                {#if openingViewer.has(g.graph)}<Loader2 size={11} class="spin" />{:else}<ExternalLink size={11} />{/if} Viewer
              </button>
            {/if}
          {:else}
            <span class="chip chip-unreg">not in Library</span>
            {#if !picker}
              <button class="btn btn-xs btn-ghost" on:click={() => registerGraph(g)} disabled={busy} title="Adopt this graph as a shape graph"><BookmarkPlus size={11} /> Register</button>
            {/if}
          {/if}
        </div>
        {#if expanded.has(g.graph)}
          {#if loadingGraph.has(g.graph)}
            <div class="grp-loading"><Loader2 size={15} class="spin" /> loading shapes…</div>
          {:else}
            {@const vis = visibleShapes(g.graph)}
            {#if vis.length === 0}
              <div class="grp-loading dim">No shapes match the filter in this graph.</div>
            {:else}
              <div class="grp-tools">
                <button class="link-btn" on:click={() => selectAllVisible(g.graph, !allVisibleSelected(g.graph))}>
                  {allVisibleSelected(g.graph) ? 'Deselect' : 'Select'} all {search.trim() ? 'matching' : ''} ({vis.length})
                </button>
              </div>
              <ul class="shape-list">
                {#each vis as s (s.shape)}
                  <li class="shape-row" class:sel={selected.has(key(g.graph, s.shape))} on:click={() => toggle(g.graph, s.shape)} role="presentation">
                    <span class="box" class:on={selected.has(key(g.graph, s.shape))}>{#if selected.has(key(g.graph, s.shape))}<Check size={11} />{/if}</span>
                    <span class="kind kind-{s.kind}">{s.kind === 'property' ? 'P' : 'N'}</span>
                    <span class="shape-name" title={s.shape}>{s.label || shortIRI(s.shape)}</span>
                    {#each (s.target_classes || []).slice(0, 3) as tc}<span class="chip chip-target"><Database size={9} /> {shortIRI(tc)}</span>{/each}
                    {#if s.path}<span class="chip chip-path">{shortIRI(s.path)}</span>{/if}
                  </li>
                {/each}
              </ul>
            {/if}
          {/if}
        {/if}
      </div>
    {/each}
  {/if}
</div>

<style>
  .catalog { display: flex; flex-direction: column; gap: 0.6rem; }
  .cat-toolbar { display: flex; align-items: center; gap: 0.7rem; flex-wrap: wrap; }
  .search-wrap { flex: 1; min-width: 220px; display: flex; align-items: center; gap: 0.5rem; padding: 0 0.4rem; border: 1px solid var(--line-soft); border-radius: 10px; background: #fff; color: #94a3b8; }
  .search-input { flex: 1; border: none; outline: none; background: transparent; font-size: 0.88rem; color: #1e293b; padding: 0.45rem 0.2rem; }
  .summary-line { font-size: 0.76rem; color: #94a3b8; font-weight: 600; }

  .kind-seg { display: inline-flex; gap: 0.15rem; padding: 0.2rem; background: var(--surface, #fff); border: 1px solid var(--line-soft); border-radius: 9px; flex-shrink: 0; }
  .seg { display: inline-flex; align-items: center; gap: 0.3rem; padding: 0.28rem 0.6rem; border: none; border-radius: 6px; background: transparent; color: #64748b; font-weight: 600; font-size: 0.78rem; cursor: pointer; }
  .seg:hover { background: #f1f5f9; color: #334155; }
  .seg.active { background: #ecfeff; color: #0e7490; }

  /* Push the registration status + per-graph actions to the right edge. */
  .group-head .chip-reg, .group-head .chip-unreg { margin-left: auto; }

  .action-bar { display: flex; align-items: center; gap: 0.5rem; flex-wrap: wrap; padding: 0.5rem 0.7rem; background: #ecfeff; border: 1px solid #7ED6D0; border-radius: 10px; }
  .sel-count { display: inline-flex; align-items: center; gap: 0.3rem; font-weight: 700; font-size: 0.8rem; color: #0e7490; }
  .add-existing { display: inline-flex; align-items: center; gap: 0.3rem; }
  .btn-xs { font-size: 0.7rem; padding: 0.15rem 0.45rem; }

  .error { color: #dc2626; background: #fef2f2; border: 1px solid #fecaca; padding: 0.6rem 0.8rem; border-radius: 10px; font-size: 0.85rem; }
  .placeholder { display: flex; flex-direction: column; align-items: center; gap: 0.6rem; padding: 2.5rem; color: #94a3b8; text-align: center; }
  .placeholder p { margin: 0; }

  .graph-group { border: 1px solid var(--line-soft); border-radius: 11px; overflow: hidden; background: #fff; }
  .group-head { display: flex; align-items: center; gap: 0.5rem; padding: 0.45rem 0.7rem; background: #f8fafc; }
  .expander { background: none; border: none; padding: 0; cursor: pointer; display: grid; place-items: center; color: #64748b; }
  :global(.graph-group .grp-icon) { color: #64748b; flex-shrink: 0; }
  .grp-iri { font-family: 'IBM Plex Mono', monospace; font-size: 0.8rem; color: #334155; font-weight: 600; cursor: pointer; }
  .grp-counts { font-size: 0.76rem; color: #475569; font-weight: 700; margin-left: 0.2rem; }
  .grp-counts .dim { color: #94a3b8; font-weight: 500; }
  .grp-loading { padding: 0.5rem 0.8rem; font-size: 0.8rem; color: #64748b; display: flex; align-items: center; gap: 0.4rem; border-top: 1px solid var(--line-soft); }
  .grp-tools { padding: 0.35rem 0.8rem; border-top: 1px solid var(--line-soft); }
  .link-btn { background: none; border: none; color: #0e7490; font-size: 0.76rem; font-weight: 600; cursor: pointer; padding: 0; }

  .shape-list { list-style: none; margin: 0; padding: 0; max-height: 420px; overflow: auto; }
  .shape-row { display: flex; align-items: center; gap: 0.5rem; padding: 0.4rem 0.7rem; border-top: 1px solid #f1f5f9; cursor: pointer; }
  .shape-row:hover { background: #f8fafc; }
  .shape-row.sel { background: #ecfeff; }
  .box { width: 16px; height: 16px; border: 1.5px solid #cbd5e1; border-radius: 4px; display: grid; place-items: center; color: #fff; flex-shrink: 0; }
  .box.on { background: #0e9bb0; border-color: #0e9bb0; }
  .kind { width: 17px; height: 17px; border-radius: 4px; display: grid; place-items: center; font-size: 0.64rem; font-weight: 800; flex-shrink: 0; }
  .kind-node { background: #e0e7ff; color: #3730a3; }
  .kind-property { background: #dcfce7; color: #166534; }
  .shape-name { font-size: 0.84rem; color: #1e293b; font-weight: 500; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .chip { display: inline-flex; align-items: center; gap: 0.2rem; font-size: 0.66rem; padding: 1px 6px; border-radius: 999px; font-weight: 600; white-space: nowrap; }
  .chip-target { background: #ecfeff; color: #0e7490; font-family: 'IBM Plex Mono', monospace; font-weight: 500; }
  .chip-path { background: #f1f5f9; color: #475569; font-family: 'IBM Plex Mono', monospace; }
  .chip-reg { background: #dbeafe; color: #1d4ed8; }
  .chip-unreg { background: #fef3c7; color: #92400e; }

  :global(:is([data-theme="dark"], .dark)) .search-wrap { background: var(--bg-soft); color: var(--ink-500); }
  :global(:is([data-theme="dark"], .dark)) .search-input { color: var(--ink-900); }
  :global(:is([data-theme="dark"], .dark)) .kind-seg { background: var(--bg-strong); }
  :global(:is([data-theme="dark"], .dark)) .seg { color: var(--ink-500); }
  :global(:is([data-theme="dark"], .dark)) .seg:hover { background: rgba(255,255,255,0.06); color: var(--ink-800); }
  :global(:is([data-theme="dark"], .dark)) .seg.active { background: var(--brand-100); color: var(--brand-700); }
  :global(:is([data-theme="dark"], .dark)) .action-bar { background: var(--brand-100); border-color: var(--brand-300); }
  :global(:is([data-theme="dark"], .dark)) .sel-count { color: var(--brand-700); }
  :global(:is([data-theme="dark"], .dark)) .graph-group { background: var(--bg-strong); }
  :global(:is([data-theme="dark"], .dark)) .group-head { background: var(--bg-soft); }
  :global(:is([data-theme="dark"], .dark)) .grp-iri { color: var(--ink-800); }
  :global(:is([data-theme="dark"], .dark)) .shape-row { border-top-color: rgba(255,255,255,0.05); }
  :global(:is([data-theme="dark"], .dark)) .shape-row:hover { background: rgba(255,255,255,0.05); }
  :global(:is([data-theme="dark"], .dark)) .shape-row.sel { background: var(--brand-100); }
  :global(:is([data-theme="dark"], .dark)) .shape-name { color: var(--ink-900); }
  :global(:is([data-theme="dark"], .dark)) .chip-target { background: var(--brand-100); color: var(--brand-700); }
  :global(:is([data-theme="dark"], .dark)) .chip-path { background: rgba(255,255,255,0.06); color: var(--ink-500); }
</style>
