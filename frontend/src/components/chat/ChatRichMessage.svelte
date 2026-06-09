<script>
  // Renders one assistant chat answer as interleaved markdown and live widgets
  // (see lib/chatRich.js for the block grammar). Markdown runs are rendered with
  // the shared renderMarkdown (marked + DOMPurify + RDF/SPARQL highlighting),
  // then inline `GET /api/...` codes are decorated into run buttons whose clicks
  // bubble up as a `runApi` event — the chat page attaches the actual run panel.
  import { createEventDispatcher } from 'svelte';
  import { t } from 'svelte-i18n';
  import { Loader2 } from 'lucide-svelte';
  import { renderMarkdown } from '../../lib/markdown.js';
  import { parseChatBlocks, reuseSegments, decorateApiLinks } from '../../lib/chatRich.js';
  import SparqlRunBlock from './SparqlRunBlock.svelte';
  import ApiRunBlock from './ApiRunBlock.svelte';
  import ChatChart from './ChatChart.svelte';
  import ChatMap from './ChatMap.svelte';
  import ChatInfoCard from './ChatInfoCard.svelte';
  import CsvPreview from './CsvPreview.svelte';

  export let content = '';
  /** True while the message is still arriving token by token: unclosed widget
   *  fences render as a pending placeholder instead of half-parsed widgets. */
  export let streaming = false;

  const dispatch = createEventDispatcher();

  // Streamed messages re-parse on every delta; reuseSegments keeps the object
  // identity of unchanged segments so settled widgets (charts, maps) don't
  // re-render, and the WeakMap below makes unchanged markdown runs free.
  let prevSegments = [];
  $: segments = trackSegments(content, streaming);
  function trackSegments(text, isStreaming) {
    prevSegments = reuseSegments(prevSegments, parseChatBlocks(text, { streaming: isStreaming }));
    return prevSegments;
  }

  const htmlCache = new WeakMap();
  function mdHtml(seg) {
    let html = htmlCache.get(seg);
    if (html === undefined) {
      html = decorateApiLinks(renderMarkdown(seg.source, { breaks: true }).html);
      htmlCache.set(seg, html);
    }
    return html;
  }

  function pendingLabel(label) {
    switch (label) {
      case 'sparql': return $t('components.chat.sparqlTitle');
      case 'api': return $t('components.chat.apiTitle');
      case 'chart': return $t('components.chat.pendingChart');
      case 'map': return $t('components.chat.pendingMap');
      case 'card': return $t('components.chat.pendingCard');
      case 'csv': return $t('components.chat.pendingTable');
      default: return $t('components.chat.pendingContent');
    }
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
    <div class="md-seg" on:click={onClick} on:keydown={onKeydown}>{@html mdHtml(seg)}</div>
  {:else if seg.kind === 'pending'}
    <div class="pending" role="status">
      <Loader2 size={13} class="spin" />
      <span>{$t('components.chat.preparingWidget', { values: { label: pendingLabel(seg.label) } })}</span>
    </div>
  {:else if seg.kind === 'sparql'}
    <SparqlRunBlock code={seg.code} on:openInSparql />
  {:else if seg.kind === 'api'}
    <ApiRunBlock method={seg.method} path={seg.path} />
  {:else if seg.kind === 'chart'}
    <ChatChart spec={seg.spec} />
  {:else if seg.kind === 'map'}
    <ChatMap features={seg.features} />
  {:else if seg.kind === 'card'}
    <ChatInfoCard card={seg.card} />
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
  /* A widget block still being generated (streaming): calm shimmer, fixed-ish
     height so the bubble doesn't jump when the real widget replaces it. */
  .pending {
    display: flex; align-items: center; gap: 0.45rem;
    margin: 0 0 0.55rem; padding: 0.55rem 0.75rem;
    border: 1px dashed var(--line-strong, rgba(15,32,39,0.16));
    border-radius: 10px; color: var(--ink-400); font-size: 0.78rem; font-style: italic;
    background: linear-gradient(100deg, transparent 30%, rgba(109,74,217,0.07) 50%, transparent 70%)
      var(--bg-soft, rgba(15,32,39,0.03));
    background-size: 220% 100%;
    animation: pending-sheen 1.6s linear infinite;
  }
  @keyframes pending-sheen { from { background-position: 120% 0; } to { background-position: -100% 0; } }
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
