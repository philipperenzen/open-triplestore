<script>
  // Full-screen file-browser dialog: manage a dataset's files from anywhere, or
  // pick one (mode="pick" — emits `select` with the chosen asset).
  import { createEventDispatcher, onMount, onDestroy } from 'svelte';
  import { t } from 'svelte-i18n';
  import { X as XIcon, FolderOpen, ExternalLink } from 'lucide-svelte';
  import { Link } from '../../lib/router/index.js';
  import FileBrowser from './FileBrowser.svelte';

  export let datasetId;
  export let datasetName = '';
  export let canWrite = false;
  export let mode = 'manage'; // 'manage' | 'pick'
  export let initialPath = '';
  export let datasetVisibility = null;
  /** Hide the "open full page" shortcut (e.g. when already on /files). */
  export let showPageLink = true;

  const dispatch = createEventDispatcher();

  function close() { dispatch('close'); }

  function onKeydown(e) {
    // The browser itself stops propagation for its own Escape uses (menus,
    // selection); a bubble reaching us means "close the dialog".
    if (e.key === 'Escape') close();
  }

  onMount(() => {
    window.addEventListener('keydown', onKeydown);
    document.body.style.overflow = 'hidden';
  });
  onDestroy(() => {
    window.removeEventListener('keydown', onKeydown);
    document.body.style.overflow = '';
  });
</script>

<div class="fbm-backdrop" on:click|self={close} role="presentation">
  <div class="fbm-shell" role="dialog" aria-modal="true" aria-label={$t('components.fileBrowser.dialogAria')} tabindex="-1">
    <div class="fbm-head">
      <div class="fbm-title">
        <FolderOpen size={16} />
        <span>
          {mode === 'pick' ? $t('components.fileBrowser.pickTitle') : $t('components.fileBrowser.manageTitle')}
          {#if datasetName}<span class="fbm-dataset">· {datasetName}</span>{/if}
        </span>
      </div>
      <div class="fbm-head-actions">
        {#if showPageLink && mode === 'manage'}
          <Link class="fbm-page-link" to={`/files/${datasetId}`} on:click={close} title={$t('components.fileBrowser.openFullPage')}>
            <ExternalLink size={13} /> {$t('components.fileBrowser.openFullPage')}
          </Link>
        {/if}
        <button class="fbm-close" on:click={close} title={$t('system.close')} aria-label={$t('system.close')}>
          <XIcon size={16} />
        </button>
      </div>
    </div>
    <div class="fbm-body">
      <FileBrowser
        {datasetId}
        {canWrite}
        {mode}
        {initialPath}
        {datasetVisibility}
        on:select={(e) => dispatch('select', e.detail)}
        on:close={close}
      />
    </div>
  </div>
</div>

<style>
  .fbm-backdrop {
    position: fixed; inset: 0; z-index: 45000;
    background: rgba(15, 23, 42, 0.55);
    backdrop-filter: blur(2px);
    display: flex; align-items: center; justify-content: center;
    padding: 2.5vh 2vw;
  }
  .fbm-shell {
    width: min(1120px, 100%);
    height: min(760px, 95vh);
    background: var(--bg, #fff);
    border-radius: 16px;
    box-shadow: 0 24px 70px rgba(0, 0, 0, 0.28);
    display: flex; flex-direction: column;
    overflow: hidden;
    animation: fbmIn 0.16s ease;
  }
  @keyframes fbmIn {
    from { opacity: 0; transform: scale(0.97) translateY(8px); }
    to { opacity: 1; transform: scale(1) translateY(0); }
  }

  .fbm-head {
    display: flex; align-items: center; justify-content: space-between; gap: 1rem;
    padding: 0.7rem 1rem;
    border-bottom: 1px solid var(--line-soft, #eef2f6);
    background: var(--bg-soft, #f8fafc);
    flex-shrink: 0;
  }
  .fbm-title {
    display: flex; align-items: center; gap: 0.45rem;
    font-weight: 700; font-size: 0.92rem; color: var(--ink-900, #0f172a);
    min-width: 0;
  }
  .fbm-dataset { color: var(--ink-400, #94a3b8); font-weight: 600; }
  .fbm-head-actions { display: flex; align-items: center; gap: 0.5rem; flex-shrink: 0; }
  :global(.fbm-page-link) {
    display: inline-flex; align-items: center; gap: 0.3rem;
    font-size: 0.76rem; font-weight: 600; text-decoration: none;
    color: var(--brand-600, #2563eb);
    padding: 0.3rem 0.55rem; border-radius: 8px;
    transition: background 0.12s;
  }
  :global(.fbm-page-link:hover) { background: var(--brand-50, #eff6ff); }
  .fbm-close {
    width: 30px; height: 30px; border-radius: 8px;
    display: grid; place-items: center;
    border: 1px solid var(--line-soft, #e2e8f0);
    background: var(--bg, #fff); color: var(--ink-500, #64748b);
    cursor: pointer; transition: all 0.12s;
  }
  .fbm-close:hover { background: #fee2e2; color: #dc2626; border-color: #fca5a5; }

  .fbm-body { flex: 1; min-height: 0; display: flex; flex-direction: column; padding: 0.75rem; }
  .fbm-body :global(.fb) { flex: 1; min-height: 0; }

  :global(:is([data-theme="dark"], .dark)) .fbm-shell { background: var(--bg-strong); }
  :global(:is([data-theme="dark"], .dark)) .fbm-head { background: var(--bg-soft); border-color: var(--line-soft); }
  :global(:is([data-theme="dark"], .dark)) .fbm-close { background: var(--bg-soft); border-color: var(--line-strong); color: var(--ink-600); }
  :global(:is([data-theme="dark"], .dark)) .fbm-close:hover { background: rgba(239,68,68,0.18); color: #fca5a5; border-color: rgba(239,68,68,0.35); }
</style>
