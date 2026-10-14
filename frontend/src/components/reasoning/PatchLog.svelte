<script lang="ts">
  // A dataset's RDF Patch log (GET /api/datasets/:id/log): every entry, a
  // patch appended to the log or a version cut, with its author and graphs;
  // each patch viewable and downloadable, plus version 0 (the dataset the
  // log starts from) and the latest patch.
  import { t } from 'svelte-i18n';
  import { Download, Eye, History, Loader2, RefreshCw } from 'lucide-svelte';
  import {
    getDatasetPatchLog, getDatasetLogPatch, datasetLogPatchUrl, datasetLogInitUrl, datasetLogCurrentUrl,
    type PatchLog, type PatchLogEntry,
  } from '../../lib/api';
  import './reasoning.css';

  export let datasetId: string;
  /** Bump to reload (a patch was just appended). */
  export let refreshKey = 0;
  export let pageSize = 50;
  export let log: PatchLog | null = null;

  let entries: PatchLogEntry[] = [];
  let loading = true;
  let error = '';
  let more = false;
  let viewing: number | null = null;
  let viewText = '';
  let viewError = '';

  async function load() {
    loading = true;
    try {
      log = await getDatasetPatchLog(datasetId, { limit: pageSize });
      entries = log.entries;
      more = entries.length >= pageSize;
      error = '';
    } catch (e) {
      error = (e as Error)?.message || String(e);
    } finally {
      loading = false;
    }
  }

  async function loadMore() {
    const last = entries[entries.length - 1];
    if (!last) return;
    try {
      const next = await getDatasetPatchLog(datasetId, { after: last.version, limit: pageSize });
      entries = [...entries, ...next.entries];
      more = next.entries.length >= pageSize;
    } catch (e) {
      error = (e as Error)?.message || String(e);
    }
  }

  async function view(v: number) {
    if (viewing === v) { viewing = null; return; }
    viewing = v;
    viewText = '';
    viewError = '';
    try {
      viewText = await getDatasetLogPatch(datasetId, v);
    } catch (e) {
      viewError = (e as Error)?.message || String(e);
    }
  }

  $: refreshKey, datasetId, load();

  function fmt(ts?: string) {
    if (!ts) return '—';
    const d = new Date(ts);
    return Number.isNaN(d.getTime()) ? ts : d.toLocaleString();
  }
</script>

<div class="card rz-card" id="log">
  <div class="rz-head">
    <div class="rz-head-left"><History size={15} /><h3>{$t('components.patch.logHeading')}</h3></div>
    <div class="rz-row">
      {#if entries.length}
        <a class="btn btn-sm btn-ghost" href={datasetLogInitUrl(datasetId)} download={`${datasetId}-log-0.trig`}><Download size={13} /> {$t('components.patch.downloadInit')}</a>
        <a class="btn btn-sm btn-ghost" href={datasetLogCurrentUrl(datasetId)} download={`${datasetId}-latest.rdfp`}><Download size={13} /> {$t('components.patch.downloadLatest')}</a>
      {/if}
      <button class="btn btn-sm btn-ghost" on:click={load} aria-label={$t('components.patch.reloadLog')}><RefreshCw size={13} /></button>
    </div>
  </div>
  <p class="rz-intro">{$t('components.patch.logIntro')}</p>
  {#if loading && !entries.length}
    <p class="rz-muted"><Loader2 size={12} class="animate-spin" /> {$t('system.loading')}</p>
  {:else if error}
    <p class="rz-msg rz-msg-bad">{error}</p>
  {:else if !entries.length}
    <p class="rz-muted">{$t('components.patch.logEmpty')}</p>
  {:else}
    <div class="rz-table-wrap">
      <table class="rz-table">
        <thead>
          <tr>
            <th class="rz-num">#</th>
            <th>{$t('components.patch.kind')}</th>
            <th>{$t('components.patch.author')}</th>
            <th>{$t('components.patch.graphs')}</th>
            <th>{$t('components.patch.created')}</th>
            <th></th>
          </tr>
        </thead>
        <tbody>
          {#each entries as e (e.version)}
            <tr data-testid="log-entry">
              <td class="rz-num">{e.version}</td>
              {#if e.withheld}
                <td colspan="4" class="rz-muted">{$t('components.patch.withheld')}</td>
              {:else}
                <td>
                  <span class="rz-badge">{$t(`components.patch.kinds.${e.kind ?? 'patch'}`)}</span>
                  {#if e.dataset_version}<code>v{e.dataset_version}</code>{/if}
                </td>
                <td>{e.author ?? '—'}</td>
                <td>{#each e.graphs ?? [] as g}<div><code>{g}</code></div>{/each}</td>
                <td>{fmt(e.created_at)}</td>
              {/if}
              <td>
                {#if !e.withheld}
                  <div class="rz-row">
                    <button class="tbl-btn" on:click={() => view(e.version)} aria-label={$t('components.patch.viewN', { values: { n: e.version } })}><Eye size={13} /></button>
                    <a class="tbl-btn" href={datasetLogPatchUrl(datasetId, e.version)} download={`${datasetId}-${e.version}.rdfp`} aria-label={$t('components.patch.downloadN', { values: { n: e.version } })}><Download size={13} /></a>
                  </div>
                {/if}
              </td>
            </tr>
            {#if viewing === e.version}
              <tr><td colspan="6">
                {#if viewError}<p class="rz-msg rz-msg-bad">{viewError}</p>
                {:else if !viewText}<p class="rz-muted"><Loader2 size={12} class="animate-spin" /></p>
                {:else}<pre class="rz-pre" data-testid="log-patch-text">{viewText}</pre>{/if}
              </td></tr>
            {/if}
          {/each}
        </tbody>
      </table>
    </div>
    {#if more}<button class="btn btn-sm btn-ghost" on:click={loadMore}>{$t('components.patch.loadMore')}</button>{/if}
  {/if}
</div>
