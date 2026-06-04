<script>
  import { onMount } from 'svelte';
  import { t } from 'svelte-i18n';
  import { GitCommit, Loader2, User } from 'lucide-svelte';
  import {
    getDataModelCommits, getVocabularyCommits, getDatasetCommits, getShapeGraphCommits,
  } from '../lib/api.js';
  import Avatar from './Avatar.svelte';
  import Select from './Select.svelte';

  // 'data-model' | 'vocabulary' | 'dataset' | 'shape-graph'
  export let kind = 'data-model';
  export let id;
  /** Optional list of branch names to populate the filter dropdown. */
  export let branches = [];

  let commits = [];
  let loading = false;
  let error = '';
  let branchFilter = '';

  // Shape graphs have a lifecycle but no branches, so they ignore the branch arg.
  const branchless = kind === 'dataset' || kind === 'shape-graph';

  function loadFn(branch) {
    if (kind === 'vocabulary') return getVocabularyCommits(id, branch);
    if (kind === 'dataset') return getDatasetCommits(id);
    if (kind === 'shape-graph') return getShapeGraphCommits(id);
    return getDataModelCommits(id, branch);
  }

  onMount(loadCommits);

  async function loadCommits() {
    loading = true; error = '';
    try { commits = (await loadFn(branchFilter || undefined)) || []; }
    catch (e) { error = e.message; }
    loading = false;
  }

  function onBranchChange() { loadCommits(); }

  function authorName(c) {
    return c.actor_display_name || c.actor_username
      || (c.actor_iri ? c.actor_iri.split('/').pop() : 'unknown');
  }
  function authorId(c) {
    return c.actor_iri ? c.actor_iri.split('/').pop() : '';
  }

  function relTime(iso) {
    if (!iso) return '';
    const then = new Date(iso).getTime();
    if (Number.isNaN(then)) return iso;
    const secs = Math.round((Date.now() - then) / 1000);
    if (secs < 60) return $t('components.commitHistory.justNow');
    const mins = Math.round(secs / 60);
    if (mins < 60) return $t('components.commitHistory.minutesAgo', { values: { count: mins } });
    const hrs = Math.round(mins / 60);
    if (hrs < 24) return $t('components.commitHistory.hoursAgo', { values: { count: hrs } });
    const days = Math.round(hrs / 24);
    if (days < 30) return $t('components.commitHistory.daysAgo', { values: { count: days } });
    return new Date(iso).toLocaleDateString();
  }
</script>

<div class="commit-card">
  <div class="commit-header">
    <GitCommit size={15} class="text-[var(--brand-500)]" />
    <span class="commit-label">{$t('components.commitHistory.commitsCount', { values: { count: commits.length } })}</span>
    {#if !branchless && branches.length}
      <Select
        size="sm"
        bind:value={branchFilter}
        on:change={onBranchChange}
        options={[{ value: '', label: $t('components.commitHistory.allBranches') }, ...branches.map(b => ({ value: b.branch, label: b.branch }))]}
      />
    {/if}
  </div>

  {#if error}
    <p class="commit-error">{error}</p>
  {/if}

  <div class="commit-body">
    {#if loading}
      <span class="commit-muted"><Loader2 size={12} class="animate-spin" /> {$t('system.loading')}</span>
    {:else if commits.length === 0}
      <span class="commit-muted">{$t('components.commitHistory.noCommits')}</span>
    {:else}
      {#each commits as c (c.commit_id)}
        <div class="commit-row">
          {#if authorId(c)}
            <Avatar kind="user" id={authorId(c)} name={authorName(c)} size={22} />
          {:else}
            <span class="commit-anon"><User size={14} /></span>
          {/if}
          <div class="commit-main">
            <div class="commit-msg">{c.message}</div>
            <div class="commit-meta">
              <span class="commit-author">{authorName(c)}</span>
              <span class="commit-dot">·</span>
              <span title={c.created_at}>{relTime(c.created_at)}</span>
              {#if c.branch}
                <span class="commit-badge commit-branch">{c.branch}</span>
              {/if}
              {#if c.version}
                <span class="commit-badge commit-version">v{c.version}</span>
              {/if}
              {#if c.added || c.removed}
                <span class="commit-counts" title={$t('components.commitHistory.addedRemoved')}>
                  +{c.added} −{c.removed}
                </span>
              {/if}
            </div>
          </div>
        </div>
      {/each}
    {/if}
  </div>
</div>

<style>
  .commit-card { border: 1px solid var(--line-soft, #e5e7eb); border-radius: 8px; padding: 0.75rem; margin-top: 1rem; }
  .commit-header { display: flex; align-items: center; gap: 0.5rem; margin-bottom: 0.5rem; }
  .commit-label { font-weight: 600; font-size: 0.85rem; flex: 1; }
  .commit-body { display: flex; flex-direction: column; gap: 0.6rem; }
  .commit-row { display: flex; align-items: flex-start; gap: 0.5rem; }
  .commit-anon { display: inline-flex; align-items: center; justify-content: center; width: 22px; height: 22px; border-radius: 50%; background: var(--line-soft, #e5e7eb); color: var(--ink-500, #6b7280); flex-shrink: 0; }
  .commit-main { flex: 1; min-width: 0; }
  .commit-msg { font-size: 0.8rem; white-space: pre-wrap; word-break: break-word; }
  .commit-meta { display: flex; align-items: center; flex-wrap: wrap; gap: 0.35rem; font-size: 0.7rem; color: var(--ink-500, #6b7280); margin-top: 0.15rem; }
  .commit-author { font-weight: 600; color: var(--ink-700, #374151); }
  .commit-dot { color: var(--ink-400, #9ca3af); }
  .commit-badge { font-size: 0.65rem; padding: 0.05rem 0.35rem; border-radius: 4px; }
  .commit-branch { background: #dbeafe; color: #1d4ed8; }
  .commit-version { background: #f3f4f6; color: #374151; font-family: monospace; }
  .commit-counts { font-family: monospace; color: var(--ink-400, #9ca3af); }
  .commit-muted { color: var(--ink-400, #9ca3af); font-size: 0.75rem; display: inline-flex; gap: 0.3rem; align-items: center; }
  .commit-error { color: #dc2626; font-size: 0.75rem; margin-bottom: 0.4rem; }
</style>
