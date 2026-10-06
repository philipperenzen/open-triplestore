<script lang="ts">
  // The OWL 2 QL rewriting of a SPARQL query (POST /api/reasoning/rewrite):
  // what the query becomes once the TBox the caller may read is unfolded
  // into it. Shown, not run.
  import { t } from 'svelte-i18n';
  import { Copy, Loader2, Wand2 } from 'lucide-svelte';
  import { rewriteQuery } from '../../lib/api';
  import { copyOrWarn } from '../../lib/clipboard';
  import { toastSuccess } from '../../lib/toast';
  import './reasoning.css';

  let query = 'SELECT ?x WHERE {\n  ?x a <https://example.org/Asset> .\n}';
  let rewritten = '';
  let running = false;
  let error = '';

  async function run() {
    running = true;
    error = '';
    rewritten = '';
    try {
      rewritten = (await rewriteQuery(query, 'owl2-ql')).rewritten;
    } catch (e) {
      error = (e as Error)?.message || String(e);
    } finally {
      running = false;
    }
  }

  async function copy() {
    if (await copyOrWarn(rewritten)) toastSuccess($t('components.reasoning.copied'));
  }
</script>

<div class="card rz-card" id="rewrite">
  <div class="rz-head">
    <div class="rz-head-left"><Wand2 size={15} /><h3>{$t('components.reasoning.rewriteHeading')}</h3></div>
  </div>
  <p class="rz-intro">{$t('components.reasoning.rewriteIntro')}</p>
  <div class="rz-field">
    <label for="ql-query">{$t('components.reasoning.rewriteQuery')}</label>
    <textarea id="ql-query" class="rz-code" bind:value={query} disabled={running}></textarea>
  </div>
  <div class="rz-row">
    <button class="btn btn-sm" on:click={run} disabled={running || !query.trim()}>
      {#if running}<Loader2 size={13} class="animate-spin" />{/if}
      {$t('components.reasoning.rewrite')}
    </button>
  </div>
  {#if error}<p class="rz-msg rz-msg-bad" role="alert">{error}</p>{/if}
  {#if rewritten}
    <div class="rz-field">
      <div class="rz-row">
        <span class="rz-label">{$t('components.reasoning.rewritten')}</span>
        <button class="btn btn-sm btn-ghost" on:click={copy} aria-label={$t('components.reasoning.copy')}><Copy size={13} /></button>
      </div>
      <pre class="rz-pre" data-testid="rewritten-query">{rewritten}</pre>
    </div>
  {/if}
</div>
