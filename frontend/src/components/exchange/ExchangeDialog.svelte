<script lang="ts">
  // A plain modal frame for the exchange dialogs (IDS import/export): title,
  // a scrolling body and a footer slot. Escape and the backdrop close it
  // unless `busy` (a request in flight).
  import { createEventDispatcher, onMount, onDestroy } from 'svelte';
  import { t } from 'svelte-i18n';
  import { X as XIcon } from 'lucide-svelte';
  import './exchange.css';

  export let title: string;
  export let busy = false;

  const dispatch = createEventDispatcher<{ close: void }>();
  let box: HTMLDivElement;

  function close() {
    if (!busy) dispatch('close');
  }
  function onKey(e: KeyboardEvent) {
    if (e.key === 'Escape') close();
  }
  onMount(() => {
    document.addEventListener('keydown', onKey);
    box?.focus();
  });
  onDestroy(() => document.removeEventListener('keydown', onKey));
</script>

<div class="xc-backdrop" on:click={close} role="presentation">
  <div
    class="xc-dialog"
    bind:this={box}
    on:click|stopPropagation
    on:keydown|stopPropagation={onKey}
    role="dialog"
    aria-modal="true"
    aria-label={title}
    tabindex="-1"
  >
    <div class="xc-dialog-head">
      <h3>{title}</h3>
      <button class="icon-btn" on:click={close} disabled={busy} aria-label={$t('system.close')}><XIcon size={16} /></button>
    </div>
    <div class="xc-dialog-body"><slot /></div>
    {#if $$slots.footer}
      <div class="xc-dialog-foot"><slot name="footer" /></div>
    {/if}
  </div>
</div>
