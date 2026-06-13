<script>
  // Renders one assistant chat answer as interleaved markdown and live widgets
  // (see lib/chatRich.js for the block grammar). Markdown runs are rendered with
  // the shared renderMarkdown (marked + DOMPurify + RDF/SPARQL highlighting),
  // then inline `GET /api/...` codes are decorated into run buttons whose clicks
  // bubble up as a `runApi` event — the chat page attaches the actual run panel.
  import { createEventDispatcher } from 'svelte';
  import { t } from 'svelte-i18n';
  import { renderMarkdown } from '../../lib/markdown.js';
  import { parseChatBlocks, decorateApiLinks } from '../../lib/chatRich.js';
  import SparqlRunBlock from './SparqlRunBlock.svelte';
  import ApiRunBlock from './ApiRunBlock.svelte';
  import ChatChart from './ChatChart.svelte';
  import ChatMap from './ChatMap.svelte';
  import ChatInfoCard from './ChatInfoCard.svelte';
  import ChatModel3D from './ChatModel3D.svelte';
  import ChatFileCard from './ChatFileCard.svelte';
  import CsvPreview from './CsvPreview.svelte';

  export let content = '';

  const dispatch = createEventDispatcher();

  $: segments = parseChatBlocks(content);

  function mdHtml(src) {
    return decorateApiLinks(renderMarkdown(src, { breaks: true }).html);
  }

  function apiLinkFrom(e) {
    const el = e.target?.closest?.('.chat-api-link');
    return el ? { method: el.dataset.method || 'GET', path: el.dataset.path || '' } : null;
  }
  function onClick(e) {
    const ep = apiLinkFrom(e);
    if (ep?.path) dispatch('runApi', ep);
  }
  function onKeydown(e) {
    if (e.key !== 'Enter' && e.key !== ' ') return;
    const ep = apiLinkFrom(e);
    if (ep?.path) {
      e.preventDefault();
      dispatch('runApi', ep);
    }
  }
</script>

{#each segments as seg}
  {#if seg.kind === 'md'}
    <!-- renderMarkdown sanitizes with DOMPurify; decorateApiLinks only adds attributes. -->
    <!-- svelte-ignore a11y-no-static-element-interactions -->
    <!-- eslint-disable-next-line svelte/no-at-html-tags -->
    <div class="md-seg" on:click={onClick} on:keydown={onKeydown}>{@html mdHtml(seg.source)}</div>
  {:else if seg.kind === 'sparql'}
    <SparqlRunBlock code={seg.code} on:openInSparql />
  {:else if seg.kind === 'api'}
    <ApiRunBlock method={seg.method} path={seg.path} />
  {:else if seg.kind === 'chart'}
    <ChatChart spec={seg.spec} />
  {:else if seg.kind === 'map'}
    <ChatMap features={seg.features} models={seg.models || []} />
  {:else if seg.kind === 'card'}
    <ChatInfoCard card={seg.card} />
  {:else if seg.kind === 'model3d'}
    <ChatModel3D models={seg.models} />
  {:else if seg.kind === 'file'}
    <ChatFileCard file={seg.file} />
  {:else if seg.kind === 'csv'}
    <CsvPreview columns={seg.columns} rows={seg.rows} raw={seg.raw} />
  {:else if seg.kind === 'broken'}
    <div class="broken">
      <p class="broken-note">{$t('components.chat.brokenBlock', { values: { label: seg.label } })}</p>
      <pre class="broken-raw"><code>{seg.raw}</code></pre>
    </div>
  {/if}
{/each}

<style>
  .md-seg { word-break: break-word; }
  /* Inline `GET /api/...` codes decorated by decorateApiLinks() — make them read
     as clickable chips inside the prose. */
  .md-seg :global(code.chat-api-link) {
    cursor: pointer;
    color: #047857;
    background: #ecfdf5;
    border: 1px solid #a7f3d0;
    padding: 0 6px;
    border-radius: 6px;
    transition: background 0.12s, border-color 0.12s;
  }
  .md-seg :global(code.chat-api-link:hover),
  .md-seg :global(code.chat-api-link:focus-visible) {
    background: #d1fae5; border-color: #6ee7b7; outline: none;
  }
  .md-seg :global(code.chat-api-link)::after { content: ' ▸'; font-size: 0.85em; }
  .broken { margin: 0 0 0.55rem; }
  .broken-note { margin: 0 0 0.25rem; font-size: 0.72rem; color: var(--ink-400); font-style: italic; }
  .broken-raw {
    margin: 0; padding: 0.55rem 0.7rem; background: #1e1e2e; color: #cdd6f4;
    border-radius: 8px; font-size: 0.74rem; overflow-x: auto;
    font-family: 'SF Mono', ui-monospace, monospace; white-space: pre-wrap; word-break: break-word;
  }
  :global(:is([data-theme="dark"], .dark)) .md-seg :global(code.chat-api-link) {
    color: #6ee7b7; background: rgba(16,185,129,0.14); border-color: rgba(16,185,129,0.3);
  }
  :global(:is([data-theme="dark"], .dark)) .md-seg :global(code.chat-api-link:hover) {
    background: rgba(16,185,129,0.24);
  }
</style>
