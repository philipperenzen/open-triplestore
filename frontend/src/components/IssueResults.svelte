<script>
  import { shortenIRI, downloadFile } from '../lib/rdf-utils.js';
  import { Download, ClipboardCopy, Search, Filter, ChevronRight, ChevronDown, Info } from 'lucide-svelte';
  import { toastSuccess, toastError } from '../lib/toast.ts';
  import { t } from 'svelte-i18n';
  import Select from './Select.svelte';
  import { copyToClipboard } from '../lib/clipboard.js';

  export let results = [];
  export let datasetName = '';

  const PAGE = 200; // items rendered per group before "show more"

  let filterSeverity = 'all';
  let filterShape = '';
  let filterPath = '';
  let search = '';
  let groupBy = 'none'; // none | focus | shape | constraint
  let collapsed = {};
  let groupLimits = {};

  function sevKey(s) { return (s || 'violation').toLowerCase(); }

  function matchesText(r, q) {
    if (!q) return true;
    const hay = `${r.message || ''} ${r.focus_node || ''} ${r.source_shape || ''} ${r.path || ''} ${r.value || ''}`.toLowerCase();
    return hay.includes(q.toLowerCase());
  }

  $: severityCounts = results.reduce((acc, r) => {
    const k = sevKey(r.severity);
    acc[k] = (acc[k] || 0) + 1;
    return acc;
  }, {});

  // Facet options (distinct shapes / paths) computed over the full result set.
  $: shapeFacets = (() => {
    const m = new Map();
    for (const r of results) {
      const k = r.source_shape || '';
      if (k) m.set(k, (m.get(k) || 0) + 1);
    }
    return [...m.entries()].sort((a, b) => b[1] - a[1]);
  })();
  $: pathFacets = (() => {
    const m = new Map();
    for (const r of results) {
      const k = r.path || '';
      if (k) m.set(k, (m.get(k) || 0) + 1);
    }
    return [...m.entries()].sort((a, b) => b[1] - a[1]);
  })();

  $: filtered = results.filter(r => {
    if (filterSeverity !== 'all' && sevKey(r.severity) !== filterSeverity) return false;
    if (filterShape && r.source_shape !== filterShape) return false;
    if (filterPath && (r.path || '') !== filterPath) return false;
    if (!matchesText(r, search)) return false;
    return true;
  });

  // `groupBy` is passed in explicitly so Svelte tracks it as a dependency of
  // `groups` — referencing it only inside a helper would not trigger recompute.
  function computeGroups(rows, gb) {
    const keyOf = (r) => {
      if (gb === 'focus') return r.focus_node || '(no focus node)';
      if (gb === 'shape') return r.source_shape || '(no shape)';
      if (gb === 'constraint') return r.source_constraint || '(no constraint)';
      return '__all__';
    };
    const m = new Map();
    for (const r of rows) {
      const k = keyOf(r);
      if (!m.has(k)) m.set(k, []);
      m.get(k).push(r);
    }
    return [...m.entries()]
      .map(([key, items]) => ({ key, items }))
      .sort((a, b) => b.items.length - a.items.length);
  }

  $: groups = computeGroups(filtered, groupBy);

  $: hasActiveFilter = filterSeverity !== 'all' || filterShape || filterPath || search;

  function clearFilters() {
    filterSeverity = 'all'; filterShape = ''; filterPath = ''; search = '';
  }
  function toggleGroup(key) { collapsed = { ...collapsed, [key]: !collapsed[key] }; }
  function showMore(key) { groupLimits = { ...groupLimits, [key]: (groupLimits[key] || PAGE) + PAGE }; }

  function rowsFor(scope) {
    return scope === 'filtered' ? filtered : results;
  }
  function exportReport(format, scope) {
    const rows = rowsFor(scope);
    const base = (datasetName || 'validation').replace(/[^a-z0-9_-]+/gi, '_');
    if (format === 'json') {
      downloadFile(JSON.stringify({ results: rows, results_count: rows.length }, null, 2), `${base}-report.json`, 'application/json');
    } else {
      const header = 'severity,focus_node,path,value,source_shape,source_constraint,message';
      const csv = rows.map(r =>
        [r.severity, r.focus_node, r.path || '', r.value || '', r.source_shape, r.source_constraint || '', r.message]
          .map(v => `"${(v ?? '').toString().replace(/"/g, '""')}"`).join(',')
      );
      downloadFile([header, ...csv].join('\n'), `${base}-report.csv`, 'text/csv');
    }
  }
  async function copySparql(scope) {
    const rows = rowsFor(scope);
    const nodes = [...new Set(rows.map(r => r.focus_node))].filter(Boolean).slice(0, 50);
    if (nodes.length === 0) { toastError($t('components.issueResults.noFocusNodes')); return; }
    const values = nodes.map(n => `<${n}>`).join(' ');
    const q = `SELECT ?s ?p ?o WHERE {\n  VALUES ?s { ${values} }\n  ?s ?p ?o\n}`;
    if (await copyToClipboard(q)) toastSuccess($t('components.issueResults.sparqlCopied'));
  }

  let exportScope = 'all';
</script>

<div class="issue-results">
  <div class="toolbar">
    <div class="sev-row">
      <button class="sev-chip" class:active={filterSeverity === 'all'} on:click={() => filterSeverity = 'all'}>
        {$t('components.issueResults.all')} <span class="sev-count">{results.length}</span>
      </button>
      {#each ['violation','warning','info'] as sev}
        {#if severityCounts[sev]}
          <button class="sev-chip sev-{sev}" class:active={filterSeverity === sev} on:click={() => filterSeverity = sev}>
            <span class="sev-dot sev-dot-{sev}"></span>{$t(`components.issueResults.severity.${sev}`)}<span class="sev-count">{severityCounts[sev]}</span>
          </button>
        {/if}
      {/each}
    </div>

    <div class="export-row">
      <Select size="sm" class="facet-w" bind:value={exportScope} title={$t('components.issueResults.exportScope')} options={[
        { value: 'all', label: $t('components.issueResults.allIssues') },
        { value: 'filtered', label: $t('components.issueResults.filteredScope', { values: { count: filtered.length } }) },
      ]} />
      <button class="btn btn-sm btn-ghost" on:click={() => exportReport('csv', exportScope)} title={$t('components.issueResults.exportCsv')}><Download size={13} /> CSV</button>
      <button class="btn btn-sm btn-ghost" on:click={() => exportReport('json', exportScope)} title={$t('components.issueResults.exportJson')}><Download size={13} /> JSON</button>
      <button class="btn btn-sm btn-ghost" on:click={() => copySparql(exportScope)} title={$t('components.issueResults.copySparql')}><ClipboardCopy size={13} /> SPARQL</button>
    </div>
  </div>

  <div class="filters">
    <div class="input-icon"><Search size={13} /><input placeholder={$t('components.issueResults.searchPlaceholder')} bind:value={search} /></div>
    <Select size="sm" class="facet-w" bind:value={filterShape} title={$t('components.issueResults.filterByShape')} options={[
      { value: '', label: $t('components.issueResults.allShapes') },
      ...shapeFacets.map(([iri, n]) => ({ value: iri, label: `${shortenIRI(iri)} (${n})` })),
    ]} />
    {#if pathFacets.length}
      <Select size="sm" class="facet-w" bind:value={filterPath} title={$t('components.issueResults.filterByPath')} options={[
        { value: '', label: $t('components.issueResults.allPaths') },
        ...pathFacets.map(([iri, n]) => ({ value: iri, label: `${shortenIRI(iri)} (${n})` })),
      ]} />
    {/if}
    <div class="group-by">
      <Filter size={13} />
      <Select size="sm" bind:value={groupBy} title={$t('components.issueResults.groupIssuesBy')} options={[
        { value: 'none', label: $t('components.issueResults.noGrouping') },
        { value: 'focus', label: $t('components.issueResults.groupByFocus') },
        { value: 'shape', label: $t('components.issueResults.groupByShape') },
        { value: 'constraint', label: $t('components.issueResults.groupByConstraint') },
      ]} />
    </div>
    <span class="count-note">{$t('components.issueResults.showingCount', { values: { shown: filtered.length, total: results.length } })}</span>
    {#if hasActiveFilter}<button class="btn btn-sm btn-ghost" on:click={clearFilters}>{$t('system.clear')}</button>{/if}
  </div>

  <div class="issue-scroll">
    {#if filtered.length === 0}
      <div class="no-results"><Info size={18} /> {$t('components.issueResults.noMatch')}</div>
    {:else}
      {#each groups as g (g.key)}
        {#if groupBy !== 'none'}
          <button class="group-head" on:click={() => toggleGroup(g.key)}>
            {#if collapsed[g.key]}<ChevronRight size={14} />{:else}<ChevronDown size={14} />{/if}
            <span class="group-label" title={g.key}>{g.key === '__all__' ? $t('components.issueResults.allIssues') : shortenIRI(g.key)}</span>
            <span class="group-count">{g.items.length}</span>
          </button>
        {/if}
        {#if groupBy === 'none' || !collapsed[g.key]}
          {#each g.items.slice(0, groupLimits[g.key] || PAGE) as r}
            {@const s = sevKey(r.severity)}
            <div class="issue" class:issue-violation={s === 'violation'} class:issue-warning={s === 'warning'} class:issue-info={s === 'info'}>
              <span class="issue-bar"></span>
              <div class="issue-main">
                <div class="issue-top">
                  <span class="sev-badge sev-{s}">{r.severity}</span>
                  <a class="focus-link" href={`/resource?iri=${encodeURIComponent(r.focus_node)}`} title={r.focus_node}>{shortenIRI(r.focus_node)}</a>
                  {#if r.path}<span class="issue-sep">·</span><span class="issue-path" title={r.path}>{shortenIRI(r.path)}</span>{/if}
                </div>
                <div class="issue-message">{r.message}</div>
                <div class="issue-foot">
                  <span class="muted">{$t('components.issueResults.shapeLabel')}</span><span class="mono" title={r.source_shape}>{shortenIRI(r.source_shape)}</span>
                  {#if r.value}<span class="issue-sep">·</span><span class="muted">{$t('components.issueResults.valueLabel')}</span><span class="mono" title={r.value}>{shortenIRI(r.value)}</span>{/if}
                </div>
              </div>
            </div>
          {/each}
          {#if g.items.length > (groupLimits[g.key] || PAGE)}
            <button class="show-more" on:click={() => showMore(g.key)}>
              {$t('components.issueResults.showMore', { values: { count: Math.min(PAGE, g.items.length - (groupLimits[g.key] || PAGE)), total: g.items.length } })}
            </button>
          {/if}
        {/if}
      {/each}
    {/if}
  </div>
</div>

<style>
  .issue-results { display: flex; flex-direction: column; min-height: 0; }
  .toolbar { display: flex; align-items: center; justify-content: space-between; gap: 0.5rem; flex-wrap: wrap; padding: 0.75rem 1.1rem 0.25rem; }
  .sev-row { display: flex; gap: 0.35rem; flex-wrap: wrap; }
  .sev-chip { display: inline-flex; align-items: center; gap: 0.35rem; padding: 4px 10px; border: 1px solid var(--line-soft); border-radius: 999px; background: #fff; cursor: pointer; font-size: 0.78rem; color: #475569; text-transform: capitalize; }
  .sev-chip:hover { background: #f8fafc; }
  .sev-chip.active { background: #1e293b; color: #fff; border-color: #1e293b; }
  .sev-chip.sev-violation.active { background: #b91c1c; border-color: #b91c1c; }
  .sev-chip.sev-warning.active { background: #b45309; border-color: #b45309; }
  .sev-chip.sev-info.active { background: #1d4ed8; border-color: #1d4ed8; }
  .sev-count { font-weight: 700; font-size: 0.72rem; opacity: 0.8; }
  .sev-dot { width: 7px; height: 7px; border-radius: 50%; display: inline-block; }
  .sev-dot-violation { background: #dc2626; }
  .sev-dot-warning { background: #f59e0b; }
  .sev-dot-info { background: #3b82f6; }

  .export-row { display: flex; align-items: center; gap: 0.3rem; }
  :global(.facet-w) { max-width: 16rem; }

  .filters { display: flex; gap: 0.5rem; padding: 0.5rem 1.1rem 0.75rem; align-items: center; flex-wrap: wrap; }
  .input-icon { display: flex; align-items: center; gap: 0.4rem; padding: 0.35rem 0.6rem; border: 1px solid var(--line-soft); border-radius: 8px; background: #fff; flex: 1; min-width: 220px; color: #64748b; }
  .input-icon input { border: none; outline: none; background: transparent; font-size: 0.82rem; color: #1e293b; flex: 1; min-width: 0; }
  .group-by { display: inline-flex; align-items: center; gap: 0.35rem; color: #64748b; }
  .count-note { font-size: 0.78rem; color: #94a3b8; margin-left: auto; }

  .issue-scroll { display: flex; flex-direction: column; gap: 0.45rem; padding: 0.25rem 1.1rem 1.1rem; max-height: 60vh; overflow: auto; }

  .group-head { display: flex; align-items: center; gap: 0.4rem; padding: 0.4rem 0.5rem; background: #f8fafc; border: 1px solid var(--line-soft); border-radius: 8px; cursor: pointer; text-align: left; width: 100%; color: #334155; margin-top: 0.25rem; }
  .group-head:hover { background: #f1f5f9; }
  .group-label { font-family: 'IBM Plex Mono', monospace; font-size: 0.8rem; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; flex: 1; }
  .group-count { font-weight: 700; font-size: 0.72rem; background: #e2e8f0; color: #475569; padding: 1px 8px; border-radius: 999px; }

  .issue { display: flex; gap: 0.6rem; padding: 0.7rem 0.85rem 0.7rem 0.5rem; background: #fff; border: 1px solid var(--line-soft); border-radius: 10px; }
  .issue:hover { border-color: #cbd5e1; box-shadow: var(--shadow-sm); }
  .issue-bar { width: 3px; border-radius: 3px; background: #cbd5e1; align-self: stretch; flex-shrink: 0; }
  .issue-violation .issue-bar { background: #dc2626; }
  .issue-warning .issue-bar { background: #f59e0b; }
  .issue-info .issue-bar { background: #3b82f6; }
  .issue-main { min-width: 0; flex: 1; display: flex; flex-direction: column; gap: 0.25rem; }
  .issue-top { display: flex; align-items: center; gap: 0.5rem; flex-wrap: wrap; }
  .sev-badge { font-size: 0.65rem; padding: 1px 6px; border-radius: 4px; font-weight: 700; text-transform: uppercase; letter-spacing: 0.05em; }
  .sev-badge.sev-violation { background: #fee2e2; color: #991b1b; }
  .sev-badge.sev-warning { background: #fef3c7; color: #92400e; }
  .sev-badge.sev-info { background: #dbeafe; color: #1e40af; }
  .focus-link { font-weight: 600; color: #2F7A8C; text-decoration: none; font-family: 'IBM Plex Mono', monospace; font-size: 0.82rem; }
  .focus-link:hover { text-decoration: underline; }
  .issue-sep { color: #cbd5e1; }
  .issue-path { font-family: 'IBM Plex Mono', monospace; color: #7c3aed; font-size: 0.8rem; }
  .issue-message { color: #334155; font-size: 0.9rem; line-height: 1.4; }
  .issue-foot { display: flex; align-items: center; gap: 0.35rem; flex-wrap: wrap; font-size: 0.75rem; }
  .muted { color: #94a3b8; }
  .mono { font-family: 'IBM Plex Mono', monospace; color: #64748b; }

  .show-more { align-self: flex-start; font-size: 0.78rem; color: #2F7A8C; background: transparent; border: 1px dashed var(--line-soft); border-radius: 8px; padding: 0.35rem 0.7rem; cursor: pointer; }
  .show-more:hover { background: #f0fdfa; border-color: #7ED6D0; }
  .no-results { display: flex; align-items: center; gap: 0.5rem; justify-content: center; padding: 2rem; color: #94a3b8; font-size: 0.88rem; }

  /* ---- Dark mode overrides (scoped rules out-specify global theme.css) ---- */
  :global(:is([data-theme="dark"], .dark)) .sev-chip { background: var(--bg-soft); color: var(--ink-700); }
  :global(:is([data-theme="dark"], .dark)) .sev-chip:hover { background: rgba(255,255,255,0.06); }
  :global(:is([data-theme="dark"], .dark)) .sev-chip.active { background: var(--brand-400); color: #fff; border-color: var(--brand-400); }
  :global(:is([data-theme="dark"], .dark)) .scope-select,
  :global(:is([data-theme="dark"], .dark)) .facet { background: var(--bg-soft); color: var(--ink-800); }
  :global(:is([data-theme="dark"], .dark)) .input-icon { background: var(--bg-soft); color: var(--ink-500); }
  :global(:is([data-theme="dark"], .dark)) .input-icon input { color: var(--ink-900); }
  :global(:is([data-theme="dark"], .dark)) .group-by { color: var(--ink-500); }
  :global(:is([data-theme="dark"], .dark)) .group-head { background: var(--bg-soft); color: var(--ink-800); }
  :global(:is([data-theme="dark"], .dark)) .group-head:hover { background: rgba(255,255,255,0.06); }
  :global(:is([data-theme="dark"], .dark)) .group-count { background: var(--line-strong); color: var(--ink-600); }
  :global(:is([data-theme="dark"], .dark)) .issue { background: var(--bg-soft); }
  :global(:is([data-theme="dark"], .dark)) .issue:hover { border-color: var(--line-strong); }
  :global(:is([data-theme="dark"], .dark)) .issue-bar { background: var(--line-strong); }
  :global(:is([data-theme="dark"], .dark)) .sev-badge.sev-violation { background: rgba(239,68,68,0.18); color: #fca5a5; }
  :global(:is([data-theme="dark"], .dark)) .sev-badge.sev-warning { background: rgba(245,158,11,0.18); color: #fcd34d; }
  :global(:is([data-theme="dark"], .dark)) .sev-badge.sev-info { background: rgba(59,130,246,0.2); color: #93c5fd; }
  :global(:is([data-theme="dark"], .dark)) .focus-link,
  :global(:is([data-theme="dark"], .dark)) .show-more { color: var(--brand-700); }
  :global(:is([data-theme="dark"], .dark)) .show-more:hover { background: var(--brand-100); border-color: var(--brand-300); }
  :global(:is([data-theme="dark"], .dark)) .issue-sep { color: var(--ink-400); }
  :global(:is([data-theme="dark"], .dark)) .issue-path { color: #c4b5fd; }
  :global(:is([data-theme="dark"], .dark)) .issue-message { color: var(--ink-800); }
  :global(:is([data-theme="dark"], .dark)) .muted,
  :global(:is([data-theme="dark"], .dark)) .mono { color: var(--ink-500); }
</style>
