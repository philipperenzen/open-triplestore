<script>
  import { onMount, createEventDispatcher } from 'svelte';
  import { t } from 'svelte-i18n';
  import { GitMerge, Loader2, X, CheckCircle } from 'lucide-svelte';
  import {
    previewDataModelMerge, mergeDataModel,
  } from '../lib/api.js';

  export let id;
  export let from;
  export let into;

  const dispatch = createEventDispatcher();
  const previewFn = previewDataModelMerge;
  const applyFn = mergeDataModel;

  let preview = null;
  let loading = true;
  let applying = false;
  let error = '';
  let choices = {}; // "subject|predicate" -> 'ours'|'theirs'

  const ckey = (c) => `${c.subject}|${c.predicate}`;
  const shorten = (s) => {
    if (!s || s.startsWith('"')) return s;
    const h = s.lastIndexOf('#'); if (h > 0) return '…' + s.slice(h);
    const sl = s.lastIndexOf('/'); if (sl > 0 && sl < s.length - 1) return '…/' + s.slice(sl + 1);
    return s;
  };

  onMount(async () => {
    try {
      preview = await previewFn(id, from, into);
      for (const c of preview.conflicts) choices[ckey(c)] = 'ours';
      choices = choices;
    } catch (e) { error = e.message; }
    loading = false;
  });

  async function apply() {
    applying = true; error = '';
    try {
      const resolutions = (preview?.conflicts || []).map(c => ({
        subject: c.subject, predicate: c.predicate, choice: choices[ckey(c)] || 'ours',
      }));
      await applyFn(id, { from, into, resolutions });
      dispatch('merged');
    } catch (e) { error = e.message; applying = false; }
  }
</script>

<div class="overlay" role="button" tabindex="-1" aria-label={$t('components.mergeDialog.closeDialog')} on:click={() => dispatch('close')} on:keydown={(e) => { if (e.key === 'Enter' || e.key === ' ' || e.key === 'Escape') dispatch('close'); }}></div>
<div class="dialog">
  <div class="dlg-header">
    <GitMerge size={16} class="text-[var(--brand-500)]" />
    <span class="dlg-title">{$t('components.mergeDialog.title', { values: { from, into } })}</span>
    <button class="dlg-x" on:click={() => dispatch('close')}><X size={16} /></button>
  </div>

  <div class="dlg-body">
    {#if loading}
      <span class="muted"><Loader2 size={14} class="animate-spin" /> {$t('components.mergeDialog.computing')}</span>
    {:else if error}
      <p class="err">{error}</p>
    {:else if preview}
      <div class="summary">
        {#if preview.clean}
          <span class="clean"><CheckCircle size={14} /> {$t('components.mergeDialog.cleanMerge')}</span>
        {:else}
          <span class="conf">{preview.conflicts.length === 1 ? $t('components.mergeDialog.conflictCountOne', { values: { count: preview.conflicts.length } }) : $t('components.mergeDialog.conflictCountOther', { values: { count: preview.conflicts.length } })}</span>
        {/if}
        <span class="muted">{$t('components.mergeDialog.base', { values: { version: preview.base_version || $t('components.mergeDialog.none') } })}</span>
        <span class="add">+{preview.auto_added}</span>
        <span class="rem">−{preview.auto_removed}</span>
      </div>

      {#each preview.conflicts as c}
        <div class="conflict">
          <div class="conflict-key" title={`${c.subject} ${c.predicate}`}>{shorten(c.subject)} · {shorten(c.predicate)}</div>
          <div class="conflict-sides">
            <button class="side {choices[ckey(c)] === 'ours' ? 'sel' : ''}" on:click={() => { choices[ckey(c)] = 'ours'; choices = choices; }}>
              <div class="side-label">{$t('components.mergeDialog.ours', { values: { from } })}</div>
              {#if c.ours.length}{#each c.ours as o}<div class="mono">{shorten(o)}</div>{/each}{:else}<div class="muted">{$t('components.mergeDialog.removed')}</div>{/if}
            </button>
            <button class="side {choices[ckey(c)] === 'theirs' ? 'sel' : ''}" on:click={() => { choices[ckey(c)] = 'theirs'; choices = choices; }}>
              <div class="side-label">{$t('components.mergeDialog.theirs', { values: { into } })}</div>
              {#if c.theirs.length}{#each c.theirs as o}<div class="mono">{shorten(o)}</div>{/each}{:else}<div class="muted">{$t('components.mergeDialog.removed')}</div>{/if}
            </button>
          </div>
        </div>
      {/each}
    {/if}
  </div>

  <div class="dlg-footer">
    <button class="btn-cancel" on:click={() => dispatch('close')}>{$t('system.cancel')}</button>
    <button class="btn-merge" on:click={apply} disabled={loading || applying || !preview}>
      {#if applying}<Loader2 size={14} class="animate-spin" />{:else}<GitMerge size={14} />{/if}
      {$t('components.mergeDialog.mergeInto', { values: { into } })}
    </button>
  </div>
</div>

<style>
  .overlay { position: fixed; inset: 0; background: rgba(0,0,0,0.4); z-index: 50; }
  .dialog { position: fixed; z-index: 51; top: 50%; left: 50%; transform: translate(-50%,-50%); width: min(90vw, 640px); max-height: 80vh; display: flex; flex-direction: column; background: var(--surface, #fff); border: 1px solid var(--line-soft, #e5e7eb); border-radius: 10px; box-shadow: 0 10px 40px rgba(0,0,0,0.2); }
  .dlg-header { display: flex; align-items: center; gap: 0.5rem; padding: 0.75rem 1rem; border-bottom: 1px solid var(--line-soft, #e5e7eb); }
  .dlg-title { font-weight: 600; font-size: 0.9rem; flex: 1; }
  .dlg-x { background: none; border: none; cursor: pointer; color: var(--ink-400, #9ca3af); }
  .dlg-body { padding: 1rem; overflow-y: auto; display: flex; flex-direction: column; gap: 0.6rem; }
  .dlg-footer { display: flex; justify-content: flex-end; gap: 0.5rem; padding: 0.75rem 1rem; border-top: 1px solid var(--line-soft, #e5e7eb); }
  .summary { display: flex; align-items: center; gap: 0.75rem; font-size: 0.78rem; }
  .clean { color: #16a34a; display: inline-flex; gap: 0.3rem; align-items: center; }
  .conf { color: #d97706; font-weight: 600; }
  .add { color: #16a34a; } .rem { color: #dc2626; }
  .muted { color: var(--ink-400, #9ca3af); display: inline-flex; gap: 0.3rem; align-items: center; }
  .err { color: #dc2626; font-size: 0.8rem; }
  .conflict { border: 1px solid var(--line-soft, #e5e7eb); border-radius: 6px; padding: 0.5rem; }
  .conflict-key { font-family: monospace; font-size: 0.72rem; color: var(--ink-500, #6b7280); margin-bottom: 0.4rem; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .conflict-sides { display: grid; grid-template-columns: 1fr 1fr; gap: 0.5rem; }
  .side { text-align: left; padding: 0.4rem; border: 1px solid var(--line-soft, #e5e7eb); border-radius: 6px; background: transparent; cursor: pointer; font-size: 0.72rem; }
  .side.sel { border-color: var(--brand-500, #2563eb); background: rgba(37,99,235,0.08); }
  .side-label { font-size: 0.62rem; text-transform: uppercase; letter-spacing: 0.04em; color: var(--ink-400, #9ca3af); margin-bottom: 0.2rem; }
  .mono { font-family: monospace; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .btn-cancel { padding: 0.4rem 0.8rem; border: 1px solid var(--line-soft, #e5e7eb); border-radius: 6px; background: transparent; cursor: pointer; font-size: 0.8rem; }
  .btn-merge { display: inline-flex; align-items: center; gap: 0.3rem; padding: 0.4rem 0.8rem; border: none; border-radius: 6px; background: var(--brand-500, #2563eb); color: #fff; cursor: pointer; font-size: 0.8rem; }
  .btn-merge:disabled { opacity: 0.5; cursor: not-allowed; }

  :global(:is([data-theme="dark"], .dark)) .dialog { background: var(--bg-strong); }
  :global(:is([data-theme="dark"], .dark)) .clean,
  :global(:is([data-theme="dark"], .dark)) .add { color: #6ee7b7; }
  :global(:is([data-theme="dark"], .dark)) .conf { color: #fcd34d; }
  :global(:is([data-theme="dark"], .dark)) .rem,
  :global(:is([data-theme="dark"], .dark)) .err { color: #fca5a5; }
  :global(:is([data-theme="dark"], .dark)) .side.sel { background: rgba(59,130,246,0.18); }
</style>
