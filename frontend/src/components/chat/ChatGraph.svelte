<script>
  // Node-link view of a CONSTRUCT / DESCRIBE result in a chat answer, drawn by
  // the app's shared cytoscape GraphCanvas (the same one the SPARQL workspace's
  // Graph tab uses). Elements come from chatViz.graphFromTable/graphFromBindings
  // — only the returned triples, capped at MAX_GRAPH_NODES. GraphCanvas pulls in
  // cytoscape, so it is imported on demand: answers without a graph never load it.
  import { t } from 'svelte-i18n';
  import { navigate } from '../../lib/router/index.js';
  import { Share2 } from 'lucide-svelte';

  /** @type {Array<{data: Record<string, unknown>}>} */
  export let nodes = [];
  /** @type {Array<{data: Record<string, unknown>}>} */
  export let edges = [];
  /** Triples in the result (may exceed what the capped graph draws). */
  export let total = 0;

  const graphCanvas = () => import('../GraphCanvas.svelte');

  function open(e) {
    const iri = e.detail?.fullIri;
    if (iri && /^(https?:\/\/|urn:)/i.test(iri)) navigate(`/resource?iri=${encodeURIComponent(iri)}`);
  }
</script>

<div class="graph-block">
  {#if nodes.length}
    <div class="stage" data-testid="chat-graph">
      {#await graphCanvas()}
        <div class="placeholder">
          {$t('components.chat.preparingWidget', { values: { label: $t('components.chat.views.graph') } })}
        </div>
      {:then mod}
        <svelte:component this={mod.default} {nodes} {edges} height="320px" on:nodeOpen={open} />
      {:catch}
        <div class="placeholder">{$t('components.chat.graphFailed')}</div>
      {/await}
    </div>
    <p class="note">
      <Share2 size={11} />
      {$t('components.chat.graphCounts', { values: { nodes: nodes.length, edges: edges.length } })}
      {#if total > edges.length}· {$t('components.chat.graphCapped', { values: { count: total } })}{/if}
    </p>
  {:else}
    <div class="empty">{$t('components.chat.graphEmpty')}</div>
  {/if}
</div>

<style>
  .graph-block { margin: 0 0 0.55rem; }
  .stage {
    height: 320px; border-radius: 10px; overflow: hidden;
    border: 1px solid var(--line-soft, #e5e7eb); background: var(--bg-strong);
  }
  .placeholder {
    height: 100%; display: flex; align-items: center; justify-content: center;
    font-size: 0.78rem; color: var(--ink-400); background: var(--bg-soft, #f1f5f9);
  }
  .note {
    display: flex; align-items: center; gap: 0.3rem;
    margin: 0.3rem 0 0; font-size: 0.72rem; color: var(--ink-400);
  }
  .empty {
    padding: 0.75rem; border: 1px dashed var(--line-strong); border-radius: 10px;
    font-size: 0.78rem; color: var(--ink-400);
  }
</style>
