<script>
  // Admin inbox for the reports users send from the feedback dialog: filter by
  // status and kind, read a report with the context it was sent from, set its
  // status, reply to the reporter (they see it under "My reports") and keep an
  // internal note (admins only).
  import { t } from 'svelte-i18n';
  import { isAdmin, authInitialized } from '../lib/stores.js';
  import { navigate, Link } from '../lib/router/index.js';
  import { MessageSquareText, Bug, Lightbulb, HelpCircle, MessageSquare, Loader2, CircleAlert, Trash2, ChevronDown, Save, Inbox, RefreshCw } from 'lucide-svelte';
  import ConfirmModal from '../components/ConfirmModal.svelte';
  import { adminListFeedback, adminUpdateFeedback, adminDeleteFeedback } from '../lib/api.js';
  import { FEEDBACK_KINDS, FEEDBACK_STATUSES, isInAppPath } from '../lib/feedback.ts';
  import { toastSuccess } from '../lib/toast.ts';

  const ICONS = { bug: Bug, feature: Lightbulb, question: HelpCircle, other: MessageSquare };

  let reports = [];
  let loading = false;
  let loadError = '';
  let statusFilter = 'open';
  let kindFilter = '';
  let expanded = null;
  /** Unsaved triage per report id: { kind, status, admin_response, admin_note }. */
  let drafts = {};
  let saving = null;
  let removeTarget = null;
  let removing = false;

  // Counts come from one unfiltered load so the chips stay accurate while a
  // filter narrows the list.
  let all = [];
  $: counts = Object.fromEntries(FEEDBACK_STATUSES.map((s) => [s, all.filter((r) => r.status === s).length]));
  $: reports = all.filter((r) => (!statusFilter || r.status === statusFilter) && (!kindFilter || r.kind === kindFilter));

  async function load() {
    loading = true;
    loadError = '';
    try {
      all = await adminListFeedback();
    } catch (e) {
      loadError = e?.message || String(e);
    }
    loading = false;
  }

  function toggle(r) {
    if (expanded === r.id) {
      expanded = null;
      return;
    }
    expanded = r.id;
    if (!drafts[r.id]) {
      drafts[r.id] = {
        kind: r.kind,
        status: r.status,
        admin_response: r.admin_response || '',
        admin_note: r.admin_note || '',
      };
    }
  }

  function dirty(r, d) {
    return !!d && (d.kind !== r.kind
      || d.status !== r.status
      || d.admin_response !== (r.admin_response || '')
      || d.admin_note !== (r.admin_note || ''));
  }

  async function save(r) {
    const d = drafts[r.id];
    saving = r.id;
    loadError = '';
    try {
      const updated = await adminUpdateFeedback(r.id, d);
      all = all.map((x) => (x.id === r.id ? updated : x));
      drafts[r.id] = {
        kind: updated.kind,
        status: updated.status,
        admin_response: updated.admin_response || '',
        admin_note: updated.admin_note || '',
      };
      toastSuccess($t('pages.adminFeedback.saved'));
    } catch (e) {
      loadError = e?.message || String(e);
    }
    saving = null;
  }

  async function doRemove() {
    if (!removeTarget) return;
    removing = true;
    try {
      await adminDeleteFeedback(removeTarget.id);
      all = all.filter((x) => x.id !== removeTarget.id);
      if (expanded === removeTarget.id) expanded = null;
    } catch (e) {
      loadError = e?.message || String(e);
    }
    removeTarget = null;
    removing = false;
  }

  function formatDate(iso) {
    const d = new Date(iso);
    return Number.isNaN(d.getTime()) ? '' : d.toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' });
  }

  let _guardChecked = false;
  $: if ($authInitialized && !_guardChecked) {
    _guardChecked = true;
    if (!$isAdmin) navigate('/');
    else load();
  }
</script>

<div class="admin-feedback">
  <div class="header-row">
    <div class="header-text">
      <h2><MessageSquareText size={20} /> {$t('pages.adminFeedback.title')}</h2>
      <p class="subtitle">{$t('pages.adminFeedback.detail')}</p>
    </div>
    <button class="btn btn-sm btn-ghost" on:click={load} disabled={loading}>
      <RefreshCw size={14} class={loading ? 'animate-spin' : ''} /> {$t('pages.adminFeedback.refresh')}
    </button>
  </div>

  <div class="filters">
    <div class="chips" role="group" aria-label={$t('pages.adminFeedback.statusFilter')}>
      <button class="chip" class:active={statusFilter === ''} on:click={() => (statusFilter = '')}>
        {$t('pages.adminFeedback.all')} <span class="chip-n">{all.length}</span>
      </button>
      {#each FEEDBACK_STATUSES as s}
        <button class="chip" class:active={statusFilter === s} on:click={() => (statusFilter = s)}>
          {$t(`components.feedback.statuses.${s}`)} <span class="chip-n">{counts[s]}</span>
        </button>
      {/each}
    </div>
    <label class="kind-filter">
      <span>{$t('pages.adminFeedback.kindFilter')}</span>
      <select bind:value={kindFilter}>
        <option value="">{$t('pages.adminFeedback.allKinds')}</option>
        {#each FEEDBACK_KINDS as k}
          <option value={k}>{$t(`components.feedback.kinds.${k}`)}</option>
        {/each}
      </select>
    </label>
  </div>

  {#if loadError}
    <p class="card-error"><CircleAlert size={14} /> {loadError}</p>
  {/if}

  {#if loading && all.length === 0}
    <div class="loading"><Loader2 size={18} class="animate-spin" /> {$t('pages.adminFeedback.loading')}</div>
  {:else if reports.length === 0}
    <div class="empty-state">
      <Inbox size={32} />
      <p class="empty-title">{all.length === 0 ? $t('pages.adminFeedback.emptyTitle') : $t('pages.adminFeedback.noMatchTitle')}</p>
      <p class="empty-body">{all.length === 0 ? $t('pages.adminFeedback.emptyBody') : $t('pages.adminFeedback.noMatchBody')}</p>
    </div>
  {:else}
    <ul class="report-list">
      {#each reports as r (r.id)}
        {@const open = expanded === r.id}
        <li class="report" class:open>
          <button class="report-head" on:click={() => toggle(r)} aria-expanded={open}>
            <span class="kind-badge kind-{r.kind}">
              <svelte:component this={ICONS[r.kind] || MessageSquare} size={12} />
              {$t(`components.feedback.kinds.${r.kind}`)}
            </span>
            <span class="report-title">{r.title}</span>
            <span class="status status-{r.status}">{$t(`components.feedback.statuses.${r.status}`)}</span>
            <span class="report-meta">{r.username || $t('pages.adminFeedback.unknownUser')} · {formatDate(r.created_at)}</span>
            <ChevronDown size={16} class="chev" />
          </button>

          {#if open && drafts[r.id]}
            <div class="report-body">
              <p class="body-text">{r.body}</p>

              <dl class="context">
                <dt>{$t('pages.adminFeedback.from')}</dt>
                <dd>{r.email ? `${r.username || '—'} <${r.email}>` : (r.username || '—')}</dd>
                <dt>{$t('pages.adminFeedback.page')}</dt>
                <dd>
                  {#if isInAppPath(r.page)}
                    <Link to={r.page}><code>{r.page}</code></Link>
                  {:else if r.page}
                    <code>{r.page}</code>
                  {:else}—{/if}
                </dd>
                <dt>{$t('pages.adminFeedback.browser')}</dt>
                <dd>{r.user_agent || '—'}</dd>
                <dt>{$t('pages.adminFeedback.version')}</dt>
                <dd>{r.app_version || '—'}</dd>
                {#if r.updated_at !== r.created_at}
                  <dt>{$t('pages.adminFeedback.updated')}</dt>
                  <dd>{formatDate(r.updated_at)}</dd>
                {/if}
              </dl>

              <div class="triage">
                <label class="field">
                  <span>{$t('pages.adminFeedback.status')}</span>
                  <select bind:value={drafts[r.id].status}>
                    {#each FEEDBACK_STATUSES as s}
                      <option value={s}>{$t(`components.feedback.statuses.${s}`)}</option>
                    {/each}
                  </select>
                </label>
                <label class="field">
                  <span>{$t('pages.adminFeedback.kind')}</span>
                  <select bind:value={drafts[r.id].kind}>
                    {#each FEEDBACK_KINDS as k}
                      <option value={k}>{$t(`components.feedback.kinds.${k}`)}</option>
                    {/each}
                  </select>
                </label>
                <label class="field field-wide">
                  <span>{$t('pages.adminFeedback.response')}</span>
                  <textarea rows="3" maxlength="5000" bind:value={drafts[r.id].admin_response}
                    placeholder={$t('pages.adminFeedback.responsePlaceholder')}></textarea>
                  <small>{$t('pages.adminFeedback.responseHint')}</small>
                </label>
                <label class="field field-wide">
                  <span>{$t('pages.adminFeedback.note')}</span>
                  <textarea rows="2" maxlength="5000" bind:value={drafts[r.id].admin_note}
                    placeholder={$t('pages.adminFeedback.notePlaceholder')}></textarea>
                  <small>{$t('pages.adminFeedback.noteHint')}</small>
                </label>
              </div>

              <div class="row-actions">
                <button class="btn btn-sm btn-ghost btn-remove" on:click={() => (removeTarget = r)}>
                  <Trash2 size={14} /> {$t('pages.adminFeedback.delete')}
                </button>
                <button class="btn btn-sm" on:click={() => save(r)} disabled={saving === r.id || !dirty(r, drafts[r.id])}>
                  {#if saving === r.id}<Loader2 size={14} class="animate-spin" />{:else}<Save size={14} />{/if}
                  {$t('pages.adminFeedback.save')}
                </button>
              </div>
            </div>
          {/if}
        </li>
      {/each}
    </ul>
  {/if}
</div>

{#if removeTarget}
  <ConfirmModal
    title={$t('pages.adminFeedback.deleteTitle')}
    message={$t('pages.adminFeedback.deleteMessage', { values: { title: removeTarget.title } })}
    confirmLabel={$t('pages.adminFeedback.delete')}
    loading={removing}
    on:confirm={doRemove}
    on:cancel={() => (removeTarget = null)}
  />
{/if}

<style>
  .header-row {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 1rem;
    margin-bottom: 1rem;
  }
  .header-text { min-width: 0; }
  h2 { display: flex; align-items: center; gap: 0.5rem; margin: 0; }
  .subtitle { margin: 0.35rem 0 0; font-size: 0.86rem; color: var(--ink-500); line-height: 1.45; }

  .filters { display: flex; align-items: center; justify-content: space-between; flex-wrap: wrap; gap: 0.75rem; margin-bottom: 1rem; }
  .chips { display: flex; flex-wrap: wrap; gap: 0.35rem; }
  .chip {
    display: inline-flex; align-items: center; gap: 0.35rem;
    padding: 0.35rem 0.75rem; border-radius: 999px; cursor: pointer;
    border: 1px solid var(--line-soft); background: white; font-size: 0.82rem; color: var(--ink-700);
  }
  .chip.active { border-color: var(--brand-500); background: var(--bg-accent-soft, #eef9f9); color: var(--brand-700); font-weight: 600; }
  .chip-n { font-size: 0.74rem; color: var(--ink-500); }
  .kind-filter { display: flex; align-items: center; gap: 0.5rem; font-size: 0.84rem; }
  .kind-filter select { width: auto; min-width: 10rem; }

  .card-error {
    display: flex; align-items: center; gap: 0.4rem; margin: 0 0 0.75rem;
    padding: 0.5rem 0.7rem; border-radius: 8px; font-size: 0.84rem;
    background: #fee2e2; color: #991b1b; overflow-wrap: anywhere;
  }
  .loading { display: flex; align-items: center; justify-content: center; gap: 0.5rem; padding: 2rem; color: var(--ink-500); }
  .empty-state { display: flex; flex-direction: column; align-items: center; gap: 0.5rem; padding: 3rem 1rem; color: var(--ink-500); text-align: center; }
  .empty-title { margin: 0; font-weight: 600; }
  .empty-body { margin: 0; max-width: 34rem; font-size: 0.86rem; line-height: 1.45; }

  .report-list { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 0.5rem; }
  .report { background: white; border: 1px solid var(--line-soft); border-radius: 12px; overflow: hidden; }
  .report.open { border-color: var(--line-strong); }
  .report-head {
    width: 100%; display: flex; align-items: center; flex-wrap: wrap; gap: 0.5rem;
    padding: 0.75rem 0.9rem; border: none; background: none; cursor: pointer; text-align: left; color: inherit;
  }
  .report-head:hover { background: var(--bg-hover, #f8fafc); }
  .report-title { flex: 1 1 14rem; min-width: 0; font-weight: 600; font-size: 0.92rem; overflow-wrap: anywhere; }
  .report-meta { font-size: 0.78rem; color: var(--ink-500); }
  .report-head :global(.chev) { color: var(--ink-400); transition: transform 0.15s; }
  .report.open .report-head :global(.chev) { transform: rotate(180deg); }

  .report-body { padding: 0 0.9rem 0.9rem; border-top: 1px solid var(--line-soft); }
  .body-text { margin: 0.8rem 0; white-space: pre-wrap; overflow-wrap: anywhere; font-size: 0.9rem; line-height: 1.5; }
  .context {
    display: grid; grid-template-columns: max-content minmax(0, 1fr); gap: 0.25rem 0.9rem;
    margin: 0 0 0.9rem; padding: 0.6rem 0.75rem; border-radius: 10px;
    background: var(--bg-subtle, #f8fafc); font-size: 0.8rem;
  }
  .context dt { color: var(--ink-500); font-weight: 600; }
  .context dd { margin: 0; overflow-wrap: anywhere; color: var(--ink-700); }
  .context code { font-size: 0.78rem; }

  .triage { display: grid; grid-template-columns: repeat(2, minmax(0, 14rem)); gap: 0.75rem 1rem; }
  .field { display: flex; flex-direction: column; gap: 0.3rem; font-size: 0.85rem; }
  .field > span { font-weight: 600; }
  .field small { font-size: 0.76rem; color: var(--ink-500); }
  .field-wide { grid-column: 1 / -1; }
  .field textarea { resize: vertical; font-family: inherit; }
  .row-actions { display: flex; justify-content: space-between; gap: 0.5rem; margin-top: 0.9rem; }
  .btn-remove { color: #c62828; }
  .btn-remove:hover { background: #ffebee; }

  .kind-badge, .status {
    display: inline-flex; align-items: center; gap: 0.25rem;
    padding: 0.1rem 0.5rem; border-radius: 999px; font-size: 0.74rem; font-weight: 600; white-space: nowrap;
  }
  .kind-bug { background: #fee2e2; color: #991b1b; }
  .kind-feature { background: #ede9fe; color: #5b21b6; }
  .kind-question { background: #e0f2fe; color: #075985; }
  .kind-other { background: #f1f5f9; color: #334155; }
  .status-open { background: #e0f2fe; color: #075985; }
  .status-in_progress { background: #fef3c7; color: #92400e; }
  .status-resolved { background: #dcfce7; color: #166534; }
  .status-closed { background: #f1f5f9; color: #475569; }

  @media (max-width: 720px) {
    .header-row { flex-direction: column; align-items: stretch; }
    .triage { grid-template-columns: 1fr; }
    .row-actions .btn { width: auto; flex: 1 1 0; min-height: 40px; }
    .kind-filter { width: 100%; }
    .kind-filter select { flex: 1; }
  }

  :global(:is([data-theme="dark"], .dark)) .report,
  :global(:is([data-theme="dark"], .dark)) .chip { background: var(--bg-strong); }
  :global(:is([data-theme="dark"], .dark)) .chip.active { background: rgba(20, 184, 166, 0.12); color: var(--brand-300); }
  :global(:is([data-theme="dark"], .dark)) .context { background: rgba(255, 255, 255, 0.04); }
  :global(:is([data-theme="dark"], .dark)) .context dd { color: var(--ink-800); }
  :global(:is([data-theme="dark"], .dark)) .card-error { background: rgba(239, 68, 68, 0.18); color: #fca5a5; }
  :global(:is([data-theme="dark"], .dark)) .btn-remove { color: #fca5a5; }
  :global(:is([data-theme="dark"], .dark)) .btn-remove:hover { background: rgba(239, 68, 68, 0.15); }
  :global(:is([data-theme="dark"], .dark)) .kind-bug { background: rgba(239, 68, 68, 0.2); color: #fca5a5; }
  :global(:is([data-theme="dark"], .dark)) .kind-feature { background: rgba(139, 92, 246, 0.2); color: #c4b5fd; }
  :global(:is([data-theme="dark"], .dark)) .kind-question,
  :global(:is([data-theme="dark"], .dark)) .status-open { background: rgba(14, 165, 233, 0.2); color: #7dd3fc; }
  :global(:is([data-theme="dark"], .dark)) .kind-other,
  :global(:is([data-theme="dark"], .dark)) .status-closed { background: rgba(148, 163, 184, 0.18); color: #cbd5e1; }
  :global(:is([data-theme="dark"], .dark)) .status-in_progress { background: rgba(245, 158, 11, 0.18); color: #fcd34d; }
  :global(:is([data-theme="dark"], .dark)) .status-resolved { background: rgba(34, 197, 94, 0.18); color: #86efac; }
</style>
