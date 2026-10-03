<script>
  // Renders the open askConfirm()/askText() request (lib/confirm.ts) — mounted
  // once in App.svelte.
  import { tick } from 'svelte';
  import { t } from 'svelte-i18n';
  import ConfirmModal from './ConfirmModal.svelte';
  import { pendingConfirm, settleConfirm } from '../lib/confirm';

  let text = '';
  let inputEl;
  let shown = null;

  // Reset the field for each new request and focus it.
  $: if ($pendingConfirm !== shown) {
    shown = $pendingConfirm;
    text = shown?.defaultValue ?? '';
    if (shown?.text) tick().then(() => { inputEl?.focus(); inputEl?.select(); });
  }

  function submit() { settleConfirm(shown?.text ? text : ''); }
  function onKeydown(e) {
    if (e.key === 'Enter') { e.preventDefault(); submit(); }
  }
</script>

{#if $pendingConfirm}
  <ConfirmModal
    title={$pendingConfirm.title}
    message={$pendingConfirm.message ?? ''}
    confirmLabel={$pendingConfirm.confirmLabel ?? $t('system.confirm')}
    cancelLabel={$pendingConfirm.cancelLabel ?? $t('system.cancel')}
    confirmVariant={$pendingConfirm.variant ?? 'danger'}
    on:confirm={submit}
    on:cancel={() => settleConfirm(null)}
  >
    {#if $pendingConfirm.text}
      <label class="ask-text">
        <span>{$pendingConfirm.label ?? $pendingConfirm.title}</span>
        <input type="text" bind:value={text} bind:this={inputEl} on:keydown={onKeydown} autocomplete="off" spellcheck="false" />
      </label>
    {/if}
  </ConfirmModal>
{/if}

<style>
  .ask-text { display: grid; gap: 0.3rem; width: 100%; margin-top: 0.5rem; text-align: left; font-size: 0.8rem; font-weight: 600; color: var(--ink-600, #475569); }
  .ask-text input { width: 100%; box-sizing: border-box; padding: 0.5rem 0.65rem; border: 1px solid var(--line-soft, #e2e8f0); border-radius: 0.6rem; font: inherit; font-weight: 400; font-size: 0.9rem; }
</style>
