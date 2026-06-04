<script>
  import { onMount, onDestroy, createEventDispatcher } from 'svelte';

  /**
   * Portal-style context menu.
   *
   * Usage:
   *   <ContextMenu bind:visible bind:x bind:y items={[
   *     { label: 'Expand', icon: Network, action: 'expand' },
   *     { divider: true },
   *     { label: 'Delete', icon: Trash2, action: 'delete', danger: true },
   *   ]} on:action={e => handleAction(e.detail)} />
   *
   * Items:
   *   { label: string, icon?: SvelteComponent, action: string, danger?: boolean, disabled?: boolean }
   *   { divider: true }
   */

  export let visible = false;
  export let x = 0;
  export let y = 0;
  export let items = [];

  const dispatch = createEventDispatcher();

  let menuEl;

  function close() {
    visible = false;
  }

  function handleItem(item) {
    if (item.disabled) return;
    dispatch('action', item.action);
    close();
  }

  function onWindowClick(e) {
    if (visible && menuEl && !menuEl.contains(e.target)) close();
  }

  function onKeydown(e) {
    if (e.key === 'Escape') close();
  }

  // Clamp to viewport
  function clampedPos(x, y) {
    if (typeof window === 'undefined') return { left: x, top: y };
    const W = window.innerWidth;
    const H = window.innerHeight;
    const menuW = 200;
    const menuH = items.length * 34 + 16; // approx
    return {
      left: Math.min(x, W - menuW - 8),
      top: Math.min(y, H - menuH - 8),
    };
  }

  $: pos = clampedPos(x, y);

  onMount(() => {
    window.addEventListener('click', onWindowClick, true);
    window.addEventListener('keydown', onKeydown, true);
  });

  onDestroy(() => {
    window.removeEventListener('click', onWindowClick, true);
    window.removeEventListener('keydown', onKeydown, true);
  });
</script>

{#if visible}
  <div
    bind:this={menuEl}
    class="ctx-menu"
    tabindex="-1"
    style="left:{pos.left}px; top:{pos.top}px"
    role="menu"
    on:keydown={onKeydown}
  >
    {#each items as item}
      {#if item.divider}
        <div class="ctx-divider"></div>
      {:else}
        <button
          class="ctx-item"
          class:ctx-danger={item.danger}
          class:ctx-disabled={item.disabled}
          role="menuitem"
          on:click={() => handleItem(item)}
        >
          {#if item.icon}
            <span class="ctx-icon"><svelte:component this={item.icon} size={14} /></span>
          {/if}
          <span class="ctx-label">{item.label}</span>
          {#if item.shortcut}
            <span class="ctx-shortcut">{item.shortcut}</span>
          {/if}
        </button>
      {/if}
    {/each}
  </div>
{/if}

<style>
  .ctx-menu {
    position: fixed;
    z-index: 99999;
    background: #ffffff;
    border: 1px solid #e2e8f0;
    border-radius: 9px;
    box-shadow: 0 4px 24px rgba(0,0,0,0.14), 0 1px 4px rgba(0,0,0,0.08);
    padding: 5px;
    min-width: 180px;
    max-width: 260px;
    font-size: 13px;
  }

  .ctx-item {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    padding: 6px 10px;
    border: none;
    background: transparent;
    border-radius: 6px;
    cursor: pointer;
    color: #1e293b;
    text-align: left;
    font-size: 13px;
    font-family: inherit;
    transition: background 0.1s;
    white-space: nowrap;
  }

  .ctx-item:hover:not(.ctx-disabled) {
    background: #f1f5f9;
  }

  .ctx-item.ctx-danger {
    color: #dc2626;
  }

  .ctx-item.ctx-danger:hover:not(.ctx-disabled) {
    background: #fef2f2;
  }

  .ctx-item.ctx-disabled {
    opacity: 0.45;
    cursor: default;
  }

  .ctx-icon {
    display: flex;
    align-items: center;
    flex-shrink: 0;
    color: #64748b;
  }

  .ctx-item.ctx-danger .ctx-icon {
    color: #dc2626;
  }

  .ctx-label {
    flex: 1;
  }

  .ctx-shortcut {
    font-size: 11px;
    color: #94a3b8;
    font-family: 'IBM Plex Mono', monospace;
  }

  .ctx-divider {
    height: 1px;
    background: #f1f5f9;
    margin: 4px 5px;
  }

  :global(:is([data-theme="dark"], .dark)) .ctx-menu { background: var(--bg-strong); border-color: var(--line-strong); }
  :global(:is([data-theme="dark"], .dark)) .ctx-item { color: var(--ink-900); }
  :global(:is([data-theme="dark"], .dark)) .ctx-item:hover:not(.ctx-disabled) { background: rgba(255,255,255,0.06); }
  :global(:is([data-theme="dark"], .dark)) .ctx-item.ctx-danger { color: #fca5a5; }
  :global(:is([data-theme="dark"], .dark)) .ctx-item.ctx-danger:hover:not(.ctx-disabled) { background: rgba(239,68,68,0.15); }
  :global(:is([data-theme="dark"], .dark)) .ctx-item.ctx-danger .ctx-icon { color: #fca5a5; }
  :global(:is([data-theme="dark"], .dark)) .ctx-divider { background: var(--line-strong); }
</style>
