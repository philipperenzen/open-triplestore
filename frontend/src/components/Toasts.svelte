<script>
  import { toasts, dismiss } from '../lib/toast.js';
  import { Check, X, AlertTriangle, Info } from 'lucide-svelte';
  import { t as i18nT } from 'svelte-i18n';
</script>

<div class="toast-container" aria-live="polite">
  {#each $toasts as t (t.id)}
    <div class="toast toast-{t.type}" role="alert">
      <span class="toast-icon">
        {#if t.type === 'success'}<Check size={14} />{:else if t.type === 'error'}<X size={14} />{:else if t.type === 'warning'}<AlertTriangle size={14} />{:else}<Info size={14} />{/if}
      </span>
      <span class="toast-msg">{t.message}</span>
      <button class="toast-close" on:click={() => dismiss(t.id)} aria-label={$i18nT('components.toasts.dismiss')}>×</button>
    </div>
  {/each}
</div>

<style>
  .toast-container {
    position: fixed;
    bottom: 1.25rem;
    right: 1.25rem;
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    z-index: 9999;
    max-width: 380px;
    pointer-events: none;
  }

  .toast {
    display: flex;
    align-items: flex-start;
    gap: 0.6rem;
    padding: 0.65rem 0.9rem;
    border-radius: 8px;
    box-shadow: 0 4px 16px rgba(0,0,0,0.18);
    font-size: 0.875rem;
    line-height: 1.4;
    animation: slide-in 0.2s ease;
    pointer-events: all;
  }

  @keyframes slide-in {
    from { opacity: 0; transform: translateX(20px); }
    to   { opacity: 1; transform: translateX(0); }
  }

  .toast-success { background: #1a4a30; color: #90cfa8; border-left: 3px solid #4caf50; }
  .toast-error   { background: #4a1a1a; color: #ffaaaa; border-left: 3px solid #d94a4a; }
  .toast-warning { background: #4a3a10; color: #ffd080; border-left: 3px solid #ffc107; }
  .toast-info    { background: #1a2e4a; color: #a8d4ff; border-left: 3px solid #4a90d9; }

  .toast-icon { flex-shrink: 0; font-weight: 700; font-size: 0.9rem; }

  .toast-msg { flex: 1; word-break: break-word; }

  .toast-close {
    flex-shrink: 0;
    background: none;
    border: none;
    color: inherit;
    opacity: 0.6;
    cursor: pointer;
    font-size: 1.1rem;
    line-height: 1;
    padding: 0;
    margin-left: 0.25rem;
  }
  .toast-close:hover { opacity: 1; }
</style>
