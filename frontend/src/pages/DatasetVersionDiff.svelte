<script lang="ts">
  // Compare two versions of a dataset, or a version with the live graphs
  // (GET /api/datasets/:id/versions/:ver/diff/:other): per-graph counts, then
  // the change itself as an RDF Patch, readable row by row and downloadable.
  import { onMount } from 'svelte';
  import { t } from 'svelte-i18n';
  import { Download, GitCompare, Loader2 } from 'lucide-svelte';
  import PageHeader from '../components/PageHeader.svelte';
  import Select from '../components/Select.svelte';
  import {
    getDataset, listDatasetVersions, diffDatasetVersions, diffDatasetVersionsPatch, datasetVersionDiffPatchUrl,
    type DatasetVersionDiff, type DatasetVersion,
  } from '../lib/api';
  import { parsePatchRows, splitTerms, type PatchRow } from '../lib/reasoning';
  import { navigate } from '../lib/router/index.js';
  import '../components/reasoning/reasoning.css';

  export let id: string;

  const ROW_PAGE = 200;
  const params = new URLSearchParams(window.location.search);
  let from = params.get('from') ?? '';
  let to = params.get('to') ?? 'live';
  let name = id;
  let versions: DatasetVersion[] = [];
  let diff: DatasetVersionDiff | null = null;
  let loading = false;
  let error = '';
  let rows: PatchRow[] | null = null;
  let rowsLoading = false;
  let rowsError = '';
  let graphFilter = '';
  let opFilter: 'all' | 'A' | 'D' = 'all';
  let limit = ROW_PAGE;

  onMount(async () => {
    try {
      const [ds, vs] = await Promise.all([getDataset(id), listDatasetVersions(id)]);
      name = ds?.name ?? id;
      versions = vs;
      if (!from && vs.length) from = vs[0].version;
    } catch (e) {
      error = (e as Error)?.message || String(e);
    }
    if (from && to) load();
  });

  $: fromOptions = versions.map((v) => ({ value: v.version, label: `v${v.version} · ${v.status}` }));
  $: toOptions = [{ value: 'live', label: $t('pages.versionDiff.live') }, ...fromOptions];
  $: breadcrumbs = [
    { label: $t('pages.apiServices.crumbDatasets'), href: '/datasets' },
    { label: name, href: `/datasets/${id}` },
    { label: $t('pages.versionDiff.title') },
  ];

  async function load() {
    if (!from || !to) return;
    loading = true;
    error = '';
    diff = null;
    rows = null;
    rowsError = '';
    navigate(`/datasets/${id}/versions/diff?from=${encodeURIComponent(from)}&to=${encodeURIComponent(to)}`, { replace: true });
    try {
      diff = await diffDatasetVersions(id, from, to);
    } catch (e) {
      error = (e as Error)?.message || String(e);
    } finally {
      loading = false;
    }
  }

  async function loadRows() {
    rowsLoading = true;
    rowsError = '';
    try {
      rows = parsePatchRows(await diffDatasetVersionsPatch(id, from, to));
      limit = ROW_PAGE;
    } catch (e) {
      rowsError = (e as Error)?.message || String(e);
    } finally {
      rowsLoading = false;
    }
  }

  $: graphs = rows ? Array.from(new Set(rows.map((r) => r.graph).filter((g): g is string => !!g))) : [];
  $: graphOptions = [{ value: '', label: $t('pages.versionDiff.allGraphs') }, ...graphs.map((g) => ({ value: g, label: g.replace(/^<|>$/g, '') }))];
  $: filtered = (rows ?? []).filter((r) =>
    (opFilter === 'all' || r.op === opFilter || (opFilter === 'A' && r.op === 'PA') || (opFilter === 'D' && r.op === 'PD')) &&
    (!graphFilter || r.graph === graphFilter),
  );
</script>

<PageHeader title={$t('pages.versionDiff.title')} icon={GitCompare} {breadcrumbs} />

<div class="card rz-card">
  <div class="rz-grid">
    <div class="rz-field">
      <span class="rz-label">{$t('pages.versionDiff.from')}</span>
      <Select bind:value={from} options={fromOptions} ariaLabel={$t('pages.versionDiff.from')} />
    </div>
    <div class="rz-field">
      <span class="rz-label">{$t('pages.versionDiff.to')}</span>
      <Select bind:value={to} options={toOptions} ariaLabel={$t('pages.versionDiff.to')} />
    </div>
  </div>
  <div class="rz-row">
    <button class="btn btn-sm" on:click={load} disabled={loading || !from || !to || from === to}>
      {#if loading}<Loader2 size={13} class="animate-spin" />{/if}
      {$t('pages.versionDiff.compare')}
    </button>
    {#if !versions.length && !error}<span class="rz-muted">{$t('pages.versionDiff.noVersions')}</span>{/if}
  </div>
  {#if error}<p class="rz-msg rz-msg-bad" role="alert">{error}</p>{/if}
</div>

{#if diff}
  <div class="card rz-card" data-testid="version-diff">
    <div class="rz-head">
      <h3>{$t('pages.versionDiff.summary', { values: { from: `v${diff.from}`, to: diff.to === 'live' ? $t('pages.versionDiff.live') : `v${diff.to}` } })}</h3>
      <a class="btn btn-sm btn-ghost" href={datasetVersionDiffPatchUrl(id, diff.from, diff.to)} download={`${id}-${diff.from}-${diff.to}.rdfp`}>
        <Download size={13} /> {$t('pages.versionDiff.downloadPatch')}
      </a>
    </div>
    <p class="rz-row">
      <span class="rz-badge rz-ok">+{diff.added}</span>
      <span class="rz-badge rz-bad">−{diff.removed}</span>
    </p>
    {#if diff.graphs.length}
      <div class="rz-table-wrap">
        <table class="rz-table">
          <thead><tr><th>{$t('pages.versionDiff.graph')}</th><th class="rz-num">{$t('pages.versionDiff.added')}</th><th class="rz-num">{$t('pages.versionDiff.removed')}</th><th></th></tr></thead>
          <tbody>
            {#each diff.graphs as g}
              <tr>
                <td><code>{g.source_graph}</code></td>
                <td class="rz-num rz-add">+{g.added}</td>
                <td class="rz-num rz-del">−{g.removed}</td>
                <td>
                  {#if g.missing_on_other_side}<span class="rz-badge rz-warn">{$t('pages.versionDiff.missingOnOther')}</span>{/if}
                  {#if g.only_on_other_side}<span class="rz-badge rz-busy">{$t('pages.versionDiff.onlyOnOther')}</span>{/if}
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {:else}
      <p class="rz-muted">{$t('pages.versionDiff.noGraphs')}</p>
    {/if}

    {#if rows === null}
      <div><button class="btn btn-sm btn-ghost" on:click={loadRows} disabled={rowsLoading || diff.added + diff.removed === 0}>
        {#if rowsLoading}<Loader2 size={13} class="animate-spin" />{/if}
        {$t('pages.versionDiff.showChanges')}
      </button></div>
      {#if rowsError}<p class="rz-msg rz-msg-bad">{rowsError}</p>{/if}
    {:else}
      <div class="rz-row">
        <div class="rz-tabs" role="group" aria-label={$t('pages.versionDiff.filter')}>
          {#each ['all', 'A', 'D'] as f}
            <button type="button" class="rz-tab" aria-pressed={opFilter === f} on:click={() => (opFilter = f as typeof opFilter)}>{$t(`pages.versionDiff.op.${f}`)}</button>
          {/each}
        </div>
        {#if graphs.length > 1}
          <Select bind:value={graphFilter} options={graphOptions} ariaLabel={$t('pages.versionDiff.graph')} size="sm" />
        {/if}
        <span class="rz-muted">{$t('pages.versionDiff.rowCount', { values: { shown: Math.min(limit, filtered.length), count: filtered.length } })}</span>
      </div>
      <div class="rz-table-wrap">
        <table class="rz-table">
          <tbody>
            {#each filtered.slice(0, limit) as r}
              {@const terms = splitTerms(r.terms)}
              <tr data-testid="diff-row" class={r.op === 'A' || r.op === 'PA' ? 'rz-add' : 'rz-del'}>
                <td><strong>{r.op === 'A' || r.op === 'PA' ? '+' : '−'}</strong></td>
                {#if r.op === 'PA' || r.op === 'PD'}
                  <td colspan="4"><code>@prefix {r.terms}</code></td>
                {:else}
                  <td><code>{terms[0] ?? ''}</code></td>
                  <td><code>{terms[1] ?? ''}</code></td>
                  <td><code>{terms[2] ?? ''}</code></td>
                  <td>{#if graphs.length > 1}<code>{terms[3] ?? ''}</code>{/if}</td>
                {/if}
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
      {#if filtered.length > limit}
        <button class="btn btn-sm btn-ghost" on:click={() => (limit += ROW_PAGE)}>{$t('pages.versionDiff.more')}</button>
      {/if}
    {/if}
  </div>
{/if}
