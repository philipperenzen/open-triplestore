<script>
  import { createEventDispatcher, onMount } from 'svelte';
  import { t } from 'svelte-i18n';
  import { sanitizeHtml } from '../lib/ontology/sanitizeHtml.js';
  import { Loader2, X, Search, Globe, Users, Lock, ShieldCheck } from 'lucide-svelte';
  import { listShapeGraphs, listBindingsForTarget, createBinding, deleteBinding } from '../lib/api.js';
  import { toastError } from '../lib/toast.ts';

  /** 'dataset' | 'graph' | 'shapegraph' — the wire value the backend expects. */
  export let targetKind;
  /** Dataset/shape-graph id, or (for a graph) the graph IRI. */
  export let targetId;
  /** Human label for the modal header (defaults to the id). */
  export let targetLabel = '';

  const dispatch = createEventDispatcher();

  let sets = [];
  let boundIds = new Set();
  let loading = true;
  let error = '';
  let busyId = null;
  let search = '';

  $: target = { kind: targetKind, id: targetId };
  $: filtered = sets.filter((s) => {
    const q = search.trim().toLowerCase();
    if (!q) return true;
    return (s.name || '').toLowerCase().includes(q) || (s.description || '').toLowerCase().includes(q);
  });

  onMount(load);

  async function load() {
    loading = true; error = '';
    try {
      const [all, bound] = await Promise.all([
        listShapeGraphs(),
        listBindingsForTarget(targetKind, targetId),
      ]);
      sets = all || [];
      // A shape graph may not validate itself, so hide it from its own picker.
      if (targetKind === 'shapegraph') sets = sets.filter((s) => s.id !== targetId);
      boundIds = new Set((bound?.shape_graphs || []).map((s) => s.id));
    } catch (e) {
      error = e.message;
    }
    loading = false;
  }

  async function toggle(set) {
    busyId = set.id;
    const wasBound = boundIds.has(set.id);
    try {
      if (wasBound) await deleteBinding(target, set.id);
      else await createBinding(target, set.id);
      const next = new Set(boundIds);
      if (wasBound) next.delete(set.id); else next.add(set.id);
      boundIds = next;
      dispatch('changed');
    } catch (e) {
      toastError(e.message);
    }
    busyId = null;
  }

  function visIcon(v) {
    return v === 'public' ? Globe : v === 'members' ? Users : Lock;
  }
</script>

<div class="modal-backdrop" on:click={() => dispatch('close')} role="presentation" on:keydown={(e) => e.key === 'Escape' && dispatch('close')}>
  <div class="modal-box" on:click|stopPropagation on:keydown|stopPropagation role="dialog" aria-modal="true" aria-label={$t('components.attachShapesDialog.title')} tabindex="-1">
    <div class="flex items-center justify-between mb-1">
      <h3 class="text-lg font-semibold m-0 flex items-center gap-2"><ShieldCheck size={18} class="text-[var(--brand-500)]" /> {$t('components.attachShapesDialog.title')}</h3>
      <button class="p-1.5 rounded-lg hover:bg-[var(--bg-soft)] text-[var(--ink-400)]" on:click={() => dispatch('close')}>
        <X size={18} />
      </button>
    </div>
    <p class="text-sm text-[var(--ink-500)] mb-3">
      <!-- eslint-disable-next-line svelte/no-at-html-tags -- DOMPurify-sanitized -->
      {@html sanitizeHtml($t('components.attachShapesDialog.intro', { values: { target: `<strong>${targetLabel || targetId}</strong>` } }))}{#if targetKind === 'graph'} {$t('components.attachShapesDialog.introGraphTail')}{:else} {$t('components.attachShapesDialog.introDefaultTail')}{/if}
    </p>

    <div class="search-wrap mb-3">
      <Search size={14} class="text-[var(--ink-400)]" />
      <input class="search-input" placeholder={$t('components.attachShapesDialog.filterPlaceholder')} bind:value={search} />
    </div>

    {#if error}
      <p class="text-sm text-red-600 mb-3">{error}</p>
    {/if}

    <div class="set-list">
      {#if loading}
        <span class="muted"><Loader2 size={14} class="animate-spin" /> {$t('system.loading')}</span>
      {:else if filtered.length === 0}
        <span class="muted">{sets.length === 0 ? $t('components.attachShapesDialog.emptyLibrary') : $t('components.attachShapesDialog.noMatches')}</span>
      {:else}
        {#each filtered as set (set.id)}
          {@const Icon = visIcon(set.visibility)}
          <label class="set-row" class:bound={boundIds.has(set.id)}>
            <input
              type="checkbox"
              checked={boundIds.has(set.id)}
              disabled={busyId === set.id}
              on:change={() => toggle(set)}
            />
            <div class="set-main">
              <div class="set-name">
                {set.name}
                {#if busyId === set.id}<Loader2 size={12} class="animate-spin" />{/if}
              </div>
              <div class="set-meta">
                <Icon size={11} /> {set.visibility}
                {#if set.status}<span class="dot">·</span><span class="status status-{set.status}">{set.status}</span>{/if}
                {#if set.shape_count != null}<span class="dot">·</span>{$t('components.attachShapesDialog.shapeCount', { values: { count: set.shape_count } })}{/if}
              </div>
            </div>
          </label>
        {/each}
      {/if}
    </div>

    <div class="flex justify-end mt-4">
      <button class="btn btn-ghost" on:click={() => dispatch('close')}>{$t('components.attachShapesDialog.done')}</button>
    </div>
  </div>
</div>

<style>
  .modal-backdrop { position: fixed; inset: 0; background: rgba(0,0,0,0.35); display: flex; align-items: center; justify-content: center; z-index: 50; }
  .modal-box { background: white; border-radius: 1rem; padding: 1.5rem; width: min(520px, calc(100vw - 2rem)); max-height: calc(100vh - 4rem); display: flex; flex-direction: column; box-shadow: 0 20px 60px rgba(0,0,0,0.15); }
  .search-wrap { display: flex; align-items: center; gap: 0.4rem; border: 1px solid var(--line-soft, #e5e7eb); border-radius: 0.6rem; padding: 0.35rem 0.6rem; }
  .search-input { flex: 1; border: none; outline: none; background: transparent; font-size: 0.85rem; }
  .set-list { display: flex; flex-direction: column; gap: 0.35rem; overflow-y: auto; }
  .set-row { display: flex; align-items: flex-start; gap: 0.6rem; padding: 0.5rem 0.6rem; border: 1px solid var(--line-soft, #e5e7eb); border-radius: 0.6rem; cursor: pointer; }
  .set-row:hover { background: var(--bg-soft, #f8fafc); }
  .set-row.bound { border-color: var(--brand-400, #818cf8); background: var(--brand-50, #eef2ff); }
  .set-row input { margin-top: 0.2rem; }
  .set-main { flex: 1; min-width: 0; }
  .set-name { font-size: 0.85rem; font-weight: 600; display: flex; align-items: center; gap: 0.4rem; }
  .set-meta { display: flex; align-items: center; gap: 0.25rem; font-size: 0.7rem; color: var(--ink-500, #6b7280); margin-top: 0.1rem; text-transform: capitalize; }
  .dot { color: var(--ink-300, #cbd5e1); }
  .status { padding: 0.02rem 0.3rem; border-radius: 4px; font-weight: 600; }
  .status-draft { background: #f3f4f6; color: #6b7280; }
  .status-staged { background: #fef3c7; color: #92400e; }
  .status-published { background: #dcfce7; color: #166534; }
  .status-deprecated { background: #fee2e2; color: #991b1b; }
  .muted { color: var(--ink-400, #9ca3af); font-size: 0.8rem; display: inline-flex; gap: 0.3rem; align-items: center; padding: 0.5rem; }
  .btn { display: inline-flex; align-items: center; gap: 0.375rem; padding: 0.5rem 1rem; border-radius: 0.75rem; font-size: 0.875rem; font-weight: 500; cursor: pointer; border: none; }
  .btn-ghost { background: transparent; color: var(--ink-600, #475569); }
  .btn-ghost:hover { background: var(--bg-soft, #f1f5f9); }

  :global(:is([data-theme="dark"], .dark)) .modal-box { background: var(--bg-strong); }
</style>
