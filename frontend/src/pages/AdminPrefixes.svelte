<script>
  import { onDestroy } from 'svelte';
  import { t } from 'svelte-i18n';
  import { isAdmin, authInitialized } from '../lib/stores.js';
  import { navigate } from '../lib/router/index.js';
  import { Tags, Plus, Trash2, Loader2, Replace, CircleAlert, Check } from 'lucide-svelte';
  import ConfirmModal from '../components/ConfirmModal.svelte';
  import {
    adminListPrefixOverrides, adminCreatePrefixOverride,
    adminPutPrefixOverride, adminDeletePrefixOverride, lookupPrefixLabel,
  } from '../lib/api.js';
  import { validatePrefixLabel, validatePrefixNamespace } from '../lib/validate.ts';

  let overrides = [];
  let loading = false;
  let loadError = '';

  async function load() {
    loading = true;
    loadError = '';
    try {
      overrides = await adminListPrefixOverrides();
    } catch (e) {
      loadError = e.message;
    }
    loading = false;
  }

  // ── Add an override ─────────────────────────────────────────────────────────

  let showForm = false;
  let form = { label: '', namespace: '' };
  let touched = { label: false, namespace: false };
  let saving = false;
  let formError = '';
  /**
   * The 409 body: the label is taken, and `namespace` is what it resolves to
   * here today. Kept as state rather than flattened into `formError` because it
   * is not only a sentence — the user decides from it whether to repoint.
   */
  let conflict = null;

  // The server is the gate; these only save a round trip on input it would
  // refuse anyway, so the messages appear as the field is typed.
  $: labelError = validatePrefixLabel(form.label.trim());
  $: namespaceError = validatePrefixNamespace(form.namespace.trim());
  $: formValid = !labelError && !namespaceError;

  function labelMessage(code) {
    if (code === 'required') return $t('pages.adminPrefixes.labelRequired');
    if (code === 'tooLong') return $t('pages.adminPrefixes.labelTooLong');
    if (code === 'start') return $t('pages.adminPrefixes.labelStart');
    return $t('pages.adminPrefixes.labelCharset');
  }

  function namespaceMessage(code) {
    if (code === 'required') return $t('pages.adminPrefixes.namespaceRequired');
    if (code === 'scheme') return $t('pages.adminPrefixes.namespaceScheme');
    return $t('pages.adminPrefixes.namespaceFormat');
  }

  function openForm() {
    form = { label: '', namespace: '' };
    touched = { label: false, namespace: false };
    formError = '';
    conflict = null;
    clearTimeout(resolveTimer);
    resolving = false;
    resolved = null;
    resolvedFor = '';
    showForm = true;
  }

  function closeForm() {
    showForm = false;
    clearTimeout(resolveTimer);
    resolving = false;
  }

  async function submitCreate() {
    touched = { label: true, namespace: true };
    if (!formValid || saving) return;
    saving = true;
    formError = '';
    conflict = null;
    try {
      await adminCreatePrefixOverride(form.label.trim(), form.namespace.trim());
      closeForm();
      await load();
    } catch (e) {
      const body = e?.body;
      if (e?.status === 409 && body && typeof body.namespace === 'string') {
        conflict = { label: body.label || form.label.trim(), namespace: body.namespace };
      } else {
        formError = e.message;
      }
    }
    saving = false;
  }

  /** Accept the conflict: PUT repoints the label the POST refused to claim. */
  async function repointFromConflict() {
    if (!conflict || saving) return;
    saving = true;
    formError = '';
    try {
      await adminPutPrefixOverride(conflict.label, form.namespace.trim());
      conflict = null;
      closeForm();
      await load();
    } catch (e) {
      formError = e.message;
    }
    saving = false;
  }

  // ── What the label means here today ─────────────────────────────────────────
  //
  // Repointing a shorthand changes what every stored CURIE using it expands to,
  // so the tier it currently resolves from is shown before the override is
  // created rather than after. Debounced (this runs while the field is typed)
  // and sequenced (answers can arrive out of order).

  let resolved = null;      // { namespace, source } once answered
  let resolvedFor = '';     // the label `resolved` describes
  let resolving = false;
  let resolveTimer = null;
  let resolveSeq = 0;

  function scheduleResolve() {
    clearTimeout(resolveTimer);
    const label = form.label.trim();
    if (validatePrefixLabel(label)) {
      resolving = false;
      resolved = null;
      resolvedFor = '';
      return;
    }
    // Back to a label already answered — including by way of a keystroke that
    // left and returned, whose pending lookup was just cancelled above.
    if (label === resolvedFor) {
      resolving = false;
      return;
    }
    resolving = true;
    resolveTimer = setTimeout(() => void resolveLabel(label), 300);
  }

  async function resolveLabel(label) {
    const seq = ++resolveSeq;
    try {
      const hit = await lookupPrefixLabel(label);
      if (seq !== resolveSeq) return;
      resolved = { namespace: hit.namespace, source: hit.source };
    } catch {
      if (seq !== resolveSeq) return;
      // 404 is the useful answer here — nothing defines the label yet — and any
      // other failure means the same thing to the user: nothing to show.
      resolved = null;
    }
    resolvedFor = label;
    resolving = false;
  }

  // Reactive so the tier names re-translate when the language changes.
  $: SOURCE_LABEL = {
    admin: $t('pages.adminPrefixes.sourceAdmin'),
    platform: $t('pages.adminPrefixes.sourcePlatform'),
    seeded: $t('pages.adminPrefixes.sourceSeeded'),
    'prefix.cc': $t('pages.adminPrefixes.sourcePrefixCc'),
    lov: $t('pages.adminPrefixes.sourceLov'),
    cache: $t('pages.adminPrefixes.sourceCache'),
  };

  // ── Repoint an existing override ────────────────────────────────────────────

  let editingLabel = null;
  let editNamespace = '';
  let editError = '';
  let editSaving = false;

  $: editNamespaceError = editingLabel === null ? null : validatePrefixNamespace(editNamespace.trim());

  function openEdit(o) {
    editingLabel = o.label;
    editNamespace = o.namespace;
    editError = '';
  }

  function cancelEdit() {
    editingLabel = null;
    editNamespace = '';
    editError = '';
  }

  async function submitEdit() {
    if (editNamespaceError || editSaving) return;
    editSaving = true;
    editError = '';
    try {
      await adminPutPrefixOverride(editingLabel, editNamespace.trim());
      cancelEdit();
      await load();
    } catch (e) {
      editError = e.message;
    }
    editSaving = false;
  }

  // ── Remove an override ──────────────────────────────────────────────────────

  let removeTarget = null;
  let removing = false;

  async function doRemove() {
    if (!removeTarget) return;
    removing = true;
    try {
      await adminDeletePrefixOverride(removeTarget.label);
      removeTarget = null;
      await load();
    } catch (e) {
      loadError = e.message;
      removeTarget = null;
    }
    removing = false;
  }

  function formatDate(iso) {
    const d = new Date(iso);
    return Number.isNaN(d.getTime()) ? '' : d.toLocaleDateString();
  }

  let _guardChecked = false;
  $: if ($authInitialized && !_guardChecked) {
    _guardChecked = true;
    if (!$isAdmin) navigate('/');
    else load();
  }

  onDestroy(() => clearTimeout(resolveTimer));
</script>

<div class="admin-prefixes">
  <div class="header-row">
    <div class="header-text">
      <h2><Tags size={20} /> {$t('pages.adminPrefixes.title')}</h2>
      <p class="subtitle">{$t('pages.adminPrefixes.detail')}</p>
    </div>
    <button class="btn btn-sm" on:click={openForm} disabled={showForm}>
      <Plus size={14} /> {$t('pages.adminPrefixes.addOverride')}
    </button>
  </div>

  {#if loadError}
    <p class="card-error"><CircleAlert size={14} /> {loadError}</p>
  {/if}

  {#if showForm}
    <section class="card">
      <h3>{$t('pages.adminPrefixes.newOverride')}</h3>
      <p class="explain">{$t('pages.adminPrefixes.newOverrideHint')}</p>

      {#if formError}
        <p class="card-error"><CircleAlert size={14} /> {formError}</p>
      {/if}

      <div class="form-grid">
        <div class="form-group">
          <label for="prefix-label">{$t('pages.adminPrefixes.labelField')}</label>
          <input
            id="prefix-label"
            bind:value={form.label}
            on:input={() => { touched.label = true; conflict = null; scheduleResolve(); }}
            placeholder="geo"
            maxlength="64"
            autocomplete="off"
            spellcheck="false"
            aria-invalid={touched.label && !!labelError}
          />
          {#if touched.label && labelError}
            <p class="field-error">{labelMessage(labelError)}</p>
          {:else}
            <p class="field-hint">{$t('pages.adminPrefixes.labelHint')}</p>
          {/if}
        </div>
        <div class="form-group">
          <label for="prefix-namespace">{$t('pages.adminPrefixes.namespaceField')}</label>
          <input
            id="prefix-namespace"
            bind:value={form.namespace}
            on:input={() => touched.namespace = true}
            placeholder="https://example.org/ns#"
            autocomplete="off"
            spellcheck="false"
            aria-invalid={touched.namespace && !!namespaceError}
          />
          {#if touched.namespace && namespaceError}
            <p class="field-error">{namespaceMessage(namespaceError)}</p>
          {:else}
            <p class="field-hint">{$t('pages.adminPrefixes.namespaceHint')}</p>
          {/if}
        </div>
      </div>

      <!-- What the label means here before the override, so repointing is a
           decision rather than a surprise. -->
      {#if !labelError && (resolving || resolved || resolvedFor)}
        <div class="resolves" class:resolves-busy={resolving}>
          {#if resolving}
            <p class="resolves-line">
              <Loader2 size={13} class="animate-spin" />
              {$t('pages.adminPrefixes.resolvesChecking', { values: { label: form.label.trim() } })}
            </p>
          {:else if resolved}
            <p class="resolves-line">
              {$t('pages.adminPrefixes.resolvesNow', { values: { label: resolvedFor } })}
              <span class="source-badge">
                {$t('pages.adminPrefixes.sourceFrom', {
                  values: { source: SOURCE_LABEL[resolved.source] || resolved.source },
                })}
              </span>
            </p>
            <p class="resolves-ns">{resolved.namespace}</p>
          {:else if resolvedFor}
            <p class="resolves-line">
              <Check size={13} />
              {$t('pages.adminPrefixes.resolvesNothing', { values: { label: resolvedFor } })}
            </p>
          {/if}
        </div>
      {/if}

      {#if conflict}
        <div class="conflict">
          <p class="conflict-head">
            <CircleAlert size={14} />
            {$t('pages.adminPrefixes.conflictTitle', { values: { label: conflict.label } })}
          </p>
          <p class="conflict-ns">{conflict.namespace}</p>
          <p class="conflict-body">
            {$t('pages.adminPrefixes.conflictBody', { values: { label: conflict.label } })}
          </p>
          <button class="btn btn-sm" on:click={repointFromConflict} disabled={saving || !formValid}>
            {#if saving}<Loader2 size={14} class="animate-spin" />{:else}<Replace size={14} />{/if}
            {$t('pages.adminPrefixes.conflictRepoint')}
          </button>
        </div>
      {/if}

      <div class="form-actions">
        <button class="btn btn-sm btn-ghost" on:click={closeForm} disabled={saving}>
          {$t('system.cancel')}
        </button>
        <button class="btn btn-sm" on:click={submitCreate} disabled={saving || !formValid}>
          {#if saving}<Loader2 size={14} class="animate-spin" />{/if}
          {$t('pages.adminPrefixes.createOverride')}
        </button>
      </div>
    </section>
  {/if}

  {#if loading && overrides.length === 0}
    <div class="loading"><Loader2 size={24} class="animate-spin" /> {$t('system.loading')}</div>
  {:else if overrides.length === 0 && !showForm}
    <div class="empty-state">
      <Tags size={32} />
      <p class="empty-title">{$t('pages.adminPrefixes.emptyTitle')}</p>
      <p class="empty-body">{$t('pages.adminPrefixes.emptyBody')}</p>
    </div>
  {:else if overrides.length > 0}
    <ul class="override-list">
      {#each overrides as o (o.label)}
        <li class="override">
          <div class="override-main">
            <span class="prefix-chip">{o.label}:</span>
            {#if editingLabel === o.label}
              <div class="edit-field">
                <!-- The prefix chip beside it is the visible label; the input
                     carries its own so a screen reader hears what it edits. -->
                <input
                  aria-label={$t('pages.adminPrefixes.namespaceField')}
                  bind:value={editNamespace}
                  autocomplete="off"
                  spellcheck="false"
                  aria-invalid={!!editNamespaceError}
                />
                {#if editNamespaceError}
                  <p class="field-error">{namespaceMessage(editNamespaceError)}</p>
                {/if}
                {#if editError}
                  <p class="field-error">{editError}</p>
                {/if}
              </div>
            {:else}
              <span class="namespace">{o.namespace}</span>
              <span class="updated">{$t('pages.adminPrefixes.updated', { values: { date: formatDate(o.updated_at) } })}</span>
            {/if}
          </div>
          <div class="row-actions">
            {#if editingLabel === o.label}
              <button class="btn btn-sm btn-ghost" on:click={cancelEdit} disabled={editSaving}>
                {$t('system.cancel')}
              </button>
              <button class="btn btn-sm" on:click={submitEdit} disabled={editSaving || !!editNamespaceError}>
                {#if editSaving}<Loader2 size={14} class="animate-spin" />{/if}
                {$t('system.save')}
              </button>
            {:else}
              <button class="btn btn-sm btn-ghost" on:click={() => openEdit(o)}>
                <Replace size={14} /> {$t('pages.adminPrefixes.repoint')}
              </button>
              <button class="btn btn-sm btn-ghost btn-remove" on:click={() => removeTarget = o}>
                <Trash2 size={14} /> {$t('pages.adminPrefixes.remove')}
              </button>
            {/if}
          </div>
        </li>
      {/each}
    </ul>
  {/if}
</div>

{#if removeTarget}
  <ConfirmModal
    title={$t('pages.adminPrefixes.removeTitle', { values: { label: removeTarget.label } })}
    message={$t('pages.adminPrefixes.removeMessage', { values: { label: removeTarget.label } })}
    confirmLabel={$t('pages.adminPrefixes.removeConfirm')}
    confirmVariant="warning"
    loading={removing}
    on:confirm={doRemove}
    on:cancel={() => removeTarget = null}
  />
{/if}

<style>
  .admin-prefixes { max-width: 1000px; }

  .header-row {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 1rem;
    margin-bottom: 1rem;
  }
  .header-text { min-width: 0; }
  h2 {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    margin: 0;
  }
  .subtitle {
    margin: 0.35rem 0 0;
    font-size: 0.86rem;
    color: var(--ink-500);
    line-height: 1.45;
    overflow-wrap: break-word;
  }

  .card {
    background: white;
    border: 1px solid var(--line-soft);
    border-radius: 12px;
    padding: 1rem 1.1rem;
    margin-bottom: 1rem;
  }
  .card h3 { margin: 0; font-size: 1.02rem; }
  .explain {
    margin: 0.35rem 0 0.9rem;
    font-size: 0.86rem;
    color: var(--ink-600, #4b5563);
    line-height: 1.45;
    overflow-wrap: break-word;
  }

  .form-grid {
    display: grid;
    grid-template-columns: minmax(0, 12rem) minmax(0, 1fr);
    gap: 0.75rem 1rem;
  }
  .form-group { display: flex; flex-direction: column; gap: 0.3rem; min-width: 0; }
  .form-group label { font-size: 0.85rem; font-weight: 600; }
  .form-group input {
    padding: 0.5rem 0.65rem;
    border: 1px solid var(--line-soft);
    border-radius: 8px;
    font-size: 0.9rem;
    font-family: 'IBM Plex Mono', monospace;
    background: white;
    min-height: 40px;
  }
  .field-hint { margin: 0; font-size: 0.78rem; color: var(--ink-500); overflow-wrap: break-word; }
  .field-error { margin: 0; font-size: 0.78rem; color: #c62828; overflow-wrap: break-word; }

  /* What the label means here today, above the decision to change it. */
  .resolves {
    margin-top: 0.9rem;
    padding: 0.6rem 0.75rem;
    border: 1px dashed var(--line-soft);
    border-radius: 10px;
    background: var(--bg-accent, #f8f9fa);
  }
  .resolves-busy { opacity: 0.75; }
  .resolves-line {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 0.4rem;
    margin: 0;
    font-size: 0.84rem;
    color: var(--ink-600, #4b5563);
    overflow-wrap: break-word;
  }
  .resolves-ns {
    margin: 0.25rem 0 0;
    font-family: 'IBM Plex Mono', monospace;
    font-size: 0.82rem;
    overflow-wrap: anywhere;
  }
  .source-badge {
    padding: 0.1rem 0.5rem;
    border-radius: 20px;
    font-size: 0.74rem;
    font-weight: 600;
    background: #e3f2fd;
    color: #1565c0;
    overflow-wrap: break-word;
  }

  .conflict {
    margin-top: 0.9rem;
    padding: 0.7rem 0.8rem;
    border: 1px solid #fcd9b6;
    border-radius: 10px;
    background: #fef3c7;
  }
  .conflict-head {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    margin: 0;
    font-size: 0.86rem;
    font-weight: 700;
    color: #92400e;
    overflow-wrap: break-word;
  }
  .conflict-ns {
    margin: 0.35rem 0 0;
    font-family: 'IBM Plex Mono', monospace;
    font-size: 0.82rem;
    color: #92400e;
    overflow-wrap: anywhere;
  }
  .conflict-body {
    margin: 0.35rem 0 0.7rem;
    font-size: 0.84rem;
    color: #92400e;
    line-height: 1.45;
    overflow-wrap: break-word;
  }

  .form-actions {
    display: flex;
    justify-content: flex-end;
    gap: 0.5rem;
    margin-top: 1rem;
  }

  .loading {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 2rem;
    justify-content: center;
    color: var(--ink-500);
  }
  .card-error {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    margin: 0 0 0.75rem;
    padding: 0.5rem 0.7rem;
    border-radius: 8px;
    font-size: 0.84rem;
    background: #fee2e2;
    color: #991b1b;
    overflow-wrap: break-word;
  }

  .empty-state {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 0.5rem;
    padding: 3rem 1rem;
    color: var(--ink-500);
    text-align: center;
  }
  .empty-title { margin: 0; font-weight: 600; }
  .empty-body { margin: 0; max-width: 34rem; font-size: 0.86rem; line-height: 1.45; overflow-wrap: break-word; }

  .override-list { list-style: none; margin: 0; padding: 0; }
  .override {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 0.75rem;
    padding: 0.8rem 0.9rem;
    background: white;
    border: 1px solid var(--line-soft);
    border-radius: 12px;
    margin-bottom: 0.5rem;
  }
  .override-main {
    display: flex;
    align-items: baseline;
    flex-wrap: wrap;
    gap: 0.5rem;
    min-width: 0;
    flex: 1 1 auto;
  }
  .prefix-chip {
    padding: 0.15rem 0.55rem;
    border-radius: 20px;
    background: #ede7f6;
    color: #4527a0;
    font-family: 'IBM Plex Mono', monospace;
    font-size: 0.82rem;
    font-weight: 700;
    overflow-wrap: anywhere;
  }
  .namespace {
    font-family: 'IBM Plex Mono', monospace;
    font-size: 0.84rem;
    overflow-wrap: anywhere;
    min-width: 0;
  }
  .updated { font-size: 0.76rem; color: var(--ink-500); overflow-wrap: break-word; }
  .edit-field { display: flex; flex-direction: column; gap: 0.3rem; flex: 1 1 16rem; min-width: 0; }
  .edit-field input {
    padding: 0.5rem 0.65rem;
    border: 1px solid var(--line-soft);
    border-radius: 8px;
    font-family: 'IBM Plex Mono', monospace;
    font-size: 0.84rem;
    background: white;
    min-height: 40px;
  }
  .row-actions { display: flex; gap: 0.35rem; flex-shrink: 0; }
  .btn-remove { color: #c62828; }
  .btn-remove:hover { background: #ffebee; }

  @media (max-width: 720px) {
    .header-row { flex-direction: column; align-items: stretch; }
    .form-grid { grid-template-columns: 1fr; }
    .override { flex-direction: column; align-items: stretch; }
    /* app.css stretches every .btn to the full row below 720px, which is right
       for a button that owns its row. These share one, so they split it — and
       each stays a 40px tap target, which .btn-sm on its own is not. */
    .row-actions .btn, .form-actions .btn { width: auto; flex: 1 1 0; }
    .btn-sm { min-height: 40px; }
  }

  :global(:is([data-theme="dark"], .dark)) .card,
  :global(:is([data-theme="dark"], .dark)) .override { background: var(--bg-strong); }
  :global(:is([data-theme="dark"], .dark)) .form-group input,
  :global(:is([data-theme="dark"], .dark)) .edit-field input { background: var(--bg-strong); color: var(--ink-900); }
  :global(:is([data-theme="dark"], .dark)) .explain { color: var(--ink-400, #9ca3af); }
  :global(:is([data-theme="dark"], .dark)) .resolves { background: rgba(255,255,255,0.04); }
  :global(:is([data-theme="dark"], .dark)) .resolves-line { color: var(--ink-400, #9ca3af); }
  :global(:is([data-theme="dark"], .dark)) .source-badge { background: rgba(59,130,246,0.2); color: #93c5fd; }
  :global(:is([data-theme="dark"], .dark)) .prefix-chip { background: rgba(139,92,246,0.2); color: #c4b5fd; }
  :global(:is([data-theme="dark"], .dark)) .card-error { background: rgba(239,68,68,0.18); color: #fca5a5; }
  :global(:is([data-theme="dark"], .dark)) .field-error { color: #fca5a5; }
  :global(:is([data-theme="dark"], .dark)) .conflict { background: rgba(245,158,11,0.16); border-color: rgba(245,158,11,0.35); }
  :global(:is([data-theme="dark"], .dark)) .conflict-head,
  :global(:is([data-theme="dark"], .dark)) .conflict-ns,
  :global(:is([data-theme="dark"], .dark)) .conflict-body { color: #fcd34d; }
  :global(:is([data-theme="dark"], .dark)) .btn-remove { color: #fca5a5; }
  :global(:is([data-theme="dark"], .dark)) .btn-remove:hover { background: rgba(239,68,68,0.15); }
</style>
