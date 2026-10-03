<script>
  // "Send feedback" dialog: bug reports, feature requests and questions for
  // this instance's admins, plus a "My reports" tab where the reporter follows
  // status and replies. Mounted once in App.svelte and opened through
  // lib/feedback.ts. The draft survives closing the dialog; it is cleared only
  // once a report is sent.
  import { tick, onDestroy } from 'svelte';
  import { t } from 'svelte-i18n';
  import { Bug, Lightbulb, HelpCircle, MessageSquare, X, Loader2, CheckCircle2, ShieldAlert, LogIn, CircleAlert, Inbox } from 'lucide-svelte';
  import { submitFeedback, listMyFeedback } from '../lib/api.js';
  import { isAuthenticated } from '../lib/stores.js';
  import { Link } from '../lib/router/index.js';
  import {
    feedbackDialog, closeFeedback, currentPage,
    FEEDBACK_KINDS, FEEDBACK_TITLE_MAX, FEEDBACK_BODY_MAX,
  } from '../lib/feedback.ts';

  const ICONS = { bug: Bug, feature: Lightbulb, question: HelpCircle, other: MessageSquare };

  let tab = 'new';
  let kind = 'bug';
  let title = '';
  let body = '';
  let includePage = true;
  let includeBrowser = true;
  let page = '';

  let sending = false;
  let sendError = '';
  let sent = null;

  let mine = [];
  let mineLoading = false;
  let mineError = '';

  let dialogEl;
  let open = false;

  // Run onOpen on the closed → open edge only. A subscription rather than
  // reactive statements: Svelte would order `wasOpen = open` before the check.
  const unsubscribe = feedbackDialog.subscribe((s) => {
    const opening = s.open && !open;
    open = s.open;
    if (opening) onOpen(s.kind);
  });
  onDestroy(unsubscribe);

  $: titleLen = title.trim().length;
  $: bodyLen = body.trim().length;
  $: valid = titleLen > 0 && titleLen <= FEEDBACK_TITLE_MAX && bodyLen > 0 && bodyLen <= FEEDBACK_BODY_MAX;

  async function onOpen(preset) {
    if (preset) kind = preset;
    page = currentPage();
    tab = 'new';
    sendError = '';
    await tick();
    dialogEl?.querySelector('input[name="feedback-kind"]:checked')?.focus();
  }

  function close() {
    if (sending) return;
    sent = null;
    closeFeedback();
  }

  function onKeydown(e) {
    if (open && e.key === 'Escape') close();
  }

  async function send() {
    if (!valid || sending) return;
    sending = true;
    sendError = '';
    try {
      sent = await submitFeedback({
        kind,
        title: title.trim(),
        body: body.trim(),
        page: includePage ? page : null,
        include_browser: includeBrowser,
      });
      title = '';
      body = '';
    } catch (e) {
      sendError = e?.status === 429
        ? (e.message || $t('components.feedback.rateLimited'))
        : (e?.message || String(e));
    } finally {
      sending = false;
    }
  }

  function another() {
    sent = null;
    tick().then(() => dialogEl?.querySelector('#feedback-title')?.focus());
  }

  async function showMine() {
    tab = 'mine';
    sent = null;
    mineLoading = true;
    mineError = '';
    try {
      mine = await listMyFeedback();
    } catch (e) {
      mineError = e?.message || String(e);
    } finally {
      mineLoading = false;
    }
  }

  function formatDate(iso) {
    const d = new Date(iso);
    return Number.isNaN(d.getTime()) ? '' : d.toLocaleDateString(undefined, { dateStyle: 'medium' });
  }
</script>

<svelte:window on:keydown={onKeydown} />

{#if open}
  <!-- svelte-ignore a11y-click-events-have-key-events -->
  <div class="fb-backdrop" on:click={close} role="presentation">
    <div
      class="fb-box"
      on:click|stopPropagation
      role="dialog"
      aria-modal="true"
      aria-labelledby="fb-title"
      tabindex="-1"
      bind:this={dialogEl}
    >
      <header class="fb-head">
        <div class="fb-head-text">
          <h2 id="fb-title">{$t('components.feedback.title')}</h2>
          <p>{$t('components.feedback.subtitle')}</p>
        </div>
        <button class="fb-close" on:click={close} aria-label={$t('components.feedback.close')}>
          <X size={18} />
        </button>
      </header>

      {#if !$isAuthenticated}
        <div class="fb-signin">
          <LogIn size={22} />
          <p class="fb-signin-t">{$t('components.feedback.signInTitle')}</p>
          <p class="fb-signin-b">{$t('components.feedback.signInBody')}</p>
          <div class="fb-signin-actions">
            <Link class="btn btn-sm" to="/login" on:click={close}>{$t('nav.signIn')}</Link>
            <Link class="btn btn-sm btn-ghost" to="/docs/faq" on:click={close}>{$t('components.feedback.readFaq')}</Link>
          </div>
        </div>
      {:else}
        <div class="fb-tabs" role="tablist">
          <button role="tab" aria-selected={tab === 'new'} class:active={tab === 'new'} on:click={() => (tab = 'new')}>
            {$t('components.feedback.tabNew')}
          </button>
          <button role="tab" aria-selected={tab === 'mine'} class:active={tab === 'mine'} on:click={showMine}>
            {$t('components.feedback.tabMine')}
          </button>
        </div>

        {#if tab === 'new' && sent}
          <div class="fb-sent" role="status">
            <CheckCircle2 size={34} />
            <p class="fb-sent-t">{$t('components.feedback.sentTitle')}</p>
            <p class="fb-sent-b">{$t('components.feedback.sentBody')}</p>
            <div class="fb-actions">
              <button class="btn btn-sm btn-ghost" on:click={showMine}>{$t('components.feedback.viewMine')}</button>
              <button class="btn btn-sm btn-ghost" on:click={another}>{$t('components.feedback.sendAnother')}</button>
              <button class="btn btn-sm" on:click={close}>{$t('components.feedback.done')}</button>
            </div>
          </div>
        {:else if tab === 'new'}
          <form class="fb-form" on:submit|preventDefault={send}>
            <fieldset class="fb-kinds">
              <legend>{$t('components.feedback.kindLegend')}</legend>
              {#each FEEDBACK_KINDS as k}
                <label class="fb-kind" class:selected={kind === k}>
                  <input type="radio" name="feedback-kind" value={k} bind:group={kind} />
                  <svelte:component this={ICONS[k]} size={18} />
                  <span class="fb-kind-t">{$t(`components.feedback.kinds.${k}`)}</span>
                  <span class="fb-kind-d">{$t(`components.feedback.kindHints.${k}`)}</span>
                </label>
              {/each}
            </fieldset>

            <div class="fb-field">
              <label for="feedback-title">{$t('components.feedback.titleLabel')}</label>
              <input
                id="feedback-title"
                type="text"
                bind:value={title}
                maxlength={FEEDBACK_TITLE_MAX}
                placeholder={$t(`components.feedback.titlePlaceholders.${kind}`)}
                autocomplete="off"
              />
            </div>

            <div class="fb-field">
              <label for="feedback-body">{$t('components.feedback.bodyLabel')}</label>
              <textarea
                id="feedback-body"
                rows="7"
                bind:value={body}
                maxlength={FEEDBACK_BODY_MAX}
                placeholder={$t(`components.feedback.bodyPlaceholders.${kind}`)}
              ></textarea>
              <span class="fb-count" class:over={bodyLen > FEEDBACK_BODY_MAX}>{bodyLen.toLocaleString()} / {FEEDBACK_BODY_MAX.toLocaleString()}</span>
            </div>

            <div class="fb-context">
              <p class="fb-context-h">{$t('components.feedback.contextHeading')}</p>
              <label class="fb-check">
                <input type="checkbox" bind:checked={includePage} />
                <span>{$t('components.feedback.includePage')} <code>{page}</code></span>
              </label>
              <label class="fb-check">
                <input type="checkbox" bind:checked={includeBrowser} />
                <span>{$t('components.feedback.includeBrowser')}</span>
              </label>
            </div>

            <p class="fb-security">
              <ShieldAlert size={15} />
              <span>{$t('components.feedback.securityNote')}</span>
            </p>

            {#if sendError}
              <p class="fb-error" role="alert"><CircleAlert size={14} /> {sendError}</p>
            {/if}

            <div class="fb-actions">
              <button type="button" class="btn btn-sm btn-ghost" on:click={close} disabled={sending}>
                {$t('components.feedback.cancel')}
              </button>
              <button type="submit" class="btn btn-sm" disabled={!valid || sending}>
                {#if sending}<Loader2 size={14} class="animate-spin" />{/if}
                {$t('components.feedback.send')}
              </button>
            </div>
          </form>
        {:else}
          <div class="fb-mine">
            {#if mineLoading}
              <p class="fb-muted"><Loader2 size={14} class="animate-spin" /> {$t('components.feedback.loading')}</p>
            {:else if mineError}
              <p class="fb-error"><CircleAlert size={14} /> {mineError}</p>
            {:else if mine.length === 0}
              <div class="fb-empty">
                <Inbox size={26} />
                <p>{$t('components.feedback.mineEmpty')}</p>
              </div>
            {:else}
              <ul class="fb-list">
                {#each mine as r (r.id)}
                  <li class="fb-item">
                    <div class="fb-item-top">
                      <span class="fb-kind-badge kind-{r.kind}">
                        <svelte:component this={ICONS[r.kind] || MessageSquare} size={12} />
                        {$t(`components.feedback.kinds.${r.kind}`)}
                      </span>
                      <span class="fb-status status-{r.status}">{$t(`components.feedback.statuses.${r.status}`)}</span>
                      <span class="fb-date">{formatDate(r.created_at)}</span>
                    </div>
                    <p class="fb-item-title">{r.title}</p>
                    {#if r.admin_response}
                      <div class="fb-reply">
                        <p class="fb-reply-h">{$t('components.feedback.replyHeading')}</p>
                        <p class="fb-reply-b">{r.admin_response}</p>
                      </div>
                    {/if}
                  </li>
                {/each}
              </ul>
            {/if}
          </div>
        {/if}
      {/if}
    </div>
  </div>
{/if}

<style>
  .fb-backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.35);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 120;
    padding: 1rem;
  }
  .fb-box {
    background: white;
    border-radius: 1rem;
    width: min(600px, 100%);
    max-height: calc(100vh - 2rem);
    max-height: calc(100dvh - 2rem);
    overflow-y: auto;
    box-shadow: 0 20px 60px rgba(0, 0, 0, 0.18);
    padding: 1.25rem 1.35rem 1.35rem;
    outline: none;
  }
  .fb-head { display: flex; align-items: flex-start; gap: 0.75rem; }
  .fb-head-text { flex: 1; min-width: 0; }
  .fb-head h2 { margin: 0; font-size: 1.15rem; }
  .fb-head p { margin: 0.25rem 0 0; font-size: 0.85rem; color: var(--ink-500); line-height: 1.45; }
  .fb-close {
    border: none; background: none; cursor: pointer; color: var(--ink-500);
    padding: 0.3rem; border-radius: 8px; display: inline-flex;
  }
  .fb-close:hover { background: var(--bg-hover, #f3f4f6); color: var(--ink-900); }

  .fb-tabs { display: flex; gap: 0.25rem; margin: 1rem 0 0.9rem; border-bottom: 1px solid var(--line-soft); }
  .fb-tabs button {
    border: none; background: none; cursor: pointer; padding: 0.5rem 0.75rem;
    font-size: 0.86rem; font-weight: 600; color: var(--ink-500);
    border-bottom: 2px solid transparent; margin-bottom: -1px;
  }
  .fb-tabs button.active { color: var(--brand-600); border-bottom-color: var(--brand-500); }

  .fb-form { display: flex; flex-direction: column; gap: 0.9rem; }
  .fb-kinds { border: none; margin: 0; padding: 0; display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 0.5rem; }
  .fb-kinds legend { font-size: 0.85rem; font-weight: 600; margin-bottom: 0.4rem; padding: 0; }
  .fb-kind {
    position: relative;
    display: grid;
    grid-template-columns: auto 1fr;
    column-gap: 0.55rem;
    align-items: center;
    padding: 0.6rem 0.7rem;
    border: 1px solid var(--line-soft);
    border-radius: 10px;
    cursor: pointer;
    color: var(--ink-600);
  }
  .fb-kind:hover { border-color: var(--line-strong); }
  .fb-kind.selected { border-color: var(--brand-500); background: var(--bg-accent-soft, #eef9f9); color: var(--brand-700); }
  .fb-kind:focus-within { outline: 2px solid var(--brand-400); outline-offset: 1px; }
  .fb-kind input { position: absolute; opacity: 0; pointer-events: none; }
  .fb-kind-t { font-size: 0.88rem; font-weight: 600; color: var(--ink-900); }
  .fb-kind-d { grid-column: 2; font-size: 0.76rem; color: var(--ink-500); line-height: 1.35; }

  .fb-field { display: flex; flex-direction: column; gap: 0.3rem; }
  .fb-field label { font-size: 0.85rem; font-weight: 600; }
  .fb-field textarea { resize: vertical; min-height: 8rem; font-family: inherit; line-height: 1.45; }
  .fb-count { align-self: flex-end; font-size: 0.74rem; color: var(--ink-400); }
  .fb-count.over { color: #c62828; }

  .fb-context { display: flex; flex-direction: column; gap: 0.35rem; }
  .fb-context-h { margin: 0; font-size: 0.85rem; font-weight: 600; }
  .fb-check { display: flex; align-items: flex-start; gap: 0.5rem; font-size: 0.84rem; color: var(--ink-700); cursor: pointer; }
  .fb-check input { margin-top: 0.2rem; }
  .fb-check code { font-size: 0.78rem; overflow-wrap: anywhere; }

  .fb-security {
    display: flex; gap: 0.45rem; align-items: flex-start; margin: 0;
    padding: 0.55rem 0.7rem; border-radius: 8px;
    background: #fef3c7; color: #92400e; font-size: 0.8rem; line-height: 1.4;
  }
  .fb-security :global(svg) { flex-shrink: 0; margin-top: 0.1rem; }

  .fb-error {
    display: flex; align-items: center; gap: 0.4rem; margin: 0;
    padding: 0.5rem 0.7rem; border-radius: 8px; font-size: 0.84rem;
    background: #fee2e2; color: #991b1b; overflow-wrap: anywhere;
  }
  .fb-actions { display: flex; justify-content: flex-end; flex-wrap: wrap; gap: 0.5rem; }

  .fb-sent, .fb-signin, .fb-empty {
    display: flex; flex-direction: column; align-items: center; text-align: center;
    gap: 0.4rem; padding: 1.5rem 0.5rem 0.5rem;
  }
  .fb-sent :global(svg) { color: var(--success-500, #16a34a); }
  .fb-signin :global(svg), .fb-empty :global(svg) { color: var(--ink-400); }
  .fb-sent-t, .fb-signin-t { margin: 0; font-weight: 700; }
  .fb-sent-b, .fb-signin-b, .fb-empty p { margin: 0 0 0.6rem; font-size: 0.86rem; color: var(--ink-500); max-width: 26rem; line-height: 1.45; }
  .fb-sent .fb-actions, .fb-signin-actions { justify-content: center; display: flex; gap: 0.5rem; flex-wrap: wrap; }

  .fb-muted { display: flex; align-items: center; gap: 0.4rem; color: var(--ink-500); font-size: 0.86rem; }
  .fb-list { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 0.5rem; }
  .fb-item { border: 1px solid var(--line-soft); border-radius: 10px; padding: 0.65rem 0.8rem; }
  .fb-item-top { display: flex; align-items: center; flex-wrap: wrap; gap: 0.4rem; }
  .fb-item-title { margin: 0.35rem 0 0; font-size: 0.9rem; font-weight: 600; overflow-wrap: anywhere; }
  .fb-date { margin-left: auto; font-size: 0.76rem; color: var(--ink-500); }
  .fb-reply { margin-top: 0.5rem; padding: 0.5rem 0.65rem; border-left: 3px solid var(--brand-400); background: var(--bg-subtle, #f8fafc); border-radius: 0 8px 8px 0; }
  .fb-reply-h { margin: 0; font-size: 0.74rem; font-weight: 700; text-transform: uppercase; letter-spacing: 0.04em; color: var(--ink-500); }
  .fb-reply-b { margin: 0.2rem 0 0; font-size: 0.85rem; white-space: pre-wrap; overflow-wrap: anywhere; }

  .fb-kind-badge, .fb-status {
    display: inline-flex; align-items: center; gap: 0.25rem;
    padding: 0.1rem 0.5rem; border-radius: 999px; font-size: 0.74rem; font-weight: 600;
  }
  .kind-bug { background: #fee2e2; color: #991b1b; }
  .kind-feature { background: #ede9fe; color: #5b21b6; }
  .kind-question { background: #e0f2fe; color: #075985; }
  .kind-other { background: #f1f5f9; color: #334155; }
  .status-open { background: #e0f2fe; color: #075985; }
  .status-in_progress { background: #fef3c7; color: #92400e; }
  .status-resolved { background: #dcfce7; color: #166534; }
  .status-closed { background: #f1f5f9; color: #475569; }

  @media (max-width: 520px) {
    .fb-kinds { grid-template-columns: 1fr; }
    .fb-box { padding: 1rem; }
  }

  :global(:is([data-theme="dark"], .dark)) .fb-backdrop { background: rgba(0, 0, 0, 0.62); }
  :global(:is([data-theme="dark"], .dark)) .fb-box {
    background: var(--bg-strong);
    color: var(--ink-900);
    border: 1px solid var(--line-strong);
    box-shadow: 0 24px 70px rgba(0, 0, 0, 0.55);
  }
  :global(:is([data-theme="dark"], .dark)) .fb-kind.selected { background: rgba(20, 184, 166, 0.12); color: var(--brand-300); }
  :global(:is([data-theme="dark"], .dark)) .fb-security { background: rgba(245, 158, 11, 0.16); color: #fcd34d; }
  :global(:is([data-theme="dark"], .dark)) .fb-error { background: rgba(239, 68, 68, 0.18); color: #fca5a5; }
  :global(:is([data-theme="dark"], .dark)) .fb-reply { background: rgba(255, 255, 255, 0.04); }
  :global(:is([data-theme="dark"], .dark)) .kind-bug { background: rgba(239, 68, 68, 0.2); color: #fca5a5; }
  :global(:is([data-theme="dark"], .dark)) .kind-feature { background: rgba(139, 92, 246, 0.2); color: #c4b5fd; }
  :global(:is([data-theme="dark"], .dark)) .kind-question,
  :global(:is([data-theme="dark"], .dark)) .status-open { background: rgba(14, 165, 233, 0.2); color: #7dd3fc; }
  :global(:is([data-theme="dark"], .dark)) .kind-other,
  :global(:is([data-theme="dark"], .dark)) .status-closed { background: rgba(148, 163, 184, 0.18); color: #cbd5e1; }
  :global(:is([data-theme="dark"], .dark)) .status-in_progress { background: rgba(245, 158, 11, 0.18); color: #fcd34d; }
  :global(:is([data-theme="dark"], .dark)) .status-resolved { background: rgba(34, 197, 94, 0.18); color: #86efac; }
</style>
