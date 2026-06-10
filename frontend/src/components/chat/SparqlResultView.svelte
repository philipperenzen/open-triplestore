<script>
  // Shared result renderer for the chat's runnable blocks (SPARQL + API runs).
  // Takes a result normalized by chatRich.normalizeSparqlResult():
  //   bindings → the app-wide DataTable (RdfTerm cells, IRI navigation)
  //   boolean  → an ASK badge
  //   graph    → highlighted N-Triples
  import { t } from 'svelte-i18n';
  import DataTable from '../DataTable.svelte';
  import { highlightRdf } from '../../lib/resultHighlight.js';
  import { CheckCircle2, XCircle } from 'lucide-svelte';

  export let result = null;
  export let error = null;
  /** Cap rendered rows/lines so a huge result can't stall the chat. */
  export let maxRows = 100;

  $: bindings = result?.kind === 'bindings' ? result.bindings : [];
  $: shownBindings = bindings.slice(0, maxRows);
  $: graphLines = result?.kind === 'graph' ? result.ntriples.split('\n').filter((l) => l.trim()) : [];
  $: shownGraph = graphLines.slice(0, maxRows).join('\n');
</script>

{#if error}
  <div class="run-error" role="alert">{error}</div>
{:else if result?.kind === 'boolean'}
  <div class="ask-result" class:no={!result.value}>
    {#if result.value}<CheckCircle2 size={14} />{:else}<XCircle size={14} />{/if}
    <span>{$t('components.chat.askResult')}: <strong>{result.value ? $t('components.chat.yes') : $t('components.chat.no')}</strong></span>
  </div>
{:else if result?.kind === 'graph'}
  <!-- highlightRdf HTML-escapes all source text (resultHighlight.js), so {@html} is safe. -->
  <!-- eslint-disable-next-line svelte/no-at-html-tags -->
  <pre class="graph-result"><code>{@html highlightRdf(shownGraph)}</code></pre>
  <p class="result-note">
    {$t('components.chat.graphTriples', { values: { count: graphLines.length } })}
    {#if graphLines.length > maxRows}· {$t('components.chat.showingFirst', { values: { count: maxRows } })}{/if}
  </p>
{:else if result?.kind === 'bindings'}
  <div class="table-host">
    <DataTable mode="bindings" vars={result.vars} bindings={shownBindings} maxHeight="300px" emptyText={$t('components.chat.noRows')} />
  </div>
  <p class="result-note">
    {$t('components.chat.rowCount', { values: { count: bindings.length } })}
    {#if bindings.length > maxRows}· {$t('components.chat.showingFirst', { values: { count: maxRows } })}{/if}
  </p>
{/if}

<style>
  .run-error {
    margin-top: 0.45rem; padding: 0.45rem 0.6rem; border-radius: 8px; font-size: 0.78rem;
    background: #fff8f8; border: 1px solid #f3c9c9; color: #b91c1c;
    white-space: pre-wrap; word-break: break-word;
  }
  .ask-result {
    margin-top: 0.45rem; display: inline-flex; align-items: center; gap: 0.4rem;
    padding: 0.3rem 0.6rem; border-radius: 8px; font-size: 0.8rem;
    background: #ecfdf5; border: 1px solid #bbf7d0; color: #15803d;
  }
  .ask-result.no { background: #fff7ed; border-color: #fed7aa; color: #c2410c; }
  .graph-result {
    margin: 0.45rem 0 0; padding: 0.55rem 0.7rem; background: #1e1e2e; color: #cdd6f4;
    border-radius: 8px; font-size: 0.74rem; line-height: 1.5; overflow: auto; max-height: 280px;
  }
  .graph-result code { background: none; padding: 0; font-family: 'SF Mono', ui-monospace, monospace; }
  .table-host { margin-top: 0.45rem; border: 1px solid var(--line-soft); border-radius: 8px; overflow: hidden; }
  .result-note { margin: 0.3rem 0 0; font-size: 0.72rem; color: var(--ink-400); }

  :global(:is([data-theme="dark"], .dark)) .run-error { background: rgba(220,38,38,0.12); border-color: rgba(220,38,38,0.35); color: #fca5a5; }
  :global(:is([data-theme="dark"], .dark)) .ask-result { background: rgba(16,185,129,0.15); border-color: rgba(16,185,129,0.3); color: #6ee7b7; }
  :global(:is([data-theme="dark"], .dark)) .ask-result.no { background: rgba(245,158,11,0.15); border-color: rgba(245,158,11,0.3); color: #fcd34d; }
</style>
