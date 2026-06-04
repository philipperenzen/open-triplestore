<script>
  import { createEventDispatcher } from 'svelte';
  import { t } from 'svelte-i18n';
  import { sanitizeHtml } from '../lib/ontology/sanitizeHtml.js';
  import { Loader2, CheckCircle, X } from 'lucide-svelte';

  export let registryId;
  export let version;
  /** Function with signature (registryId, version) => Promise */
  export let publishFn;

  const dispatch = createEventDispatcher();

  let loading = false;
  let error = '';

  async function handleConfirm() {
    loading = true;
    error = '';
    try {
      await publishFn(registryId, version);
      dispatch('published');
    } catch (e) {
      error = e.message;
    }
    loading = false;
  }
</script>

<div class="modal-backdrop" on:click={() => dispatch('cancel')} role="presentation" on:keydown={(e) => e.key === 'Escape' && dispatch('cancel')}>
  <div class="modal-box" on:click|stopPropagation on:keydown|stopPropagation role="dialog" aria-modal="true" aria-label={$t('components.publishConfirmDialog.dialogLabel')} tabindex="-1">
    <div class="flex items-center justify-between mb-3">
      <h3 class="text-lg font-semibold m-0">{$t('components.publishConfirmDialog.heading', { values: { version } })}</h3>
      <button class="p-1.5 rounded-lg hover:bg-[var(--bg-soft)] text-[var(--ink-400)]" on:click={() => dispatch('cancel')}>
        <X size={18} />
      </button>
    </div>

    <p class="text-sm text-[var(--ink-500)] mb-4">
      <!-- eslint-disable-next-line svelte/no-at-html-tags -- DOMPurify-sanitized -->
      {@html sanitizeHtml($t('components.publishConfirmDialog.description', { values: { version, registryId } }))}
    </p>

    {#if error}
      <p class="text-sm text-red-600 mb-3">{error}</p>
    {/if}

    <div class="flex gap-2 justify-end">
      <button class="btn btn-ghost" on:click={() => dispatch('cancel')}>{$t('system.cancel')}</button>
      <button class="btn btn-success" disabled={loading} on:click={handleConfirm}>
        {#if loading}<Loader2 size={14} class="animate-spin" />{:else}<CheckCircle size={14} />{/if}
        {$t('components.publishConfirmDialog.publishButton')}
      </button>
    </div>
  </div>
</div>

<style>
  .modal-backdrop { position: fixed; inset: 0; background: rgba(0,0,0,0.35); display: flex; align-items: center; justify-content: center; z-index: 50; }
  .modal-box { background: white; border-radius: 1rem; padding: 1.5rem; width: min(420px, calc(100vw - 2rem)); box-shadow: 0 20px 60px rgba(0,0,0,0.15); }
  .btn { display: inline-flex; align-items: center; gap: 0.375rem; padding: 0.5rem 1rem; border-radius: 0.75rem; font-size: 0.875rem; font-weight: 500; cursor: pointer; border: none; transition: all 0.15s; }
  .btn-success { background: #22c55e; color: white; }
  .btn-success:hover:not(:disabled) { background: #16a34a; }
  .btn-ghost { background: transparent; color: var(--ink-600, #475569); }
  .btn-ghost:hover { background: var(--bg-soft, #f1f5f9); }
  .btn:disabled { opacity: 0.6; cursor: not-allowed; }

  :global(:is([data-theme="dark"], .dark)) .modal-box { background: var(--bg-strong); }
</style>
