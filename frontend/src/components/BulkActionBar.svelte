<script>
  import { createEventDispatcher } from 'svelte';
  import { X } from 'lucide-svelte';
  import { t } from 'svelte-i18n';

  /** Number of currently selected items. */
  export let count = 0;
  /** Total number of visible (filtered) items. */
  export let total = 0;
  /** Label for the item type, e.g. "graph", "dataset". */
  export let itemLabel = 'item';

  const dispatch = createEventDispatcher();
</script>

{#if count > 0}
  <div class="bulk-bar" role="toolbar" aria-label={$t('components.bulkActionBar.toolbarLabel')}>
    <div class="bulk-bar-top">
      <div class="bulk-bar-left">
        <button
          class="bulk-clear"
          on:click={() => dispatch('clearSelection')}
          title={$t('components.bulkActionBar.clearSelection')}
          aria-label={$t('components.bulkActionBar.clearSelection')}
        >
          <X size={13} />
        </button>
        <span class="bulk-count">
          <strong>{count}</strong> {count === 1 ? itemLabel : itemLabel + 's'} {$t('components.bulkActionBar.selected')}
        </span>
      </div>

      <div class="bulk-bar-right">
        {#if count < total}
          <button class="bulk-ghost-btn" on:click={() => dispatch('selectAll')}>
            {$t('components.bulkActionBar.selectAllCount', { values: { total } })}
          </button>
        {:else}
          <button class="bulk-ghost-btn" on:click={() => dispatch('clearSelection')}>
            {$t('system.deselectAll')}
          </button>
        {/if}
      </div>
    </div>

    <div class="bulk-bar-actions">
      <div class="bulk-divider-h"></div>
      <!-- Page-specific action buttons go here -->
      <slot />
    </div>
  </div>
{/if}

<style>
  .bulk-bar {
    position: fixed;
    bottom: calc(4rem + env(safe-area-inset-bottom, 0px));
    left: 50%;
    transform: translateX(-50%);
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    background: var(--ink-900, #1e293b);
    color: white;
    padding: 0.65rem 0.85rem;
    border-radius: 14px;
    box-shadow: 0 8px 36px rgba(0, 0, 0, 0.28), 0 2px 8px rgba(0,0,0,0.15);
    z-index: 40;
    width: min(480px, calc(100vw - 2rem));
    animation: slideUp 0.18s cubic-bezier(0.22, 0.61, 0.36, 1);
  }

  @keyframes slideUp {
    from { opacity: 0; transform: translateX(-50%) translateY(14px); }
    to   { opacity: 1; transform: translateX(-50%) translateY(0); }
  }

  /* Top row: count + select-all */
  .bulk-bar-top {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.5rem;
  }

  .bulk-bar-left {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    min-width: 0;
  }

  .bulk-bar-right {
    flex-shrink: 0;
  }

  .bulk-clear {
    display: grid;
    place-items: center;
    width: 22px;
    height: 22px;
    flex-shrink: 0;
    border-radius: 50%;
    border: none;
    background: rgba(255, 255, 255, 0.14);
    color: white;
    cursor: pointer;
    transition: background 0.12s;
  }
  .bulk-clear:hover { background: rgba(255, 255, 255, 0.25); }

  .bulk-count {
    font-size: 0.82rem;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  /* Bottom row: action buttons */
  .bulk-bar-actions {
    display: flex;
    flex-direction: column;
    gap: 0.35rem;
  }

  .bulk-divider-h {
    height: 1px;
    background: rgba(255, 255, 255, 0.12);
    margin: 0 -0.15rem;
  }

  .bulk-ghost-btn {
    display: inline-flex;
    align-items: center;
    gap: 0.3rem;
    padding: 0.3rem 0.65rem;
    border-radius: 8px;
    border: 1px solid rgba(255, 255, 255, 0.18);
    background: rgba(255, 255, 255, 0.08);
    color: rgba(255, 255, 255, 0.85);
    font-size: 0.78rem;
    cursor: pointer;
    transition: all 0.12s;
    white-space: nowrap;
  }
  .bulk-ghost-btn:hover { background: rgba(255, 255, 255, 0.16); color: white; }

  /* Slotted action buttons */
  :global(.bulk-action-btn) {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 0.35rem;
    width: 100%;
    padding: 0.38rem 0.7rem;
    border-radius: 8px;
    border: 1px solid rgba(255, 255, 255, 0.18);
    background: rgba(255, 255, 255, 0.1);
    color: white;
    font-size: 0.8rem;
    cursor: pointer;
    transition: all 0.12s;
    white-space: nowrap;
    text-align: center;
  }
  :global(.bulk-action-btn:hover) { background: rgba(255, 255, 255, 0.2); }
  :global(.bulk-action-btn:disabled) { opacity: 0.45; cursor: not-allowed; }

  :global(.bulk-action-btn.danger) {
    border-color: rgba(248, 113, 113, 0.4);
    color: #fca5a5;
  }
  :global(.bulk-action-btn.danger:hover) {
    background: rgba(239, 68, 68, 0.25);
    color: #fecaca;
    border-color: rgba(248, 113, 113, 0.6);
  }

  /* On wider screens: put action buttons side by side */
  @media (min-width: 480px) {
    .bulk-bar-actions {
      flex-direction: row;
      flex-wrap: wrap;
    }
    :global(.bulk-action-btn) {
      width: auto;
      flex: 1 1 auto;
    }
  }

  /* Bottom-nav is hidden at ≥640px (sm:hidden), so restore default bottom spacing */
  @media (min-width: 640px) {
    .bulk-bar {
      bottom: 1.25rem;
    }
  }
</style>
