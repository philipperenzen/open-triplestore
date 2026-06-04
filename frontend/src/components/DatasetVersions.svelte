<script>
  import { onMount } from 'svelte';
  import { t } from 'svelte-i18n';
  import {
    Plus, Loader2, Download, GitBranch, PlayCircle, CheckCircle, RotateCcw, History, X,
  } from 'lucide-svelte';
  import {
    listDatasetVersions, getDatasetBranches, createDatasetVersion, stageDatasetVersion,
    publishDatasetVersion, deprecateDatasetVersion, restoreDatasetVersion, createDatasetBranch,
    getDatasetVersionDataUrl,
  } from '../lib/api.js';

  export let id;
  export let canWrite = false;
  /** The dataset's live graphs (objects with graph_iri, or bare strings). */
  export let graphs = [];

  let versions = [];
  let branches = [];
  let loading = false;
  let error = '';
  let actionKey = '';

  // New-version form
  let showNew = false;
  let newVersion = '';
  let newNotes = '';
  let newSelected = new Set();
  let snapshotAll = true; // true = snapshot every graph; false = choose a subset
  let creating = false;

  // New-branch form (per version)
  let branchFrom = null;
  let branchName = '';
  let branchBusy = false;

  $: graphIris = (graphs || []).map((g) => (typeof g === 'string' ? g : g.graph_iri));

  // ---- version logic ----
  // Parse a dotted-numeric version (optionally v-prefixed) into numeric parts.
  function parseVer(v) {
    const m = String(v ?? '').trim().replace(/^v/i, '');
    if (!/^\d+(\.\d+)*$/.test(m)) return null;
    return m.split('.').map(Number);
  }
  // -1 / 0 / 1 comparison; numeric where possible, else lexical fallback.
  function cmpVer(a, b) {
    const pa = parseVer(a), pb = parseVer(b);
    if (pa && pb) {
      for (let i = 0; i < Math.max(pa.length, pb.length); i++) {
        const x = pa[i] ?? 0, y = pb[i] ?? 0;
        if (x !== y) return x < y ? -1 : 1;
      }
      return 0;
    }
    const sa = String(a), sb = String(b);
    return sa < sb ? -1 : sa > sb ? 1 : 0;
  }

  // The highest existing version — the floor a new version must exceed.
  $: latestVersion = versions.length
    ? versions.map((v) => v.version).reduce((a, b) => (cmpVer(b, a) > 0 ? b : a))
    : null;

  function bumpFrom(base, kind) {
    const [maj = 0, min = 0, pat = 0] = parseVer(base) ?? [0, 0, 0];
    if (kind === 'major') return `${maj + 1}.0.0`;
    if (kind === 'minor') return `${maj}.${min + 1}.0`;
    return `${maj}.${min}.${pat + 1}`;
  }
  function suggestBump(kind) { newVersion = bumpFrom(latestVersion ?? '0.0.0', kind); }

  $: trimmedVersion = newVersion.trim();
  $: versionExists = versions.some((v) => v.version === trimmedVersion);
  // A new version must be strictly greater than the latest (unless this is the
  // very first version). Identical versions are rejected: published snapshots
  // are immutable, so to change anything you cut a higher version — only a
  // still-unpublished draft can keep being edited under the same number.
  $: versionError = !trimmedVersion
    ? ''
    : versionExists
      ? $t('components.datasetVersions.versionExistsError', { values: { version: trimmedVersion } })
      : latestVersion && cmpVer(trimmedVersion, latestVersion) <= 0
        ? $t('components.datasetVersions.versionTooLowError', { values: { latest: latestVersion } })
        : parseVer(trimmedVersion) === null
          ? $t('components.datasetVersions.versionNumericError')
          : '';
  $: graphsError = !snapshotAll && newSelected.size === 0
    ? $t('components.datasetVersions.selectGraphError')
    : '';
  $: canCreate = !!trimmedVersion && !versionError && !graphsError && !creating;

  async function load() {
    loading = true; error = '';
    try {
      [versions, branches] = await Promise.all([
        listDatasetVersions(id),
        getDatasetBranches(id).catch(() => []),
      ]);
    } catch (e) {
      error = e?.message || String(e);
    }
    loading = false;
  }
  onMount(load);

  function badge(status) {
    const map = {
      published: 'bg-green-100 text-green-700',
      staged: 'bg-blue-100 text-blue-700',
      draft: 'bg-amber-100 text-amber-700',
      deprecated: 'bg-gray-100 text-gray-500',
    };
    return map[status] || 'bg-gray-100 text-gray-500';
  }

  function toggleGraph(iri) {
    const next = new Set(newSelected);
    next.has(iri) ? next.delete(iri) : next.add(iri);
    newSelected = next;
  }

  function openNew() {
    showNew = true;
    // Default to the next patch of the latest version, or 1.0.0 for the first.
    newVersion = latestVersion ? bumpFrom(latestVersion, 'patch') : '1.0.0';
    newNotes = '';
    newSelected = new Set();
    snapshotAll = true;
  }

  async function createVersion() {
    if (!canCreate) return;
    creating = true;
    try {
      await createDatasetVersion(id, {
        version: trimmedVersion,
        notes: newNotes.trim() || undefined,
        graphs: snapshotAll ? [] : Array.from(newSelected),
      });
      showNew = false; newVersion = ''; newNotes = ''; newSelected = new Set();
      await load();
    } catch (e) {
      alert(e?.message || String(e));
    }
    creating = false;
  }

  async function act(ver, action) {
    actionKey = `${ver}:${action}`;
    try {
      if (action === 'stage') await stageDatasetVersion(id, ver);
      else if (action === 'publish') await publishDatasetVersion(id, ver);
      else if (action === 'deprecate') await deprecateDatasetVersion(id, ver);
      else if (action === 'restore') {
        if (!confirm($t('components.datasetVersions.restoreConfirm', { values: { version: ver } }))) { actionKey = ''; return; }
        await restoreDatasetVersion(id, ver);
      }
      await load();
    } catch (e) {
      alert(e?.message || String(e));
    }
    actionKey = '';
  }

  async function makeBranch() {
    if (!branchName.trim() || !branchFrom) return;
    branchBusy = true;
    try {
      await createDatasetBranch(id, branchName.trim(), branchFrom);
      branchFrom = null; branchName = '';
      await load();
    } catch (e) {
      alert(e?.message || String(e));
    }
    branchBusy = false;
  }
</script>

<section class="dsv">
  <div class="dsv-head">
    <h3><History size={16} /> {$t('components.datasetVersions.heading', { values: { count: versions.length } })}</h3>
    {#if canWrite}
      <button class="btn-sm" on:click={() => (showNew ? (showNew = false) : openNew())}>
        <Plus size={13} /> {$t('components.datasetVersions.newVersion')}
      </button>
    {/if}
  </div>

  {#if error}<p class="err">{error}</p>{/if}

  {#if showNew}
    <div class="new-form">
      <div class="field">
        <span class="field-label">{$t('components.datasetVersions.versionLabel')}</span>
        <div class="ver-input-row">
          <input class="inp ver-inp" class:invalid={!!versionError} placeholder="1.0.0" bind:value={newVersion} aria-invalid={!!versionError} />
          <div class="bump-group">
            <button type="button" class="bump" title={$t('components.datasetVersions.majorReleaseTitle', { values: { version: bumpFrom(latestVersion ?? '0.0.0', 'major') } })} on:click={() => suggestBump('major')}>{$t('components.datasetVersions.major')}</button>
            <button type="button" class="bump" title={$t('components.datasetVersions.minorReleaseTitle', { values: { version: bumpFrom(latestVersion ?? '0.0.0', 'minor') } })} on:click={() => suggestBump('minor')}>{$t('components.datasetVersions.minor')}</button>
            <button type="button" class="bump" title={$t('components.datasetVersions.patchReleaseTitle', { values: { version: bumpFrom(latestVersion ?? '0.0.0', 'patch') } })} on:click={() => suggestBump('patch')}>{$t('components.datasetVersions.patch')}</button>
          </div>
        </div>
        {#if versionError}
          <span class="field-err">{versionError}</span>
        {:else}
          <span class="field-hint">
            {#if latestVersion}{$t('components.datasetVersions.mustBeHigherHintPrefix')} <code>{latestVersion}</code>.{:else}{$t('components.datasetVersions.firstVersionHint')}{/if}
          </span>
        {/if}
      </div>

      <div class="field">
        <span class="field-label">{$t('components.datasetVersions.notesLabel')} <span class="opt">{$t('components.datasetVersions.optional')}</span></span>
        <input class="inp" placeholder={$t('components.datasetVersions.notesPlaceholder')} bind:value={newNotes} />
      </div>

      {#if graphIris.length > 0}
        <div class="field">
          <span class="field-label">{$t('components.datasetVersions.graphsToSnapshot')}</span>
          <div class="seg">
            <button type="button" class="seg-btn" class:active={snapshotAll} on:click={() => (snapshotAll = true)}>
              {$t('components.datasetVersions.allGraphs')} <span class="seg-count">{graphIris.length}</span>
            </button>
            <button type="button" class="seg-btn" class:active={!snapshotAll} on:click={() => (snapshotAll = false)}>
              {$t('components.datasetVersions.chooseGraphs')}{#if !snapshotAll && newSelected.size > 0}<span class="seg-count">{newSelected.size}</span>{/if}
            </button>
          </div>
          {#if !snapshotAll}
            <div class="graph-list">
              <div class="graph-list-head">
                <span class="hint">{$t('components.datasetVersions.selectedCount', { values: { selected: newSelected.size, total: graphIris.length } })}</span>
                <div class="graph-list-actions">
                  <button type="button" class="link-btn" on:click={() => (newSelected = new Set(graphIris))}>{$t('components.datasetVersions.allShort')}</button>
                  <button type="button" class="link-btn" on:click={() => (newSelected = new Set())}>{$t('components.datasetVersions.noneShort')}</button>
                </div>
              </div>
              {#each graphIris as g (g)}
                <label class="graph-item" class:sel={newSelected.has(g)}>
                  <input type="checkbox" checked={newSelected.has(g)} on:change={() => toggleGraph(g)} />
                  <span class="graph-name">{g.split('/').pop() || g}</span>
                  <code class="graph-iri" title={g}>{g}</code>
                </label>
              {/each}
            </div>
            {#if graphsError}<span class="field-err">{graphsError}</span>{/if}
          {/if}
        </div>
      {/if}

      <div class="row end">
        <button class="btn-sm ghost" on:click={() => (showNew = false)}>{$t('system.cancel')}</button>
        <button class="btn-sm primary" disabled={!canCreate} on:click={createVersion}>
          {#if creating}<Loader2 size={13} class="animate-spin" />{/if} {$t('components.datasetVersions.createDraft')}
        </button>
      </div>
    </div>
  {/if}

  {#if loading}
    <p class="muted"><Loader2 size={14} class="animate-spin" /> {$t('system.loading')}</p>
  {:else if versions.length === 0}
    <p class="muted">{$t('components.datasetVersions.emptyState')}</p>
  {:else}
    <div class="ver-list">
      {#each versions as v (v.version)}
        <div class="ver-row">
          <div class="ver-main">
            <span class="ver-name">{v.version}</span>
            <span class="badge {badge(v.status)}">{v.status}</span>
            {#if v.branch}<span class="branch-tag"><GitBranch size={11} /> {v.branch}</span>{/if}
            <span class="muted sm" title={v.snapshot_graphs.join('\n')}>{$t('components.datasetVersions.graphCount', { values: { count: v.snapshot_graphs.length } })}</span>
            {#if v.notes}<span class="muted sm notes" title={v.notes}>· {v.notes}</span>{/if}
          </div>
          <div class="ver-actions">
            <a class="btn-sm" href={getDatasetVersionDataUrl(id, v.version, 'trig')} download="{id}-{v.version}.trig" title={$t('components.datasetVersions.downloadSnapshotTitle')}>
              <Download size={13} /> {$t('components.datasetVersions.download')}
            </a>
            {#if canWrite}
              {#if v.status === 'draft'}
                <button class="btn-sm" disabled={actionKey === `${v.version}:stage`} on:click={() => act(v.version, 'stage')}>
                  {#if actionKey === `${v.version}:stage`}<Loader2 size={13} class="animate-spin" />{:else}<PlayCircle size={13} />{/if} {$t('components.datasetVersions.stage')}
                </button>
              {/if}
              {#if v.status === 'draft' || v.status === 'staged'}
                <button class="btn-sm publish" disabled={actionKey === `${v.version}:publish`} on:click={() => act(v.version, 'publish')}>
                  {#if actionKey === `${v.version}:publish`}<Loader2 size={13} class="animate-spin" />{:else}<CheckCircle size={13} />{/if} {$t('components.datasetVersions.publish')}
                </button>
              {/if}
              {#if v.status === 'published'}
                <button class="btn-sm danger" disabled={actionKey === `${v.version}:deprecate`} on:click={() => act(v.version, 'deprecate')}>{$t('components.datasetVersions.deprecate')}</button>
              {/if}
              <button class="btn-sm" disabled={actionKey === `${v.version}:restore`} on:click={() => act(v.version, 'restore')} title={$t('components.datasetVersions.restoreTitle')}>
                {#if actionKey === `${v.version}:restore`}<Loader2 size={13} class="animate-spin" />{:else}<RotateCcw size={13} />{/if} {$t('components.datasetVersions.restore')}
              </button>
              <button class="btn-sm" on:click={() => { branchFrom = v.version; branchName = ''; }} title={$t('components.datasetVersions.branchTitle')}>
                <GitBranch size={13} /> {$t('components.datasetVersions.branch')}
              </button>
            {/if}
          </div>
        </div>
        {#if branchFrom === v.version}
          <div class="branch-form">
            <GitBranch size={13} />
            <input class="inp flex1" placeholder={$t('components.datasetVersions.branchNamePlaceholder')} bind:value={branchName} />
            <button class="btn-sm primary" disabled={branchBusy || !branchName.trim()} on:click={makeBranch}>
              {#if branchBusy}<Loader2 size={13} class="animate-spin" />{/if} {$t('components.datasetVersions.createBranch')}
            </button>
            <button class="btn-sm ghost" on:click={() => (branchFrom = null)}><X size={13} /></button>
          </div>
        {/if}
      {/each}
    </div>
  {/if}

  {#if branches.length > 0}
    <div class="branches">
      <span class="hint"><GitBranch size={12} /> {$t('components.datasetVersions.branchesLabel', { values: { count: branches.length } })}</span>
      {#each branches as b (b.branch)}
        <span class="branch-chip" title={$t('components.datasetVersions.branchChipTitle', { values: { tip: b.tip_version, ahead: b.ahead, behind: b.behind } })}>
          {b.branch} <span class="muted sm">{b.tip_version}</span>
        </span>
      {/each}
    </div>
  {/if}
</section>

<style>
  .dsv { margin-top: 1.5rem; }
  .dsv-head { display: flex; align-items: center; justify-content: space-between; margin-bottom: 0.5rem; }
  .dsv-head h3 { margin: 0; display: flex; align-items: center; gap: 0.4rem; font-size: 1rem; }
  .err { color: #ef4444; font-size: 0.85rem; }
  .muted { color: var(--ink-400, #94a3b8); font-size: 0.85rem; display: inline-flex; align-items: center; gap: 0.3rem; }
  .muted.sm { font-size: 0.75rem; }
  .notes { max-width: 22rem; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .ver-list { display: flex; flex-direction: column; gap: 0.4rem; }
  .ver-row { display: flex; align-items: center; justify-content: space-between; gap: 0.5rem; padding: 0.5rem 0.75rem; background: var(--bg-soft, #f8fafc); border: 1px solid var(--border, #e2e8f0); border-radius: 0.5rem; flex-wrap: wrap; }
  .ver-main { display: flex; align-items: center; gap: 0.5rem; flex-wrap: wrap; }
  .ver-name { font-family: var(--mono, monospace); font-weight: 700; color: var(--ink-900, #0f172a); }
  .badge { font-size: 0.7rem; padding: 0.1rem 0.5rem; border-radius: 999px; font-weight: 600; }
  .branch-tag { display: inline-flex; align-items: center; gap: 0.2rem; font-size: 0.72rem; padding: 0.05rem 0.4rem; border-radius: 0.3rem; background: var(--bg, #fff); border: 1px solid var(--border, #e2e8f0); color: var(--ink-500); }
  .ver-actions { display: flex; align-items: center; gap: 0.3rem; flex-wrap: wrap; }
  .btn-sm { display: inline-flex; align-items: center; gap: 0.25rem; font-size: 0.75rem; padding: 0.2rem 0.5rem; border-radius: 0.35rem; border: 1px solid var(--border, #e2e8f0); background: var(--bg, #fff); color: var(--ink-600, #475569); cursor: pointer; text-decoration: none; }
  .btn-sm:hover:not(:disabled) { background: var(--bg-soft, #f1f5f9); }
  .btn-sm:disabled { opacity: 0.5; cursor: default; }
  .btn-sm.primary { background: var(--brand-600, #4f46e5); color: #fff; border-color: var(--brand-600, #4f46e5); }
  .btn-sm.publish { color: #15803d; border-color: #bbf7d0; }
  .btn-sm.danger { color: #ef4444; border-color: #fecaca; }
  .btn-sm.ghost { border-color: transparent; }
  .new-form, .branch-form { display: flex; flex-direction: column; gap: 0.7rem; padding: 0.85rem 0.9rem; background: var(--bg-soft, #f8fafc); border: 1px dashed var(--border, #e2e8f0); border-radius: 0.6rem; margin-bottom: 0.6rem; }
  .branch-form { flex-direction: row; align-items: center; gap: 0.5rem; margin: 0.4rem 0 0.6rem; }
  .row { display: flex; gap: 0.5rem; align-items: center; }
  .row.end { justify-content: flex-end; }
  .inp { width: 100%; box-sizing: border-box; font-size: 0.8rem; padding: 0.4rem 0.55rem; border: 1px solid var(--border, #e2e8f0); border-radius: 0.4rem; background: var(--bg, #fff); }
  .inp:focus { outline: none; border-color: var(--brand-500, #1f9897); box-shadow: 0 0 0 3px rgba(31, 152, 151, 0.14); }
  .inp.invalid { border-color: #ef4444; }
  .inp.invalid:focus { box-shadow: 0 0 0 3px rgba(239, 68, 68, 0.14); }
  .flex1 { flex: 1; }
  .hint { font-size: 0.72rem; color: var(--ink-400, #94a3b8); display: inline-flex; align-items: center; gap: 0.3rem; }

  /* Form fields */
  .field { display: flex; flex-direction: column; gap: 0.3rem; }
  .field-label { font-size: 0.72rem; font-weight: 600; text-transform: uppercase; letter-spacing: 0.04em; color: var(--ink-500, #64748b); }
  .field-label .opt { font-weight: 400; text-transform: none; letter-spacing: 0; color: var(--ink-400, #94a3b8); }
  .field-hint { font-size: 0.72rem; color: var(--ink-400, #94a3b8); }
  .field-hint code { font-family: var(--mono, monospace); background: var(--bg, #fff); padding: 0 0.25rem; border-radius: 0.25rem; border: 1px solid var(--border, #e2e8f0); }
  .field-err { font-size: 0.72rem; color: #dc2626; font-weight: 500; }

  /* Version input + semver bump buttons */
  .ver-input-row { display: flex; gap: 0.4rem; align-items: center; flex-wrap: wrap; }
  .ver-inp { width: auto; flex: 0 0 9rem; font-family: var(--mono, monospace); font-weight: 600; }
  .bump-group { display: inline-flex; border: 1px solid var(--border, #e2e8f0); border-radius: 0.4rem; overflow: hidden; }
  .bump { font-size: 0.72rem; padding: 0.32rem 0.55rem; border: none; border-left: 1px solid var(--border, #e2e8f0); background: var(--bg, #fff); color: var(--ink-600, #475569); cursor: pointer; }
  .bump:first-child { border-left: none; }
  .bump:hover { background: var(--bg-soft, #f1f5f9); color: var(--ink-900, #0f172a); }

  /* All / Choose segmented control */
  .seg { display: inline-flex; align-self: flex-start; border: 1px solid var(--border, #e2e8f0); border-radius: 0.45rem; overflow: hidden; background: var(--bg, #fff); }
  .seg-btn { display: inline-flex; align-items: center; gap: 0.35rem; font-size: 0.75rem; padding: 0.35rem 0.7rem; border: none; border-left: 1px solid var(--border, #e2e8f0); background: transparent; color: var(--ink-600, #475569); cursor: pointer; }
  .seg-btn:first-child { border-left: none; }
  .seg-btn.active { background: var(--brand-500, #1f9897); color: #fff; }
  .seg-count { font-size: 0.66rem; font-weight: 600; padding: 0.02rem 0.35rem; border-radius: 999px; background: rgba(0, 0, 0, 0.08); }
  .seg-btn.active .seg-count { background: rgba(255, 255, 255, 0.25); }

  /* Graph subset list */
  .graph-list { border: 1px solid var(--border, #e2e8f0); border-radius: 0.5rem; background: var(--bg, #fff); max-height: 13rem; overflow-y: auto; }
  .graph-list-head { position: sticky; top: 0; display: flex; align-items: center; justify-content: space-between; padding: 0.35rem 0.6rem; background: var(--bg-soft, #f8fafc); border-bottom: 1px solid var(--border, #e2e8f0); }
  .graph-list-actions { display: inline-flex; gap: 0.5rem; }
  .link-btn { border: none; background: none; padding: 0; font-size: 0.72rem; font-weight: 600; color: var(--brand-600, #167c80); cursor: pointer; }
  .link-btn:hover { text-decoration: underline; }
  .graph-item { display: flex; align-items: center; gap: 0.5rem; padding: 0.32rem 0.6rem; font-size: 0.78rem; cursor: pointer; border-bottom: 1px solid var(--line-soft, #f1f5f9); }
  .graph-item:last-child { border-bottom: none; }
  .graph-item:hover { background: var(--bg-soft, #f8fafc); }
  .graph-item.sel { background: rgba(31, 152, 151, 0.07); }
  .graph-name { font-weight: 600; color: var(--ink-900, #0f172a); flex-shrink: 0; }
  .graph-iri { font-family: var(--mono, monospace); font-size: 0.7rem; color: var(--ink-400, #94a3b8); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .branches { margin-top: 0.6rem; display: flex; flex-wrap: wrap; gap: 0.4rem; align-items: center; }
  .branch-chip { font-size: 0.72rem; padding: 0.1rem 0.5rem; border-radius: 999px; background: var(--brand-50, #eef2ff); color: var(--brand-700, #4338ca); border: 1px solid var(--brand-100, #e0e7ff); }

  /* ---- Dark mode overrides (most surfaces flip via global --bg/--border) ---- */
  :global(:is([data-theme="dark"], .dark)) .err,
  :global(:is([data-theme="dark"], .dark)) .field-err { color: #fca5a5; }
  :global(:is([data-theme="dark"], .dark)) .btn-sm.publish { color: #6ee7b7; border-color: rgba(16,185,129,0.4); }
  :global(:is([data-theme="dark"], .dark)) .btn-sm.danger { color: #fca5a5; border-color: rgba(239,68,68,0.4); }
  :global(:is([data-theme="dark"], .dark)) .branch-chip { background: var(--brand-100); border-color: var(--brand-200); }
  :global(:is([data-theme="dark"], .dark)) .seg-count { background: rgba(255,255,255,0.12); }
</style>
