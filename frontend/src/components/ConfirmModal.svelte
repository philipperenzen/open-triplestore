<script>
  import { createEventDispatcher, onMount, onDestroy } from 'svelte';
  import { AlertTriangle, Trash2, Loader2 } from 'lucide-svelte';

  /** Modal heading */
  export let title = 'Are you sure?';
  /** Body text */
  export let message = '';
  /** Label on the confirm button */
  export let confirmLabel = 'Delete';
  /**
   * Visual style for the confirm button.
   * 'danger'  → red (default, for destructive deletes)
   * 'warning' → amber (for revoke / deactivate actions)
   */
  export let confirmVariant = 'danger';
  /** When true, shows a spinner on the confirm button and disables both buttons */
  export let loading = false;
  /**
   * When set to a non-empty string, the user must type this exact phrase before
   * the confirm button is enabled (e.g. the resource name for destructive deletes).
   */
  export let requirePhrase = null;

  let typedPhrase = '';
  $: confirmDisabled = loading || (requirePhrase !== null && typedPhrase !== requirePhrase);

  const dispatch = createEventDispatcher();

  function confirm() { if (!confirmDisabled) dispatch('confirm'); }
  function cancel()  {
    if (!loading) {
      typedPhrase = '';
      dispatch('cancel');
    }
  }

  function handleKeydown(e) {
    if (e.key === 'Escape' && !loading) cancel();
  }

  let dialogEl;
  onMount(() => {
    document.addEventListener('keydown', handleKeydown);
    // Focus the cancel button so Enter does not accidentally confirm
    dialogEl?.querySelector('.modal-cancel')?.focus();
  });
  onDestroy(() => {
    document.removeEventListener('keydown', handleKeydown);
  });
</script>

<!-- svelte-ignore a11y-click-events-have-key-events -->
<div
  class="confirm-backdrop"
  on:click={cancel}
  role="presentation"
>
  <div
    class="confirm-box"
    on:click|stopPropagation
    role="dialog"
    aria-modal="true"
    aria-labelledby="confirm-title"
    tabindex="-1"
    bind:this={dialogEl}
  >
    <!-- Icon -->
    <div class="confirm-icon confirm-icon-{confirmVariant}">
      {#if confirmVariant === 'warning'}
        <AlertTriangle size={22} />
      {:else}
        <Trash2 size={22} />
      {/if}
    </div>

    <h3 id="confirm-title" class="confirm-title">{title}</h3>

    {#if message}
      <p class="confirm-message">{message}</p>
    {/if}

    <!-- Slot for extra content (e.g. resource name) -->
    <slot />

    {#if requirePhrase !== null}
      <div class="confirm-phrase-wrap">
        <label class="confirm-phrase-label" for="confirm-phrase-input">
          Type <strong>{requirePhrase}</strong> to confirm
        </label>
        <input
          id="confirm-phrase-input"
          class="confirm-phrase-input"
          type="text"
          bind:value={typedPhrase}
          autocomplete="off"
          spellcheck="false"
        />
      </div>
    {/if}

    <div class="confirm-actions">
      <button
        class="confirm-btn confirm-btn-{confirmVariant}"
        on:click={confirm}
        disabled={confirmDisabled}
      >
        {#if loading}
          <Loader2 size={14} class="animate-spin" />
        {/if}
        {confirmLabel}
      </button>
      <button class="confirm-btn confirm-btn-ghost modal-cancel" on:click={cancel} disabled={loading}>
        Cancel
      </button>
    </div>
  </div>
</div>

<style>
  .confirm-backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.35);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 100;
  }

  .confirm-box {
    background: white;
    border-radius: 1rem;
    padding: 1.75rem 1.5rem 1.5rem;
    width: min(440px, calc(100vw - 2rem));
    box-shadow: 0 20px 60px rgba(0, 0, 0, 0.18);
    display: flex;
    flex-direction: column;
    align-items: center;
    text-align: center;
    gap: 0.5rem;
  }

  .confirm-icon {
    width: 3rem;
    height: 3rem;
    border-radius: 50%;
    display: flex;
    align-items: center;
    justify-content: center;
    margin-bottom: 0.25rem;
  }
  .confirm-icon-danger  { background: #fee2e2; color: #dc2626; }
  .confirm-icon-warning { background: #fef3c7; color: #d97706; }

  .confirm-title {
    font-size: 1.05rem;
    font-weight: 700;
    color: var(--ink-900, #1a1a2e);
    margin: 0;
  }

  .confirm-message {
    font-size: 0.875rem;
    color: var(--ink-500, #6b7280);
    margin: 0.25rem 0 0;
    line-height: 1.5;
  }

  /* Extra slot content — e.g. resource name code block */
  :global(.confirm-box code) {
    display: inline-block;
    background: #f1f5f9;
    border-radius: 0.375rem;
    padding: 0.2rem 0.5rem;
    font-size: 0.8rem;
    color: var(--ink-800, #1e293b);
    word-break: break-all;
    margin-top: 0.25rem;
  }

  .confirm-actions {
    display: flex;
    gap: 0.625rem;
    margin-top: 1rem;
    flex-wrap: wrap;
    justify-content: center;
  }

  .confirm-btn {
    display: inline-flex;
    align-items: center;
    gap: 0.375rem;
    padding: 0.5rem 1.25rem;
    border-radius: 0.75rem;
    font-size: 0.875rem;
    font-weight: 500;
    cursor: pointer;
    border: none;
    transition: all 0.15s;
  }
  .confirm-btn:disabled { opacity: 0.6; cursor: not-allowed; }

  .confirm-btn-danger  { background: #ef4444; color: white; }
  .confirm-btn-danger:hover  { background: #dc2626; }

  .confirm-btn-warning { background: #f59e0b; color: white; }
  .confirm-btn-warning:hover { background: #d97706; }

  .confirm-btn-ghost {
    background: transparent;
    color: var(--ink-600, #4b5563);
    border: 1px solid var(--line-soft, #e5e7eb);
  }
  .confirm-btn-ghost:hover { background: var(--bg-soft, #f8fafc); }

  .confirm-phrase-wrap {
    width: 100%;
    margin-top: 0.75rem;
    text-align: left;
  }
  .confirm-phrase-label {
    display: block;
    font-size: 0.8rem;
    color: var(--ink-600, #4b5563);
    margin-bottom: 0.35rem;
  }
  .confirm-phrase-input {
    width: 100%;
    box-sizing: border-box;
    padding: 0.45rem 0.7rem;
    border: 1px solid var(--line-soft, #e5e7eb);
    border-radius: 0.5rem;
    font-size: 0.875rem;
    color: var(--ink-900, #1a1a2e);
    background: var(--bg, #fff);
    outline: none;
  }
  .confirm-phrase-input:focus {
    border-color: var(--brand-400, #818cf8);
    box-shadow: 0 0 0 2px rgba(129, 140, 248, 0.2);
  }

  /* ---- Dark mode overrides (scoped rules out-specify global theme.css) ---- */
  :global(:is([data-theme="dark"], .dark)) .confirm-box { background: var(--bg-strong); }
  :global(:is([data-theme="dark"], .dark)) .confirm-icon-danger { background: rgba(239,68,68,0.18); color: #fca5a5; }
  :global(:is([data-theme="dark"], .dark)) .confirm-icon-warning { background: rgba(245,158,11,0.18); color: #fcd34d; }
  :global(:is([data-theme="dark"], .dark) .confirm-box code) { background: rgba(255,255,255,0.06); }
</style>
