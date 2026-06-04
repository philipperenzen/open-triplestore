<script>
  import { onMount, createEventDispatcher } from 'svelte';
  import { t } from 'svelte-i18n';
  import { GitBranch, GitMerge, Plus, Loader2 } from 'lucide-svelte';
  import {
    getDataModelBranches, createDataModelBranch,
    getVocabularyBranches, createVocabularyBranch,
  } from '../lib/api.js';
  import MergeDialog from './MergeDialog.svelte';
  import Select from './Select.svelte';

  // 'data-model' | 'vocabulary'
  export let kind = 'data-model';
  export let id;
  export let versions = [];
  export let canWrite = false;

  const dispatch = createEventDispatcher();
  const listFn = kind === 'vocabulary' ? getVocabularyBranches : getDataModelBranches;
  const createFn = kind === 'vocabulary' ? createVocabularyBranch : createDataModelBranch;

  let branches = [];
  let loading = false;
  let error = '';
  let showForm = false;
  let creating = false;
  let branchName = '';
  let fromVersion = '';

  $: forkable = (versions || []).filter(v => v.status === 'published' || v.status === 'staged');
  $: if (!fromVersion && forkable.length) fromVersion = forkable[0].version;
  $: mergeInto = (versions || []).find(v => v.status === 'published')?.version
    || (forkable[0] && forkable[0].version);

  let mergeFrom = null; // branch tip version being merged

  onMount(loadBranches);

  async function loadBranches() {
    loading = true; error = '';
    try { branches = (await listFn(id)) || []; }
    catch (e) { error = e.message; }
    loading = false;
  }

  async function create() {
    if (!branchName.trim() || !fromVersion) return;
    creating = true; error = '';
    try {
      await createFn(id, branchName.trim(), fromVersion, '');
      branchName = '';
      showForm = false;
      await loadBranches();
      dispatch('created');
    } catch (e) { error = e.message; }
    creating = false;
  }

  function statusClass(s) {
    return {
      published: 'bg-emerald-100 text-emerald-700',
      staged: 'bg-amber-100 text-amber-700',
      draft: 'bg-blue-100 text-blue-700',
      deprecated: 'bg-gray-100 text-gray-500 line-through',
    }[s] || 'bg-gray-100 text-gray-600';
  }
</script>

<div class="branch-card">
  <div class="branch-header">
    <GitBranch size={15} class="text-[var(--brand-500)]" />
    <span class="branch-label">{$t('components.branchPanel.branchesCount', { values: { count: branches.length } })}</span>
    {#if canWrite && forkable.length}
      <button class="branch-new-btn" on:click={() => (showForm = !showForm)}>
        <Plus size={12} /> {$t('components.branchPanel.newBranch')}
      </button>
    {/if}
  </div>

  {#if error}
    <p class="branch-error">{error}</p>
  {/if}

  {#if showForm}
    <div class="branch-form">
      <input class="branch-input" placeholder={$t('components.branchPanel.branchNamePlaceholder')} bind:value={branchName} />
      <Select
        size="sm"
        bind:value={fromVersion}
        options={forkable.map(v => ({ value: v.version, label: $t('components.branchPanel.fromVersion', { values: { version: v.version, status: v.status } }) }))}
      />
      <button class="branch-create-btn" on:click={create} disabled={creating || !branchName.trim() || !fromVersion}>
        {#if creating}<Loader2 size={12} class="animate-spin" />{:else}<Plus size={12} />{/if}
        {$t('system.create')}
      </button>
    </div>
  {/if}

  <div class="branch-body">
    {#if loading}
      <span class="branch-muted"><Loader2 size={12} class="animate-spin" /> {$t('system.loading')}</span>
    {:else if branches.length === 0}
      <span class="branch-muted">{$t('components.branchPanel.noBranches')}</span>
    {:else}
      {#each branches as b}
        <div class="branch-row">
          <GitBranch size={12} class="text-[var(--ink-400)]" />
          <span class="branch-name">{b.branch}</span>
          <span class="branch-tip">{b.tip_version}</span>
          <span class="branch-status {statusClass(b.status)}">{b.status}</span>
          {#if b.ahead > 0 || b.behind > 0}
            <span class="branch-counts" title={$t('components.branchPanel.aheadBehindTitle')}>↑{b.ahead} ↓{b.behind}</span>
          {/if}
          {#if canWrite && mergeInto && b.branch !== 'main' && b.tip_version !== mergeInto}
            <button class="branch-merge-btn" on:click={() => (mergeFrom = b.tip_version)} title={$t('components.branchPanel.mergeTitle', { values: { branch: b.branch, target: mergeInto } })}>
              <GitMerge size={11} /> {$t('components.branchPanel.merge')}
            </button>
          {/if}
        </div>
      {/each}
    {/if}
  </div>
</div>

{#if mergeFrom && mergeInto}
  <MergeDialog
    {kind} {id} from={mergeFrom} into={mergeInto}
    on:close={() => (mergeFrom = null)}
    on:merged={() => { mergeFrom = null; loadBranches(); dispatch('created'); }}
  />
{/if}

<style>
  .branch-card { border: 1px solid var(--line-soft, #e5e7eb); border-radius: 8px; padding: 0.75rem; margin-top: 1rem; }
  .branch-header { display: flex; align-items: center; gap: 0.5rem; margin-bottom: 0.5rem; }
  .branch-label { font-weight: 600; font-size: 0.85rem; flex: 1; }
  .branch-new-btn, .branch-create-btn, .branch-merge-btn { display: inline-flex; align-items: center; gap: 0.25rem; font-size: 0.7rem; padding: 0.2rem 0.5rem; border: 1px solid var(--line-soft, #e5e7eb); border-radius: 6px; background: transparent; cursor: pointer; }
  .branch-merge-btn { margin-left: auto; }
  .branch-create-btn:disabled { opacity: 0.5; cursor: not-allowed; }
  .branch-form { display: flex; gap: 0.4rem; margin-bottom: 0.5rem; flex-wrap: wrap; }
  .branch-input { font-size: 0.75rem; padding: 0.25rem 0.4rem; border: 1px solid var(--line-soft, #e5e7eb); border-radius: 6px; }
  .branch-body { display: flex; flex-direction: column; gap: 0.3rem; }
  .branch-row { display: flex; align-items: center; gap: 0.4rem; font-size: 0.75rem; }
  .branch-name { font-weight: 600; }
  .branch-tip { font-family: monospace; color: var(--ink-500, #6b7280); }
  .branch-status { font-size: 0.65rem; padding: 0.05rem 0.35rem; border-radius: 4px; }
  .branch-counts { color: var(--ink-400, #9ca3af); font-size: 0.7rem; }
  .branch-muted { color: var(--ink-400, #9ca3af); font-size: 0.75rem; display: inline-flex; gap: 0.3rem; align-items: center; }
  .branch-error { color: #dc2626; font-size: 0.75rem; margin-bottom: 0.4rem; }
</style>
