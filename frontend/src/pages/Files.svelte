<script>
  // /files — the platform-wide file manager. Without a dataset it lists the
  // datasets you can browse (each dataset owns one file library); /files/:id
  // opens that dataset's FileBrowser full-page, deep-linkable via ?path=.
  import { onMount } from 'svelte';
  import { t } from 'svelte-i18n';
  import { Link } from '../lib/router/index.js';
  import { listDatasets, getDataset } from '../lib/api.js';
  import { isAuthenticated } from '../lib/stores.js';
  import { normalizePath } from '../lib/files';
  import FileBrowser from '../components/files/FileBrowser.svelte';
  import Avatar from '../components/Avatar.svelte';
  import {
    FolderOpen, Database, ArrowLeft, Search, Loader2, Lock, Globe, Users,
  } from 'lucide-svelte';

  export let datasetId = null;

  // ── Dataset chooser (no :id) ──────────────────────────────────────────────
  let datasets = [];
  let listLoading = true;
  let listError = '';
  let search = '';

  // ── Single-dataset browser ────────────────────────────────────────────────
  let dataset = null;
  let datasetLoading = false;
  let datasetError = '';
  let initialPath = '';

  onMount(() => {
    initialPath = normalizePath(new URLSearchParams(window.location.search).get('path') || '');
    if (!datasetId) fetchList();
  });

  $: if (datasetId) fetchDataset(datasetId);

  let fetchedFor = null;
  async function fetchDataset(id) {
    if (fetchedFor === id) return;
    fetchedFor = id;
    datasetLoading = true;
    datasetError = '';
    dataset = null;
    try {
      dataset = await getDataset(id);
    } catch (e) {
      datasetError = e?.message || $t('pages.files.datasetLoadFailed');
    } finally {
      datasetLoading = false;
    }
  }

  async function fetchList() {
    listLoading = true;
    listError = '';
    try {
      datasets = await listDatasets();
    } catch (e) {
      listError = e?.message || $t('pages.files.listLoadFailed');
    } finally {
      listLoading = false;
    }
  }

  function onPathChange(e) {
    // Keep the folder in the URL so reload/share lands in the same place.
    const p = e.detail;
    const url = new URL(window.location.href);
    if (p) url.searchParams.set('path', p); else url.searchParams.delete('path');
    history.replaceState(history.state, '', url);
  }

  $: filtered = datasets.filter((d) =>
    !search.trim() || (d.name || '').toLowerCase().includes(search.trim().toLowerCase())
  );
  $: canWrite = (dataset?.can_write ?? false) && $isAuthenticated;
</script>

{#if datasetId}
  <!-- ── One dataset's file manager ─────────────────────────────────────── -->
  <div class="files-page">
    <div class="files-head">
      <Link to="/files" class="files-back"><ArrowLeft size={14} /> {$t('pages.files.allLibraries')}</Link>
      {#if dataset}
        <span class="files-head-dataset">
          <Avatar kind="dataset" id={dataset.id} name={dataset.name} hasImage={!!dataset.image_key} size={26} />
          <Link to={`/datasets/${dataset.id}`} class="files-dataset-link">{dataset.name}</Link>
          <span class="files-vis files-vis-{dataset.visibility}">
            {#if dataset.visibility === 'public'}<Globe size={11} />{:else if dataset.visibility === 'members'}<Users size={11} />{:else}<Lock size={11} />{/if}
            {dataset.visibility}
          </span>
        </span>
      {/if}
    </div>

    {#if datasetLoading}
      <div class="files-state"><Loader2 size={20} class="animate-spin" /> {$t('system.loading')}</div>
    {:else if datasetError}
      <div class="files-state files-error">{datasetError}</div>
    {:else if dataset}
      <FileBrowser
        datasetId={dataset.id}
        {canWrite}
        {initialPath}
        datasetVisibility={dataset.visibility}
        on:pathchange={onPathChange}
      />
    {/if}
  </div>
{:else}
  <!-- ── Library chooser ────────────────────────────────────────────────── -->
  <div class="files-page">
    <div class="files-intro card">
      <div class="files-intro-icon"><FolderOpen size={22} /></div>
      <div>
        <h3>{$t('pages.files.chooseTitle')}</h3>
        <p>{$t('pages.files.chooseHint')}</p>
      </div>
      <div class="files-search">
        <Search size={14} />
        <input
          type="search"
          placeholder={$t('pages.files.searchDatasets')}
          bind:value={search}
          aria-label={$t('pages.files.searchDatasets')}
        />
      </div>
    </div>

    {#if listLoading}
      <div class="files-state"><Loader2 size={20} class="animate-spin" /> {$t('system.loading')}</div>
    {:else if listError}
      <div class="files-state files-error">{listError}</div>
    {:else if !filtered.length}
      <div class="files-state">
        {search ? $t('pages.files.noMatches') : $t('pages.files.noDatasets')}
      </div>
    {:else}
      <div class="files-ds-grid">
        {#each filtered as ds (ds.id)}
          <Link to={`/files/${ds.id}`} class="files-ds-card">
            <Avatar kind="dataset" id={ds.id} name={ds.name} hasImage={!!ds.image_key} size={38} />
            <span class="files-ds-meta">
              <span class="files-ds-name">{ds.name}</span>
              <span class="files-ds-sub">
                <Database size={11} />
                {ds.visibility}
                {#if ds.description} · {ds.description}{/if}
              </span>
            </span>
            <FolderOpen size={16} class="files-ds-open" />
          </Link>
        {/each}
      </div>
    {/if}
  </div>
{/if}

<style>
  .files-page { display: flex; flex-direction: column; gap: 0.9rem; }

  .files-head {
    display: flex; align-items: center; gap: 0.9rem; flex-wrap: wrap;
  }
  :global(.files-back) {
    display: inline-flex; align-items: center; gap: 0.3rem;
    font-size: 0.8rem; font-weight: 600; text-decoration: none;
    color: var(--ink-500, #64748b);
    padding: 0.35rem 0.6rem; border-radius: 9px;
    border: 1px solid var(--line-soft, #e2e8f0);
    background: var(--bg, #fff);
    transition: all 0.12s;
  }
  :global(.files-back:hover) { color: var(--ink-800, #1e293b); border-color: var(--brand-200, #bfdbfe); }
  .files-head-dataset { display: inline-flex; align-items: center; gap: 0.5rem; min-width: 0; }
  :global(.files-dataset-link) {
    font-weight: 700; font-size: 1rem; color: var(--ink-900, #0f172a);
    text-decoration: none;
  }
  :global(.files-dataset-link:hover) { text-decoration: underline; }
  .files-vis {
    display: inline-flex; align-items: center; gap: 0.25rem;
    font-size: 0.68rem; font-weight: 700;
    padding: 0.14rem 0.5rem; border-radius: 999px;
    background: var(--bg-soft, #f1f5f9); color: var(--ink-500, #64748b);
    text-transform: capitalize;
  }
  .files-vis-public { background: #dcfce7; color: #15803d; }

  .files-state {
    display: flex; align-items: center; justify-content: center; gap: 0.5rem;
    min-height: 140px; color: var(--ink-400, #94a3b8); font-size: 0.85rem;
    text-align: center; padding: 1.5rem;
  }
  .files-error { color: #b91c1c; }

  .files-intro {
    display: flex; align-items: center; gap: 0.9rem; flex-wrap: wrap;
  }
  .files-intro h3 { margin: 0; font-size: 0.98rem; }
  .files-intro p { margin: 0.15rem 0 0; font-size: 0.8rem; color: var(--ink-500, #64748b); }
  .files-intro-icon {
    width: 44px; height: 44px; border-radius: 12px; flex-shrink: 0;
    display: grid; place-items: center;
    background: var(--brand-50, #eff6ff); color: var(--brand-600, #2563eb);
  }
  .files-search {
    margin-left: auto;
    display: flex; align-items: center; gap: 0.4rem;
    border: 1px solid var(--line-soft, #e2e8f0);
    border-radius: 10px; padding: 0.4rem 0.6rem;
    color: var(--ink-400, #94a3b8); background: var(--bg, #fff);
  }
  .files-search input {
    border: none; outline: none; background: transparent;
    font-size: 0.82rem; width: 180px; color: var(--ink-900, #0f172a);
  }
  .files-search:focus-within { border-color: var(--brand-300, #93c5fd); }

  .files-ds-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(270px, 1fr));
    gap: 0.7rem;
  }
  :global(.files-ds-card) {
    display: flex; align-items: center; gap: 0.7rem;
    padding: 0.8rem 0.9rem;
    border: 1px solid var(--line-soft, #e8edf3);
    border-radius: 14px;
    background: var(--bg, #fff);
    text-decoration: none;
    transition: border-color 0.12s, box-shadow 0.12s;
    min-width: 0;
  }
  :global(.files-ds-card:hover) {
    border-color: var(--brand-200, #bfdbfe);
    box-shadow: 0 4px 14px rgba(15, 23, 42, 0.07);
  }
  .files-ds-meta { display: flex; flex-direction: column; gap: 0.1rem; min-width: 0; flex: 1; }
  .files-ds-name {
    font-weight: 700; font-size: 0.88rem; color: var(--ink-900, #0f172a);
    overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
  }
  .files-ds-sub {
    display: flex; align-items: center; gap: 0.3rem;
    font-size: 0.72rem; color: var(--ink-400, #94a3b8);
    overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
    text-transform: capitalize;
  }
  .files-page :global(.files-ds-open) { color: var(--ink-300, #cbd5e1); flex-shrink: 0; }
  :global(.files-ds-card:hover .files-ds-open) { color: var(--brand-500, #3b82f6); }

  :global(:is([data-theme="dark"], .dark) .files-back),
  :global(:is([data-theme="dark"], .dark)) .files-search,
  :global(:is([data-theme="dark"], .dark) .files-ds-card) {
    background: rgba(255, 255, 255, 0.04);
    border-color: var(--line-strong);
  }
  :global(:is([data-theme="dark"], .dark)) .files-vis { background: rgba(255,255,255,0.07); }
  :global(:is([data-theme="dark"], .dark)) .files-vis-public { background: rgba(34,197,94,0.14); color: #86efac; }
  :global(:is([data-theme="dark"], .dark)) .files-intro-icon { background: rgba(59,130,246,0.15); color: #93c5fd; }
  :global(:is([data-theme="dark"], .dark)) .files-error { color: #fca5a5; }
</style>
