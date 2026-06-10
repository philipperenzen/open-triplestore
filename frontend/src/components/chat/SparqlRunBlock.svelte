<script>
  // A runnable SPARQL card in a chat answer: syntax-highlighted query with
  // Run / copy / open-in-workspace actions. Run executes through the normal
  // /sparql endpoint, i.e. the exact same authorization scope as a user-typed
  // query in the workspace.
  import { createEventDispatcher } from 'svelte';
  import { t } from 'svelte-i18n';
  import { sparqlQuery } from '../../lib/api.js';
  import { highlightSparql } from '../../lib/markdown.js';
  import { normalizeSparqlResult } from '../../lib/chatRich.js';
  import { copyToClipboard } from '../../lib/clipboard.js';
  import SparqlResultView from './SparqlResultView.svelte';
  import { Terminal, Play, Loader2, Copy, Check, ExternalLink } from 'lucide-svelte';

  export let code = '';

  const dispatch = createEventDispatcher();
  let running = false;
  let result = null;
  let error = null;
  let elapsed = null;
  let copied = false;

  async function run() {
    if (running) return;
    running = true;
    error = null;
    result = null;
    const t0 = performance.now();
    try {
      result = normalizeSparqlResult(await sparqlQuery(code));
    } catch (e) {
      error = e?.message || String(e);
    } finally {
      elapsed = Math.round(performance.now() - t0);
      running = false;
    }
  }

  async function copy() {
    await copyToClipboard(code);
    copied = true;
    setTimeout(() => { copied = false; }, 1500);
  }
</script>

<div class="block">
  <div class="head">
    <span class="label"><Terminal size={12} /> {$t('components.chat.sparqlTitle')}</span>
    <span class="actions">
      {#if elapsed != null && !running}<span class="elapsed">{elapsed} ms</span>{/if}
      <button class="act" on:click={copy} title={$t('components.chat.copy')} aria-label={$t('components.chat.copy')}>
        {#if copied}<Check size={12} />{:else}<Copy size={12} />{/if}
      </button>
      <button class="act" on:click={() => dispatch('openInSparql', code)} title={$t('components.chat.openInSparql')} aria-label={$t('components.chat.openInSparql')}>
        <ExternalLink size={12} />
      </button>
      <button class="act run" on:click={run} disabled={running}>
        {#if running}<Loader2 size={12} class="spin" /> {$t('components.chat.running')}{:else}<Play size={12} /> {$t('components.chat.run')}{/if}
      </button>
    </span>
  </div>
  <!-- highlightSparql HTML-escapes all source text (resultHighlight.js), so {@html} is safe. -->
  <!-- eslint-disable-next-line svelte/no-at-html-tags -->
  <pre class="code"><code>{@html highlightSparql(code)}</code></pre>
  {#if result || error}
    <div class="result"><SparqlResultView {result} {error} /></div>
  {/if}
</div>

<style>
  .block {
    margin: 0 0 0.55rem; border: 1px solid var(--line-soft); border-radius: 10px;
    background: var(--bg-soft); overflow: hidden;
  }
  .head {
    display: flex; align-items: center; justify-content: space-between; gap: 0.5rem;
    padding: 0.3rem 0.55rem; border-bottom: 1px solid var(--line-soft);
  }
  .label {
    display: inline-flex; align-items: center; gap: 0.35rem;
    font-size: 0.7rem; font-weight: 700; letter-spacing: 0.4px; text-transform: uppercase;
    color: var(--ink-500);
  }
  .actions { display: inline-flex; align-items: center; gap: 0.3rem; }
  .elapsed { font-size: 0.68rem; color: var(--ink-400); }
  .act {
    display: inline-flex; align-items: center; gap: 0.25rem; cursor: pointer;
    font-size: 0.7rem; font-weight: 600; padding: 2px 7px; border-radius: 6px;
    background: var(--bg-strong); border: 1px solid var(--line-soft); color: var(--ink-600);
  }
  .act:hover:not(:disabled) { background: var(--bg-elevated); border-color: var(--line-strong); }
  .act:disabled { opacity: 0.6; cursor: default; }
  .act.run { background: #eef2ff; border-color: #c7d2fe; color: #4338ca; }
  .act.run:hover:not(:disabled) { background: #e0e7ff; }
  .code {
    margin: 0; padding: 0.6rem 0.75rem; background: #1e1e2e; color: #cdd6f4;
    font-size: 0.76rem; line-height: 1.5; overflow-x: auto;
    font-family: 'SF Mono', ui-monospace, monospace; white-space: pre-wrap; word-break: break-word;
  }
  .code code { background: none; padding: 0; }
  .result { padding: 0 0.55rem 0.55rem; }

  :global(:is([data-theme="dark"], .dark)) .act.run { background: rgba(99,102,241,0.2); border-color: rgba(99,102,241,0.3); color: #a5b4fc; }
  :global(:is([data-theme="dark"], .dark)) .act.run:hover:not(:disabled) { background: rgba(99,102,241,0.28); }
</style>
