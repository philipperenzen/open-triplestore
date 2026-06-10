<script>
  import { onMount, tick } from 'svelte';
  import { t } from 'svelte-i18n';
  import { llmChat, llmHealth, sendLlmFeedback } from '../lib/api.js';
  import { navigate } from '../lib/router/index.js';
  import { isAuthenticated } from '../lib/stores.js';
  import { renderMarkdown, highlightSparql } from '../lib/markdown.js';
  import ChatRichMessage from '../components/chat/ChatRichMessage.svelte';
  import ApiRunBlock from '../components/chat/ApiRunBlock.svelte';
  import CsvPreview from '../components/chat/CsvPreview.svelte';
  import {
    Sparkles, Send, ThumbsUp, ThumbsDown, Loader2,
    Terminal, AlertTriangle, ChevronDown, ChevronRight, Database,
  } from 'lucide-svelte';

  // One assistant turn carries the retrieval trail (every SPARQL round the
  // backend ran, ok or failed) plus any API runs the user clicked open.
  let messages = []; // { role, content, queries?: [{sparql, ok, error?, columns?, rows?, truncated}], ranQuery?, showQuery?, reviewed?, isError?, runs?: [{id, method, path}] }
  let input = '';
  let loading = false;
  let llmStatus = null;
  let scrollEl;
  let runSeq = 0;

  $: EXAMPLES = [
    $t('pages.llmChat.example1'),
    $t('pages.llmChat.example2'),
    $t('pages.llmChat.example3'),
    $t('pages.llmChat.example4'),
    $t('pages.llmChat.example5'),
  ];

  $: offline = llmStatus && !llmStatus.reachable;

  onMount(() => {
    llmHealth().then((s) => { llmStatus = s; }).catch(() => {});
  });

  async function scrollToBottom() {
    await tick();
    if (scrollEl) scrollEl.scrollTop = scrollEl.scrollHeight;
  }

  async function send(text) {
    const content = (text ?? input).trim();
    if (!content || loading) return;
    input = '';
    messages = [...messages, { role: 'user', content }];
    loading = true;
    await scrollToBottom();

    // Transport-error bubbles (isError) are UI-only — never replay them as
    // assistant turns in the model conversation.
    const wire = messages.filter((m) => !m.isError).map((m) => ({ role: m.role, content: m.content }));
    try {
      const resp = await llmChat(wire);
      messages = [...messages, {
        role: 'assistant',
        content: resp.answer || $t('pages.llmChat.noAnswer'),
        queries: normalizeQueries(resp),
        ranQuery: !!resp.ran_query,
        showQuery: false,
        reviewed: null,
        runs: [],
      }];
    } catch (e) {
      messages = [...messages, {
        role: 'assistant',
        isError: true,
        content: e?.message || $t('pages.llmChat.unavailable'),
      }];
    } finally {
      loading = false;
      await scrollToBottom();
    }
  }

  function onKeydown(e) {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      send();
    }
  }

  // The backend's `queries` array is the full retrieval trail (it always
  // accompanies the legacy sparql/columns/rows fields when a query ran).
  function normalizeQueries(resp) {
    return resp.queries ?? [];
  }

  // The user clicked an inline `GET /api/...` in the answer: attach a run panel
  // (which auto-runs) under that message. One panel per distinct path.
  function attachRun(msg, ep) {
    if (!ep?.path) return;
    msg.runs = msg.runs || [];
    if (msg.runs.some((r) => r.path === ep.path)) return;
    msg.runs = [...msg.runs, { id: ++runSeq, method: ep.method || 'GET', path: ep.path }];
    messages = messages;
  }

  function lastUserQuestion(idx) {
    for (let i = idx - 1; i >= 0; i--) {
      if (messages[i].role === 'user') return messages[i].content;
    }
    return null;
  }

  // Best-effort training feedback, routed to the gateway's sparql track (the one
  // OpenTripleStore consumes), mirroring the SPARQL editor's review signal.
  function review(msg, idx, rating) {
    if (msg.reviewed === rating) return;
    msg.reviewed = rating;
    messages = messages;
    // Prefer the last *successful* query in the trail (the one the answer is
    // based on); only fall back to the last attempt if none succeeded.
    const trail = msg.queries || [];
    const best = [...trail].reverse().find((q) => q.ok) || trail[trail.length - 1] || null;
    const lastSparql = best ? best.sparql : null;
    sendLlmFeedback({
      track: 'sparql',
      event: 'chat',
      input: { nl_question: lastUserQuestion(idx) },
      output: { answer: msg.content, corrected_turtle: lastSparql },
      label: { decision: rating === 'down' ? 'reject' : null, rating, source: 'human', comment: null },
      prov: { app: 'opentriplestore' },
    });
  }

  // Hand the generated query off to the SPARQL workspace (read on mount there).
  function openInSparql(sparql) {
    try { sessionStorage.setItem('ots_sparql_load', sparql); } catch {}
    navigate('/sparql');
  }

  function clearChat() {
    messages = [];
  }

  // Render the assistant's markdown — including fenced code blocks, which are
  // syntax-highlighted for SPARQL/Turtle/JSON/XML. renderMarkdown sanitizes the
  // output with DOMPurify, so model output still cannot inject markup.
  function renderRich(text) {
    return renderMarkdown(text || '', { breaks: true }).html;
  }
</script>

<div class="chat-page">
  <div class="chat-head-actions">
    {#if llmStatus}
      <span class="llm-badge" class:offline title={llmStatus.reachable ? $t('pages.llmChat.badgeOnlineTitle', { values: { gateway: llmStatus.gateway } }) : $t('pages.llmChat.badgeOfflineTitle')}>
        {llmStatus.reachable ? $t('pages.llmChat.badgeOnline') : $t('pages.llmChat.badgeOffline')}
      </span>
    {/if}
    {#if messages.length}
      <button class="btn-clear" on:click={clearChat}>{$t('pages.llmChat.clearChat')}</button>
    {/if}
  </div>

  {#if offline}
    <div class="offline-banner">
      <AlertTriangle size={15} />
      <span>{$t('pages.llmChat.offlineBanner')}</span>
    </div>
  {/if}

  <div class="messages" bind:this={scrollEl}>
    {#if messages.length === 0}
      <div class="welcome">
        <div class="welcome-icon"><Sparkles size={26} /></div>
        <h2>{$t('pages.llmChat.welcomeTitle')}</h2>
        <p>{$t('pages.llmChat.welcomeIntro')}</p>
        <div class="examples">
          {#each EXAMPLES as ex}
            <button class="example" on:click={() => send(ex)} disabled={offline}>{ex}</button>
          {/each}
        </div>
        {#if !$isAuthenticated}
          <p class="anon-note">{$t('pages.llmChat.guestNote')}</p>
        {/if}
      </div>
    {/if}

    {#each messages as msg, i}
      <div class="row {msg.role}">
        <div class="bubble {msg.role}" class:error={msg.isError}>
          {#if msg.role === 'assistant' && !msg.isError}
            <!-- Interactive answer: markdown plus runnable sparql/api blocks and
                 chart/map/card/csv widgets (sanitized in ChatRichMessage). -->
            <div class="bubble-text">
              <ChatRichMessage
                content={msg.content}
                on:runApi={(e) => attachRun(msg, e.detail)}
                on:openInSparql={(e) => openInSparql(e.detail)}
              />
            </div>
          {:else}
            <!-- renderRich() renders markdown and sanitizes it with DOMPurify -->
            <!-- eslint-disable-next-line svelte/no-at-html-tags -->
            <div class="bubble-text">{@html renderRich(msg.content)}</div>
          {/if}

          {#if msg.runs?.length}
            <div class="attached-runs">
              {#each msg.runs as r (r.id)}
                <ApiRunBlock method={r.method} path={r.path} autorun />
              {/each}
            </div>
          {/if}

          {#if msg.queries?.length}
            <div class="query-block">
              <button class="query-toggle" on:click={() => { msg.showQuery = !msg.showQuery; messages = messages; }}>
                {#if msg.showQuery}<ChevronDown size={14} />{:else}<ChevronRight size={14} />{/if}
                <Terminal size={13} />
                {#if msg.queries.length > 1}
                  {$t('pages.llmChat.ranQueries', { values: { count: msg.queries.length } })}
                {:else}
                  {msg.ranQuery ? $t('pages.llmChat.ranQuery') : $t('pages.llmChat.attemptedQuery')}
                {/if}
                {#if msg.queries.length === 1 && msg.queries[0].rows}
                  <span class="row-count">· {msg.queries[0].rows.length}{msg.queries[0].truncated ? '+' : ''} {msg.queries[0].rows.length === 1 ? $t('pages.llmChat.rowSingular') : $t('pages.llmChat.rowPlural')}</span>
                {/if}
              </button>
              {#if msg.showQuery}
                {#each msg.queries as q, qi}
                  <div class="query-item">
                    {#if msg.queries.length > 1}
                      <p class="query-label">
                        {$t('pages.llmChat.queryN', { values: { n: qi + 1 } })}
                        {#if q.rows}<span class="row-count">· {q.rows.length}{q.truncated ? '+' : ''} {q.rows.length === 1 ? $t('pages.llmChat.rowSingular') : $t('pages.llmChat.rowPlural')}</span>{/if}
                      </p>
                    {/if}
                    <!-- highlightSparql escapes all source text (resultHighlight.js) -->
                    <!-- eslint-disable-next-line svelte/no-at-html-tags -->
                    <pre class="query-text"><code>{@html highlightSparql(q.sparql)}</code></pre>
                    {#if q.error}<p class="query-error">{q.error}</p>{/if}
                    <button class="open-sparql" on:click={() => openInSparql(q.sparql)}>
                      <Terminal size={12} /> {$t('pages.llmChat.openInSparql')}
                    </button>
                    {#if q.columns && q.rows && q.rows.length}
                      <CsvPreview columns={q.columns} rows={q.rows} framed={false} downloadable={false} />
                      {#if q.truncated}<p class="truncated-note">{$t('pages.llmChat.showingFirstRows', { values: { count: q.rows.length } })}</p>{/if}
                    {/if}
                  </div>
                {/each}
              {/if}
            </div>
          {/if}

          {#if msg.role === 'assistant' && !msg.isError}
            <div class="feedback">
              <span class="feedback-label">{$t('pages.llmChat.helpful')}</span>
              <button class="thumb" class:up={msg.reviewed === 'up'} on:click={() => review(msg, i, 'up')} aria-label={$t('pages.llmChat.helpfulYes')}><ThumbsUp size={13} /></button>
              <button class="thumb" class:down={msg.reviewed === 'down'} on:click={() => review(msg, i, 'down')} aria-label={$t('pages.llmChat.helpfulNo')}><ThumbsDown size={13} /></button>
            </div>
          {/if}
        </div>
      </div>
    {/each}

    {#if loading}
      <div class="row assistant">
        <div class="bubble assistant thinking">
          <Loader2 size={15} class="spin" /> {$t('pages.llmChat.thinking')}
        </div>
      </div>
    {/if}
  </div>

  <form class="composer" on:submit|preventDefault={() => send()}>
    <textarea
      class="composer-input"
      bind:value={input}
      on:keydown={onKeydown}
      rows="1"
      placeholder={offline ? $t('pages.llmChat.composerOffline') : $t('pages.llmChat.composerPlaceholder')}
      disabled={offline || loading}
    ></textarea>
    <button class="send-btn" type="submit" disabled={offline || loading || !input.trim()} aria-label={$t('pages.llmChat.send')}>
      {#if loading}<Loader2 size={16} class="spin" />{:else}<Send size={16} />{/if}
    </button>
  </form>
  <p class="disclaimer"><Database size={11} /> {$t('pages.llmChat.disclaimer')}</p>
</div>

<style>
  .chat-page {
    max-width: 880px;
    margin: 0 auto;
    padding: 0.5rem 0.25rem 1rem;
    display: flex;
    flex-direction: column;
  }

  .chat-head-actions {
    display: flex; align-items: center; justify-content: flex-end; gap: 0.5rem;
    margin-bottom: 0.5rem; min-height: 1.6rem;
  }

  .llm-badge {
    font-size: 0.7rem; font-weight: 600; color: #16a34a; white-space: nowrap;
    padding: 2px 8px; border-radius: 999px; background: #ecfdf5; border: 1px solid #bbf7d0;
  }
  .llm-badge.offline { color: #b45309; background: #fffbeb; border-color: #fde68a; }
  .btn-clear {
    font-size: 0.78rem; padding: 3px 10px; cursor: pointer; color: var(--ink-600);
    background: var(--bg-strong); border: 1px solid var(--line-soft); border-radius: 8px;
  }
  .btn-clear:hover { background: var(--bg-soft); }

  .offline-banner {
    display: flex; align-items: center; gap: 0.5rem;
    background: #fffbeb; border: 1px solid #fde68a; color: #92400e;
    padding: 0.5rem 0.75rem; border-radius: 8px; font-size: 0.82rem; margin-bottom: 0.75rem;
  }

  .messages {
    flex: 1;
    min-height: 340px;
    max-height: 62vh;
    overflow-y: auto;
    padding: 0.5rem;
    background: var(--bg-elevated, rgba(255,253,250,0.9));
    border: 1px solid var(--line-soft, rgba(15,32,39,0.08));
    border-radius: 14px;
  }

  .welcome { text-align: center; padding: 1.5rem 1rem 1rem; color: var(--ink-600); }
  .welcome-icon {
    width: 52px; height: 52px; margin: 0 auto 0.75rem; border-radius: 50%;
    display: grid; place-items: center; color: #6d4ad9;
    background: linear-gradient(135deg, #ede9fe, #e0f2fe);
  }
  .welcome h2 { margin: 0 0 0.3rem; font-size: 1.1rem; color: var(--ink-800); }
  .welcome p { margin: 0 auto 1rem; font-size: 0.88rem; max-width: 46ch; }
  .examples { display: grid; gap: 0.5rem; max-width: 560px; margin: 0 auto; }
  .example {
    text-align: left; background: var(--bg-strong); border: 1px solid var(--line-soft); border-radius: 10px;
    padding: 0.6rem 0.8rem; font-size: 0.85rem; color: var(--ink-700); cursor: pointer;
    transition: border-color 0.15s, box-shadow 0.15s, transform 0.05s;
  }
  .example:hover:not(:disabled) { border-color: #c4b5fd; box-shadow: 0 1px 6px rgba(109,74,217,0.12); }
  .example:active:not(:disabled) { transform: translateY(1px); }
  .example:disabled { opacity: 0.5; cursor: not-allowed; }
  .anon-note { font-size: 0.78rem; color: var(--ink-400); margin-top: 1rem; }

  .row { display: flex; margin: 0.5rem 0; }
  .row.user { justify-content: flex-end; }
  .row.assistant { justify-content: flex-start; }

  .bubble {
    max-width: 86%;
    padding: 0.6rem 0.85rem;
    border-radius: 14px;
    font-size: 0.9rem;
    line-height: 1.5;
    box-shadow: 0 1px 2px rgba(15,32,39,0.05);
  }
  .bubble.user { background: linear-gradient(135deg, #6d4ad9, #4f46e5); color: #fff; border-bottom-right-radius: 4px; }
  .bubble.assistant { background: var(--bg-strong); border: 1px solid var(--line-soft); color: var(--ink-800); border-bottom-left-radius: 4px; }
  .bubble.error { background: #fff8f8; border-color: #f3c9c9; color: #b91c1c; }
  .bubble.thinking { display: inline-flex; align-items: center; gap: 0.45rem; color: var(--ink-500); font-style: italic; }

  /* Markdown-rendered assistant text. Tight vertical rhythm so blocks sit snugly in
     the bubble; `breaks:true` turns single newlines into <br>, so no pre-wrap. */
  .bubble-text { word-break: break-word; }
  .bubble-text :global(p) { margin: 0 0 0.55rem; }
  .bubble-text :global(ul), .bubble-text :global(ol) { margin: 0 0 0.55rem; padding-left: 1.25rem; }
  .bubble-text :global(li) { margin: 0.12rem 0; }
  .bubble-text :global(h1), .bubble-text :global(h2),
  .bubble-text :global(h3), .bubble-text :global(h4) {
    margin: 0.55rem 0 0.35rem; font-size: 1em; font-weight: 700;
  }
  .bubble-text :global(a) { color: inherit; text-decoration: underline; }
  .bubble-text :global(p:last-child), .bubble-text :global(ul:last-child),
  .bubble-text :global(ol:last-child), .bubble-text :global(pre:last-child) { margin-bottom: 0; }
  .bubble-text :global(code) {
    background: rgba(100,116,139,0.14); padding: 0 4px; border-radius: 4px;
    font-family: 'SF Mono', ui-monospace, monospace; font-size: 0.85em;
  }
  .bubble.user .bubble-text :global(code) { background: rgba(255,255,255,0.22); }
  /* Fenced code blocks: dark panel, syntax-highlighted via resultHighlight.js tokens. */
  .bubble-text :global(pre) {
    background: #1e1e2e; color: #cdd6f4; padding: 0.7rem 0.85rem; border-radius: 0.6rem;
    overflow-x: auto; margin: 0 0 0.55rem; line-height: 1.5;
  }
  .bubble-text :global(pre code) { background: none; padding: 0; color: inherit; font-size: 0.8rem; }
  .bubble-text :global(pre .tok-comment) { color: #7f849c; font-style: italic; }
  .bubble-text :global(pre .tok-iri)     { color: #89b4fa; }
  .bubble-text :global(pre .tok-pname)   { color: #f5c2e7; }
  .bubble-text :global(pre .tok-kw)      { color: #cba6f7; font-weight: 600; }
  .bubble-text :global(pre .tok-str)     { color: #a6e3a1; }
  .bubble-text :global(pre .tok-num)     { color: #fab387; }
  .bubble-text :global(pre .tok-punct)   { color: #9399b2; }
  .bubble-text :global(pre .tok-key)     { color: #89dceb; }
  .bubble-text :global(pre .tok-tag)     { color: #89b4fa; }
  .bubble-text :global(pre .tok-attr)    { color: #f9e2af; }
  .bubble-text :global(pre .tok-meta)    { color: #7f849c; }

  .attached-runs { margin-top: 0.6rem; }
  .attached-runs > :global(* + *) { margin-top: 0.45rem; }

  .query-block { margin-top: 0.6rem; border-top: 1px solid var(--line-soft); padding-top: 0.5rem; }
  .query-toggle {
    display: inline-flex; align-items: center; gap: 0.35rem; background: none; border: none;
    cursor: pointer; color: #4f46e5; font-size: 0.78rem; font-weight: 600; padding: 0;
  }
  .query-toggle:hover { text-decoration: underline; }
  .row-count { color: var(--ink-400); font-weight: 500; }
  .query-item + .query-item { margin-top: 0.7rem; border-top: 1px dashed var(--line-soft); padding-top: 0.55rem; }
  .query-label { margin: 0.4rem 0 0; font-size: 0.74rem; font-weight: 600; color: var(--ink-500); }
  .query-error {
    margin: 0.35rem 0; padding: 0.4rem 0.55rem; border-radius: 8px; font-size: 0.76rem;
    background: #fff8f8; border: 1px solid #f3c9c9; color: #b91c1c;
    white-space: pre-wrap; word-break: break-word;
  }
  .query-text {
    margin: 0.5rem 0; padding: 0.5rem 0.6rem; background: #0f172a; color: #e2e8f0;
    border-radius: 8px; font-family: 'SF Mono', ui-monospace, monospace; font-size: 0.76rem;
    white-space: pre-wrap; word-break: break-word; overflow-x: auto;
  }
  .query-text code { background: none; padding: 0; font-family: inherit; font-size: inherit; }
  .query-text :global(.tok-comment) { color: #7f849c; font-style: italic; }
  .query-text :global(.tok-iri)     { color: #89b4fa; }
  .query-text :global(.tok-pname)   { color: #f5c2e7; }
  .query-text :global(.tok-kw)      { color: #cba6f7; font-weight: 600; }
  .query-text :global(.tok-str)     { color: #a6e3a1; }
  .query-text :global(.tok-num)     { color: #fab387; }
  .query-text :global(.tok-punct)   { color: #9399b2; }
  .open-sparql {
    display: inline-flex; align-items: center; gap: 0.3rem; background: #eef2ff; color: #4338ca;
    border: 1px solid #c7d2fe; border-radius: 6px; padding: 3px 8px; font-size: 0.74rem; cursor: pointer;
  }
  .open-sparql:hover { background: #e0e7ff; }

  .truncated-note { font-size: 0.72rem; color: var(--ink-400); margin: 0.35rem 0 0; }

  .feedback { display: flex; align-items: center; gap: 0.35rem; margin-top: 0.55rem; }
  .feedback-label { font-size: 0.72rem; color: var(--ink-400); margin-right: 0.1rem; }
  .thumb {
    display: inline-flex; align-items: center; justify-content: center;
    background: var(--bg-soft); border: 1px solid var(--line-soft); border-radius: 6px; cursor: pointer;
    color: var(--ink-500); padding: 3px 6px;
  }
  .thumb:hover { background: var(--line-strong); }
  .thumb.up { background: #16a34a; border-color: #16a34a; color: #fff; }
  .thumb.down { background: #dc2626; border-color: #dc2626; color: #fff; }

  .composer {
    display: flex; align-items: flex-end; gap: 0.5rem; margin-top: 0.75rem;
    background: var(--bg-strong); border: 1px solid var(--line-strong); border-radius: 14px; padding: 0.5rem 0.5rem 0.5rem 0.85rem;
  }
  .composer:focus-within { border-color: #a5b4fc; box-shadow: 0 0 0 3px rgba(99,102,241,0.12); }
  .composer-input {
    flex: 1; border: none; outline: none; resize: none; font: inherit; font-size: 0.92rem;
    line-height: 1.4; max-height: 160px; background: transparent; color: var(--ink-800);
  }
  .composer-input:disabled { color: var(--ink-400); }
  .send-btn {
    flex-shrink: 0; width: 38px; height: 38px; border-radius: 11px; border: none; cursor: pointer;
    display: grid; place-items: center; color: #fff;
    background: linear-gradient(135deg, #7c5cff, #4f46e5);
    box-shadow: 0 2px 6px rgba(79,70,229,0.32), inset 0 1px 0 rgba(255,255,255,0.18);
    transition: transform 0.12s ease, box-shadow 0.18s ease, filter 0.18s ease, opacity 0.18s ease;
  }
  .send-btn :global(svg) { transition: transform 0.18s ease; }
  .send-btn:hover:not(:disabled) {
    transform: translateY(-1px);
    box-shadow: 0 4px 14px rgba(79,70,229,0.42), inset 0 1px 0 rgba(255,255,255,0.25);
    filter: brightness(1.06);
  }
  .send-btn:hover:not(:disabled) :global(svg) { transform: translateX(1px) rotate(-8deg); }
  .send-btn:active:not(:disabled) {
    transform: translateY(0) scale(0.95);
    box-shadow: 0 1px 4px rgba(79,70,229,0.3), inset 0 1px 0 rgba(255,255,255,0.15);
  }
  .send-btn:focus-visible { outline: none; box-shadow: 0 0 0 3px rgba(99,102,241,0.35), 0 2px 6px rgba(79,70,229,0.32); }
  .send-btn:disabled {
    opacity: 0.4; cursor: not-allowed; box-shadow: none;
    background: linear-gradient(135deg, #b8b2d6, #a5a3c4);
  }

  .disclaimer {
    display: flex; align-items: center; gap: 0.35rem; justify-content: center;
    font-size: 0.72rem; color: var(--ink-400); margin: 0.5rem 0 0;
  }

  :global(.spin) { animation: spin 0.9s linear infinite; }
  @keyframes spin { to { transform: rotate(360deg); } }

  :global(:is([data-theme="dark"], .dark)) .llm-badge { background: rgba(16,185,129,0.18); border-color: rgba(16,185,129,0.3); color: #6ee7b7; }
  :global(:is([data-theme="dark"], .dark)) .llm-badge.offline { background: rgba(245,158,11,0.18); border-color: rgba(245,158,11,0.3); color: #fcd34d; }
  :global(:is([data-theme="dark"], .dark)) .offline-banner { background: rgba(245,158,11,0.12); border-color: rgba(245,158,11,0.35); color: #fcd34d; }
  :global(:is([data-theme="dark"], .dark)) .welcome-icon { background: rgba(139,92,246,0.2); color: #c4b5fd; }
  :global(:is([data-theme="dark"], .dark)) .bubble.error { background: rgba(220,38,38,0.12); border-color: rgba(220,38,38,0.35); color: #fca5a5; }
  :global(:is([data-theme="dark"], .dark)) .query-toggle { color: #a5b4fc; }
  :global(:is([data-theme="dark"], .dark)) .query-error { background: rgba(220,38,38,0.12); border-color: rgba(220,38,38,0.35); color: #fca5a5; }
  :global(:is([data-theme="dark"], .dark)) .open-sparql { background: rgba(99,102,241,0.2); border-color: rgba(99,102,241,0.3); color: #a5b4fc; }
  :global(:is([data-theme="dark"], .dark)) .open-sparql:hover { background: rgba(99,102,241,0.28); }
</style>
