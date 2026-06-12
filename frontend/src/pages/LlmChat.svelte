<script>
  import { onMount, onDestroy, tick } from 'svelte';
  import { t } from 'svelte-i18n';
  import {
    llmChat, llmChatStream, llmHealth, sendLlmFeedback,
    llmConversations, llmCreateConversation, llmGetConversation,
    llmRenameConversation, llmDeleteConversation, llmAppendMessage,
    llmMemory, llmSetMemory,
  } from '../lib/api.js';
  import { navigate } from '../lib/router/index.js';
  import { isAuthenticated } from '../lib/stores.js';
  import { renderMarkdown, highlightSparql } from '../lib/markdown.js';
  import ChatRichMessage from '../components/chat/ChatRichMessage.svelte';
  import ApiRunBlock from '../components/chat/ApiRunBlock.svelte';
  import CsvPreview from '../components/chat/CsvPreview.svelte';
  import ConfirmModal from '../components/ConfirmModal.svelte';
  import {
    Sparkles, Send, ThumbsUp, ThumbsDown, Loader2, Square, Check,
    Terminal, AlertTriangle, ChevronDown, ChevronRight, Database,
    Plus, Pencil, Trash2, Info, NotebookPen, X, MessageSquare,
  } from 'lucide-svelte';

  // One assistant turn carries the retrieval trail (every SPARQL round the
  // backend ran, ok or failed) plus any API runs the user clicked open.
  // While a turn streams, the trail entries are live: {sparql, pending, ok?,
  // rowCount?, truncated?, error?} — replaced by the authoritative trail on done.
  let messages = []; // { role, content, streaming?, stopped?, queries?, ranQuery?, showQuery?, reviewed?, isError?, runs? }
  let input = '';
  let loading = false;
  let llmStatus = null;
  let scrollEl;
  let runSeq = 0;
  let abortCtl = null;
  // Follow the stream only while the user is at the bottom — never yank the
  // scroll position away from someone reading earlier messages.
  let autoScroll = true;

  // ── Chat history (signed-in users only; guests stay ephemeral) ────────────
  let conversations = []; // { id, title, created_at, updated_at, message_count }
  let activeId = null;
  let convLoading = false;
  let renamingId = null;
  let renameText = '';
  let deleteTarget = null; // conversation pending ConfirmModal
  let lastModel = ''; // model id of the latest turn (shown in About)

  // ── Memory + About panels ──────────────────────────────────────────────────
  let aboutOpen = false;
  let memoryOpen = false;
  let memory = { instructions: '', enabled: true };
  let memorySaving = false;
  let memorySaved = false;

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
    if ($isAuthenticated) loadConversations();
  });

  onDestroy(() => abortCtl?.abort());

  // ── History: list / open / new / rename / delete ───────────────────────────

  async function loadConversations() {
    try {
      const r = await llmConversations();
      conversations = r.conversations || [];
    } catch {
      // History is an extra — the chat itself works without it.
    }
  }

  function newChat() {
    if (loading) return;
    activeId = null;
    messages = [];
    input = '';
  }

  async function openConversation(id) {
    if (loading || convLoading || id === activeId) return;
    convLoading = true;
    try {
      const r = await llmGetConversation(id);
      activeId = id;
      messages = (r.messages || []).map((m) => ({
        role: m.role,
        content: m.content,
        queries: m.queries || [],
        ranQuery: (m.queries || []).some((q) => q.ok),
        showQuery: false,
        reviewed: null,
        stopped: !!m.stopped,
        runs: [],
      }));
      if (r.messages?.length) {
        const last = [...r.messages].reverse().find((m) => m.model);
        if (last) lastModel = last.model;
      }
      autoScroll = true;
      await scrollToBottom(true);
    } catch {
      // Conversation may have been deleted elsewhere — refresh the list.
      loadConversations();
    } finally {
      convLoading = false;
    }
  }

  // Persist a finished turn: lazily create the conversation on the first send
  // (the server derives the title from the message), then append both sides.
  // Best-effort — a failed save never disturbs the visible chat.
  async function persistTurn(userContent, draft) {
    if (!$isAuthenticated) return;
    const keepAssistant = !draft.isError && (draft.content || draft.queries?.length);
    if (!keepAssistant && !messages.includes(draft)) return; // dropped empty turn
    try {
      if (!activeId) {
        const c = await llmCreateConversation(userContent);
        activeId = c.id;
      }
      await llmAppendMessage(activeId, { role: 'user', content: userContent });
      if (keepAssistant) {
        await llmAppendMessage(activeId, {
          role: 'assistant',
          content: draft.content || '',
          queries: draft.queries?.length ? draft.queries : null,
          model: draft.model || null,
          stopped: !!draft.stopped,
        });
      }
      loadConversations();
    } catch {
      // best-effort
    }
  }

  function startRename(c) {
    renamingId = c.id;
    renameText = c.title;
  }

  async function commitRename() {
    const id = renamingId;
    const title = renameText.trim();
    renamingId = null;
    if (!id || !title) return;
    const c = conversations.find((x) => x.id === id);
    if (!c || c.title === title) return;
    c.title = title;
    conversations = conversations;
    try { await llmRenameConversation(id, title); } catch { loadConversations(); }
  }

  async function confirmDelete() {
    const c = deleteTarget;
    deleteTarget = null;
    if (!c) return;
    conversations = conversations.filter((x) => x.id !== c.id);
    if (activeId === c.id) newChat();
    try { await llmDeleteConversation(c.id); } catch { loadConversations(); }
  }

  // ── Memory panel ────────────────────────────────────────────────────────────

  async function openMemory() {
    memoryOpen = true;
    memorySaved = false;
    try { memory = await llmMemory(); } catch { memory = { instructions: '', enabled: true }; }
  }

  async function saveMemory() {
    memorySaving = true;
    memorySaved = false;
    try {
      await llmSetMemory(memory.instructions, memory.enabled);
      memorySaved = true;
    } catch (e) {
      alert(e?.message || 'Could not save');
    } finally {
      memorySaving = false;
    }
  }

  // Models the gateway reports (OpenAI-style /v1/models payload), for About.
  $: gatewayModels = (llmStatus?.detail?.data || [])
    .map((m) => m?.id)
    .filter(Boolean)
    .slice(0, 6);

  function onScroll() {
    if (!scrollEl) return;
    autoScroll = scrollEl.scrollTop + scrollEl.clientHeight >= scrollEl.scrollHeight - 48;
  }

  async function scrollToBottom(force = false) {
    await tick();
    if (scrollEl && (autoScroll || force)) scrollEl.scrollTop = scrollEl.scrollHeight;
  }

  // Coalesce streamed tokens: the model can emit dozens of deltas per second,
  // and re-rendering markdown on every one wastes the main thread. Buffer and
  // flush on a short timer instead — still reads as live typing.
  let pendingText = '';
  let flushTimer = null;
  function queueDelta(draft, text) {
    pendingText += text;
    if (flushTimer) return;
    flushTimer = setTimeout(() => {
      flushTimer = null;
      if (!pendingText) return;
      draft.content += pendingText;
      pendingText = '';
      messages = messages;
      scrollToBottom();
    }, 50);
  }
  function clearDeltaQueue() {
    if (flushTimer) { clearTimeout(flushTimer); flushTimer = null; }
    pendingText = '';
  }

  async function send(text) {
    const content = (text ?? input).trim();
    if (!content || loading) return;
    input = '';
    loading = true;
    autoScroll = true;

    // The live assistant bubble this turn streams into.
    const draft = {
      role: 'assistant',
      content: '',
      streaming: true,
      queries: [],
      ranQuery: false,
      showQuery: false,
      reviewed: null,
      runs: [],
    };
    messages = [...messages, { role: 'user', content }, draft];
    await scrollToBottom(true);

    // Transport-error bubbles (isError) and empty drafts are UI-only — never
    // replay them as assistant turns in the model conversation.
    const wire = messages
      .filter((m) => !m.isError && m !== draft && m.content)
      .map((m) => ({ role: m.role, content: m.content }));

    abortCtl = new AbortController();
    let sawEvent = false;
    try {
      let resp;
      try {
        resp = await llmChatStream(wire, {
          signal: abortCtl.signal,
          onEvent: (ev) => { sawEvent = true; applyStreamEvent(draft, ev); },
        });
      } catch (e) {
        // An older server (404/405) or a buffering proxy: retry once, non-streaming.
        // Real rejections (guard 400, rate limit 429, auth) must NOT retry — the
        // buffered endpoint would just reject again and double-count the request.
        if (e?.name === 'AbortError' || sawEvent) throw e;
        if (e?.status && e.status !== 404 && e.status !== 405) throw e;
        resp = await llmChat(wire);
      }
      clearDeltaQueue();
      draft.content = resp.answer || $t('pages.llmChat.noAnswer');
      draft.queries = normalizeQueries(resp);
      draft.ranQuery = !!resp.ran_query;
      draft.model = resp.model || '';
      lastModel = draft.model || lastModel;
      draft.streaming = false;
    } catch (e) {
      clearDeltaQueue();
      draft.streaming = false;
      if (e?.name === 'AbortError') {
        draft.stopped = true;
        if (!draft.content && !draft.queries.length) {
          // Nothing useful arrived — drop the empty bubble.
          messages = messages.filter((m) => m !== draft);
        }
      } else if (draft.content) {
        draft.errorNote = e?.message || $t('pages.llmChat.unavailable');
      } else {
        draft.isError = true;
        draft.content = e?.message || $t('pages.llmChat.unavailable');
        draft.queries = [];
      }
    } finally {
      abortCtl = null;
      loading = false;
      messages = messages;
      persistTurn(content, draft);
      await scrollToBottom();
    }
  }

  // Fold one streamed event into the live draft bubble.
  function applyStreamEvent(draft, ev) {
    if (ev.type === 'delta') {
      queueDelta(draft, ev.text);
      return;
    }
    if (ev.type === 'round_reset') {
      clearDeltaQueue();
      draft.content = '';
    } else if (ev.type === 'query') {
      draft.queries = [...draft.queries, { sparql: ev.sparql, pending: true }];
    } else if (ev.type === 'query_result') {
      const q = draft.queries[draft.queries.length - 1];
      if (q) Object.assign(q, {
        pending: false,
        ok: ev.ok,
        rowCount: ev.rows,
        truncated: !!ev.truncated,
        error: ev.error,
      });
    }
    messages = messages;
    scrollToBottom();
  }

  function stop() {
    abortCtl?.abort();
  }

  // While streaming, an unfinished fenced block (``` not yet closed) would
  // render as a broken half-widget — cut the draft at the dangling fence and
  // let the closed part render; the rest arrives in a moment anyway.
  function streamRenderable(text) {
    const fences = [...text.matchAll(/^[ \t]*(`{3,}|~{3,})/gm)];
    if (fences.length % 2 === 0) return text;
    return text.slice(0, fences[fences.length - 1].index);
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
    newChat();
  }

  // Render the assistant's markdown — including fenced code blocks, which are
  // syntax-highlighted for SPARQL/Turtle/JSON/XML. renderMarkdown sanitizes the
  // output with DOMPurify, so model output still cannot inject markup.
  function renderRich(text) {
    return renderMarkdown(text || '', { breaks: true }).html;
  }
</script>

<div class="chat-layout" class:with-sidebar={$isAuthenticated}>
  {#if $isAuthenticated}
    <aside class="chat-sidebar">
      <button class="new-chat-btn" on:click={newChat} disabled={loading}>
        <Plus size={14} /> {$t('pages.llmChat.newChat')}
      </button>
      <div class="conv-list" class:dim={convLoading}>
        {#if conversations.length === 0}
          <p class="conv-empty">{$t('pages.llmChat.historyEmpty')}</p>
        {/if}
        {#each conversations as c (c.id)}
          <div class="conv-item" class:active={c.id === activeId}>
            {#if renamingId === c.id}
              <!-- svelte-ignore a11y-autofocus -->
              <input
                class="conv-rename"
                bind:value={renameText}
                autofocus
                on:keydown={(e) => { if (e.key === 'Enter') commitRename(); if (e.key === 'Escape') renamingId = null; }}
                on:blur={commitRename}
              />
            {:else}
              <button class="conv-open" on:click={() => openConversation(c.id)} title={c.title}>
                <MessageSquare size={13} />
                <span class="conv-title">{c.title || $t('pages.llmChat.untitled')}</span>
              </button>
              <span class="conv-actions">
                <button class="conv-act" on:click={() => startRename(c)} aria-label={$t('pages.llmChat.renameChat')} title={$t('pages.llmChat.renameChat')}><Pencil size={12} /></button>
                <button class="conv-act danger" on:click={() => { deleteTarget = c; }} aria-label={$t('pages.llmChat.deleteChat')} title={$t('pages.llmChat.deleteChat')}><Trash2 size={12} /></button>
              </span>
            {/if}
          </div>
        {/each}
      </div>
    </aside>
  {/if}

<div class="chat-page">
  <div class="chat-head-actions">
    {#if llmStatus}
      <span class="llm-badge" class:offline title={llmStatus.reachable ? $t('pages.llmChat.badgeOnlineTitle', { values: { gateway: llmStatus.gateway } }) : $t('pages.llmChat.badgeOfflineTitle')}>
        {llmStatus.reachable ? $t('pages.llmChat.badgeOnline') : $t('pages.llmChat.badgeOffline')}
      </span>
    {/if}
    <span class="head-spacer"></span>
    {#if $isAuthenticated}
      <button class="head-btn" on:click={openMemory} aria-label={$t('pages.llmChat.memoryTitle')} title={$t('pages.llmChat.memoryTitle')}>
        <NotebookPen size={14} />
      </button>
    {/if}
    <button class="head-btn" on:click={() => { aboutOpen = !aboutOpen; }} aria-label={$t('pages.llmChat.aboutSpark')} title={$t('pages.llmChat.aboutSpark')} aria-expanded={aboutOpen}>
      <Info size={14} />
    </button>
    {#if messages.length}
      <button class="btn-clear" on:click={clearChat}>{$t('pages.llmChat.clearChat')}</button>
    {/if}
    {#if aboutOpen}
      <div class="about-pop">
        <div class="about-head">
          <strong>{$t('pages.llmChat.aboutSpark')}</strong>
          <button class="head-btn" on:click={() => { aboutOpen = false; }} aria-label={$t('pages.llmChat.close')}><X size={13} /></button>
        </div>
        {#if lastModel || gatewayModels.length}
          <p class="about-row"><span>{$t('pages.llmChat.aboutModel')}</span> {lastModel || gatewayModels[0]}</p>
        {/if}
        {#if llmStatus?.gateway}
          <p class="about-row"><span>{$t('pages.llmChat.aboutGateway')}</span> {llmStatus.gateway}</p>
        {/if}
        <p class="about-title">{$t('pages.llmChat.aboutGroundingTitle')}</p>
        <p class="about-text">{$t('pages.llmChat.aboutGroundingText')}</p>
        <p class="about-title">{$t('pages.llmChat.aboutPrivacyTitle')}</p>
        <p class="about-text">{$t('pages.llmChat.aboutPrivacyText')}</p>
      </div>
    {/if}
  </div>

  {#if offline}
    <div class="offline-banner">
      <AlertTriangle size={15} />
      <span>{$t('pages.llmChat.offlineBanner')}</span>
    </div>
  {/if}

  <div class="messages" bind:this={scrollEl} on:scroll={onScroll}>
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
          {#if msg.role === 'assistant' && !msg.isError && msg.streaming}
            <!-- Live draft: tokens render as they stream (renderRich sanitizes
                 with DOMPurify); widgets materialize when the turn completes. -->
            {#if msg.content}
              <!-- eslint-disable-next-line svelte/no-at-html-tags -->
              <div class="bubble-text">{@html renderRich(streamRenderable(msg.content))}<span class="caret"></span></div>
            {:else}
              <p class="stream-status">
                <Loader2 size={13} class="spin" />
                {msg.queries.some((q) => q.pending) ? $t('pages.llmChat.statusQuerying') : $t('pages.llmChat.thinking')}
              </p>
            {/if}
            {#if msg.queries.length}
              <div class="live-trail">
                {#each msg.queries as q}
                  <span class="trail-chip" class:failed={q.pending === false && !q.ok}>
                    {#if q.pending}
                      <Loader2 size={11} class="spin" /> {$t('pages.llmChat.queryRunning')}
                    {:else if q.ok}
                      <Check size={11} /> {q.rowCount ?? 0}{q.truncated ? '+' : ''} {(q.rowCount ?? 0) === 1 ? $t('pages.llmChat.rowSingular') : $t('pages.llmChat.rowPlural')}
                    {:else}
                      <AlertTriangle size={11} /> {$t('pages.llmChat.queryFailedShort')}
                    {/if}
                  </span>
                {/each}
              </div>
            {/if}
          {:else if msg.role === 'assistant' && !msg.isError}
            <!-- Interactive answer: markdown plus runnable sparql/api blocks and
                 chart/map/card/csv widgets (sanitized in ChatRichMessage). -->
            <div class="bubble-text">
              <ChatRichMessage
                content={msg.content}
                on:runApi={(e) => attachRun(msg, e.detail)}
                on:openInSparql={(e) => openInSparql(e.detail)}
              />
            </div>
            {#if msg.stopped}
              <p class="stopped-note">{$t('pages.llmChat.stopped')}</p>
            {/if}
            {#if msg.errorNote}
              <p class="stopped-note">{msg.errorNote}</p>
            {/if}
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

          {#if !msg.streaming && msg.queries?.length}
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

          {#if msg.role === 'assistant' && !msg.isError && !msg.streaming}
            <div class="feedback">
              <span class="feedback-label">{$t('pages.llmChat.helpful')}</span>
              <button class="thumb" class:up={msg.reviewed === 'up'} on:click={() => review(msg, i, 'up')} aria-label={$t('pages.llmChat.helpfulYes')}><ThumbsUp size={13} /></button>
              <button class="thumb" class:down={msg.reviewed === 'down'} on:click={() => review(msg, i, 'down')} aria-label={$t('pages.llmChat.helpfulNo')}><ThumbsDown size={13} /></button>
            </div>
          {/if}
        </div>
      </div>
    {/each}

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
    {#if loading}
      <button class="stop-btn" type="button" on:click={stop} aria-label={$t('pages.llmChat.stop')} title={$t('pages.llmChat.stop')}>
        <Square size={13} />
      </button>
    {:else}
      <button class="send-btn" type="submit" disabled={offline || !input.trim()} aria-label={$t('pages.llmChat.send')}>
        <Send size={16} />
      </button>
    {/if}
  </form>
  <p class="disclaimer"><Database size={11} /> {$t('pages.llmChat.disclaimer')}</p>
</div>
</div>

{#if memoryOpen}
  <div
    class="memory-overlay"
    role="presentation"
    on:click={() => { memoryOpen = false; }}
    on:keydown={(e) => { if (e.key === 'Escape') memoryOpen = false; }}
  >
    <div
      class="memory-modal"
      role="dialog"
      aria-modal="true"
      aria-label={$t('pages.llmChat.memoryTitle')}
      tabindex="-1"
      on:click|stopPropagation
      on:keydown|stopPropagation={(e) => { if (e.key === 'Escape') memoryOpen = false; }}
    >
      <div class="memory-head">
        <strong><NotebookPen size={14} /> {$t('pages.llmChat.memoryTitle')}</strong>
        <button class="head-btn" on:click={() => { memoryOpen = false; }} aria-label={$t('pages.llmChat.close')}><X size={14} /></button>
      </div>
      <p class="memory-hint">{$t('pages.llmChat.memoryHint')}</p>
      <textarea
        class="memory-input"
        rows="6"
        maxlength="4000"
        bind:value={memory.instructions}
        placeholder={$t('pages.llmChat.memoryPlaceholder')}
      ></textarea>
      <label class="memory-toggle">
        <input type="checkbox" bind:checked={memory.enabled} />
        {$t('pages.llmChat.memoryEnabled')}
      </label>
      <div class="memory-actions">
        {#if memorySaved}<span class="memory-saved"><Check size={13} /> {$t('pages.llmChat.memorySaved')}</span>{/if}
        <button class="memory-save" on:click={saveMemory} disabled={memorySaving}>
          {#if memorySaving}<Loader2 size={13} class="spin" />{/if}
          {$t('pages.llmChat.memorySave')}
        </button>
      </div>
    </div>
  </div>
{/if}

{#if deleteTarget}
  <ConfirmModal
    title={$t('pages.llmChat.deleteChat')}
    message={`${$t('pages.llmChat.deleteChatConfirm')} — “${deleteTarget.title || $t('pages.llmChat.untitled')}”`}
    confirmLabel={$t('pages.llmChat.deleteChat')}
    on:confirm={confirmDelete}
    on:cancel={() => { deleteTarget = null; }}
  />
{/if}

<style>
  .chat-layout {
    max-width: 880px;
    margin: 0 auto;
    display: flex;
    align-items: stretch;
    gap: 1rem;
  }
  .chat-layout.with-sidebar { max-width: 1120px; }

  .chat-page {
    flex: 1;
    min-width: 0;
    padding: 0.5rem 0.25rem 1rem;
    display: flex;
    flex-direction: column;
  }

  /* ── Conversation sidebar ─────────────────────────────────────────────── */
  .chat-sidebar {
    width: 220px;
    flex-shrink: 0;
    padding: 0.5rem 0 1rem;
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
  }
  @media (max-width: 760px) { .chat-sidebar { display: none; } }
  .new-chat-btn {
    display: inline-flex; align-items: center; justify-content: center; gap: 0.4rem;
    padding: 0.5rem 0.75rem; border-radius: 10px; cursor: pointer;
    font-size: 0.82rem; font-weight: 600; color: #4338ca;
    background: #eef2ff; border: 1px solid #c7d2fe;
    transition: background 0.15s ease;
  }
  .new-chat-btn:hover:not(:disabled) { background: #e0e7ff; }
  .new-chat-btn:disabled { opacity: 0.5; cursor: not-allowed; }
  .conv-list {
    flex: 1; overflow-y: auto; max-height: 70vh;
    display: flex; flex-direction: column; gap: 2px;
  }
  .conv-list.dim { opacity: 0.6; pointer-events: none; }
  .conv-empty { font-size: 0.76rem; color: var(--ink-400); padding: 0.4rem 0.5rem; }
  .conv-item {
    display: flex; align-items: center; gap: 2px; border-radius: 8px;
    padding-right: 2px;
  }
  .conv-item:hover, .conv-item.active { background: var(--bg-strong); }
  .conv-item.active { border: 1px solid var(--line-soft); }
  .conv-open {
    flex: 1; min-width: 0; display: flex; align-items: center; gap: 0.45rem;
    background: none; border: none; cursor: pointer; text-align: left;
    padding: 0.45rem 0.5rem; color: var(--ink-700); font-size: 0.8rem;
  }
  .conv-open :global(svg) { flex-shrink: 0; opacity: 0.55; }
  .conv-title { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .conv-actions { display: none; align-items: center; }
  .conv-item:hover .conv-actions, .conv-item.active .conv-actions { display: inline-flex; }
  .conv-act {
    display: inline-flex; padding: 4px; border: none; background: none; cursor: pointer;
    color: var(--ink-400); border-radius: 6px;
  }
  .conv-act:hover { background: var(--line-soft); color: var(--ink-700); }
  .conv-act.danger:hover { color: #dc2626; }
  .conv-rename {
    flex: 1; min-width: 0; font: inherit; font-size: 0.8rem; padding: 0.35rem 0.45rem;
    border: 1px solid #a5b4fc; border-radius: 8px; background: var(--bg-strong); color: var(--ink-800);
    outline: none;
  }

  .chat-head-actions {
    position: relative;
    display: flex; align-items: center; justify-content: flex-end; gap: 0.5rem;
    margin-bottom: 0.5rem; min-height: 1.6rem;
  }
  .head-spacer { flex: 1; }
  .head-btn {
    display: inline-flex; align-items: center; justify-content: center;
    padding: 5px; border-radius: 8px; cursor: pointer; color: var(--ink-500);
    background: var(--bg-strong); border: 1px solid var(--line-soft);
  }
  .head-btn:hover { background: var(--bg-soft); color: var(--ink-700); }

  /* ── About popover ────────────────────────────────────────────────────── */
  .about-pop {
    position: absolute; top: 2rem; right: 0; z-index: 30; width: min(340px, 90vw);
    background: var(--bg-elevated); border: 1px solid var(--line-strong); border-radius: 12px;
    box-shadow: 0 10px 30px rgba(15,32,39,0.16); padding: 0.8rem 0.95rem; text-align: left;
  }
  .about-head { display: flex; align-items: center; justify-content: space-between; margin-bottom: 0.4rem; }
  .about-head strong { font-size: 0.88rem; color: var(--ink-800); }
  .about-row { margin: 0.15rem 0; font-size: 0.78rem; color: var(--ink-700); word-break: break-all; }
  .about-row span { display: inline-block; min-width: 64px; color: var(--ink-400); font-weight: 600; }
  .about-title { margin: 0.6rem 0 0.1rem; font-size: 0.74rem; font-weight: 700; color: var(--ink-500); text-transform: uppercase; letter-spacing: 0.03em; }
  .about-text { margin: 0; font-size: 0.78rem; color: var(--ink-600); line-height: 1.45; }

  /* ── Memory modal ─────────────────────────────────────────────────────── */
  .memory-overlay {
    position: fixed; inset: 0; z-index: 1200; display: grid; place-items: center;
    background: rgba(15, 23, 42, 0.45); padding: 1rem;
  }
  .memory-modal {
    width: min(480px, 94vw); background: var(--bg-elevated); border: 1px solid var(--line-strong);
    border-radius: 14px; box-shadow: 0 16px 48px rgba(15,32,39,0.25); padding: 1rem 1.1rem;
  }
  .memory-head { display: flex; align-items: center; justify-content: space-between; }
  .memory-head strong { display: inline-flex; align-items: center; gap: 0.4rem; font-size: 0.95rem; color: var(--ink-800); }
  .memory-hint { margin: 0.45rem 0 0.6rem; font-size: 0.78rem; color: var(--ink-500); line-height: 1.45; }
  .memory-input {
    width: 100%; box-sizing: border-box; resize: vertical; font: inherit; font-size: 0.85rem;
    min-height: 110px; padding: 0.55rem 0.65rem; border-radius: 10px;
    border: 1px solid var(--line-strong); background: var(--bg-strong); color: var(--ink-800);
  }
  .memory-input:focus { outline: none; border-color: #a5b4fc; box-shadow: 0 0 0 3px rgba(99,102,241,0.12); }
  .memory-toggle {
    display: flex; align-items: center; gap: 0.45rem; margin: 0.55rem 0 0;
    font-size: 0.8rem; color: var(--ink-700); cursor: pointer;
  }
  .memory-actions { display: flex; align-items: center; justify-content: flex-end; gap: 0.6rem; margin-top: 0.75rem; }
  .memory-saved { display: inline-flex; align-items: center; gap: 0.3rem; font-size: 0.76rem; color: #16a34a; }
  .memory-save {
    display: inline-flex; align-items: center; gap: 0.35rem; cursor: pointer;
    padding: 0.45rem 1rem; border-radius: 9px; border: none; color: #fff;
    font-size: 0.82rem; font-weight: 600;
    background: linear-gradient(135deg, #7c5cff, #4f46e5);
  }
  .memory-save:disabled { opacity: 0.6; cursor: not-allowed; }

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

  /* Live streaming turn: status line, blinking caret, and the retrieval trail
     chips that show each SPARQL round running and finishing in real time. */
  .stream-status {
    display: inline-flex; align-items: center; gap: 0.45rem; margin: 0;
    color: var(--ink-500); font-style: italic; font-size: 0.88rem;
  }
  .caret {
    display: inline-block; width: 7px; height: 1em; margin-left: 2px; border-radius: 2px;
    background: currentColor; opacity: 0.6; vertical-align: text-bottom;
    animation: caret-blink 1s steps(2, start) infinite;
  }
  @keyframes caret-blink { to { visibility: hidden; } }
  .live-trail { display: flex; flex-wrap: wrap; gap: 0.35rem; margin-top: 0.55rem; }
  .trail-chip {
    display: inline-flex; align-items: center; gap: 0.3rem;
    font-size: 0.72rem; font-weight: 600; color: #4338ca;
    background: #eef2ff; border: 1px solid #c7d2fe; border-radius: 999px; padding: 2px 9px;
  }
  .trail-chip.failed { color: #b91c1c; background: #fff8f8; border-color: #f3c9c9; }
  .stopped-note { margin: 0.45rem 0 0; font-size: 0.74rem; font-style: italic; color: var(--ink-400); }

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
  .stop-btn {
    flex-shrink: 0; width: 38px; height: 38px; border-radius: 11px; cursor: pointer;
    display: grid; place-items: center; color: #dc2626;
    background: #fff5f5; border: 1px solid #fca5a5;
    transition: background 0.15s ease, transform 0.12s ease;
  }
  .stop-btn:hover { background: #fee2e2; }
  .stop-btn:active { transform: scale(0.95); }

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
  :global(:is([data-theme="dark"], .dark)) .trail-chip { background: rgba(99,102,241,0.2); border-color: rgba(99,102,241,0.3); color: #a5b4fc; }
  :global(:is([data-theme="dark"], .dark)) .trail-chip.failed { background: rgba(220,38,38,0.12); border-color: rgba(220,38,38,0.35); color: #fca5a5; }
  :global(:is([data-theme="dark"], .dark)) .stop-btn { background: rgba(220,38,38,0.12); border-color: rgba(220,38,38,0.4); color: #fca5a5; }
  :global(:is([data-theme="dark"], .dark)) .stop-btn:hover { background: rgba(220,38,38,0.2); }
  :global(:is([data-theme="dark"], .dark)) .new-chat-btn { background: rgba(99,102,241,0.2); border-color: rgba(99,102,241,0.3); color: #a5b4fc; }
  :global(:is([data-theme="dark"], .dark)) .new-chat-btn:hover:not(:disabled) { background: rgba(99,102,241,0.28); }
  :global(:is([data-theme="dark"], .dark)) .memory-saved { color: #6ee7b7; }
</style>
