<script>
  import { onMount } from 'svelte';
  import { t } from 'svelte-i18n';
  import { navigate, Link } from '../lib/router/index.js';
  import { Library, Plus, Trash2, Search, Loader2, Tag, ChevronRight, Info, ChevronDown, Globe, Lock, CheckSquare, X, User, Building2, BookOpen } from 'lucide-svelte';
  import { listVocabularies, createVocabulary, deleteVocabulary, listDatasets, listPublicUsers, listOrganisations } from '../lib/api.js';
  import { isAdmin } from '../lib/stores.js';
  import ConfirmModal from '../components/ConfirmModal.svelte';
  import BulkActionBar from '../components/BulkActionBar.svelte';
  import Avatar from '../components/Avatar.svelte';
  import Select from '../components/Select.svelte';

  let vocabularies = [];
  let vocabDatasets = [];
  let loading = false;
  let error = '';
  let search = '';
  let showInfo = false;

  let userMap = {};
  let orgMap = {};
  let organisations = [];

  // Create modal
  let showCreate = false;
  let createForm = { title: '', namespace: '', description: '', is_public: false, owner_type: 'user', owner_org_id: '' };
  let createError = '';
  let createLoading = false;

  // Confirm delete
  let deleteTarget = null;
  let deleteLoading = false;

  // ── Multi-select ──────────────────────────────────────────────────────────
  let selectMode = false;
  let selected = new Set();
  let confirmBulkDelete = false;
  let bulkDeleting = false;

  function toggleSelect(id) {
    if (selected.has(id)) { selected.delete(id); } else { selected.add(id); }
    selected = selected;
  }
  function clearSelection() { selected.clear(); selected = selected; selectMode = false; }

  async function bulkDeleteVocabularies() {
    bulkDeleting = true;
    const toDelete = [...selected];
    for (const id of toDelete) {
      try { await deleteVocabulary(id); } catch {}
    }
    confirmBulkDelete = false;
    bulkDeleting = false;
    clearSelection();
    await load();
  }

  onMount(async () => {
    await load();
    const [users, orgs] = await Promise.all([
      listPublicUsers().catch(() => []),
      listOrganisations().catch(() => []),
    ]);
    userMap = Object.fromEntries((users || []).map(u => [String(u.id), u.username || u.email || u.id]));
    organisations = orgs || [];
    orgMap = Object.fromEntries(organisations.map(o => [String(o.id), o.name]));
  });

  async function load() {
    loading = true;
    error = '';
    try {
      [vocabularies, vocabDatasets] = await Promise.all([
        listVocabularies(),
        listDatasets().then(ds => ds.filter(d => d.graph_role === 'vocabulary')).catch(() => []),
      ]);
    } catch (e) {
      error = e.message;
    }
    loading = false;
  }

  function resolveOwner(item) {
    if (!item.owner_id) return null;
    if (item.owner_type === 'organisation') return orgMap[String(item.owner_id)] || item.owner_id;
    return userMap[String(item.owner_id)] || item.owner_id;
  }

  async function handleCreate() {
    createError = '';
    createLoading = true;
    try {
      const payload = {
        title: createForm.title,
        namespace: createForm.namespace,
        description: createForm.description,
        is_public: createForm.is_public,
      };
      if (createForm.owner_type === 'organisation' && createForm.owner_org_id) {
        payload.owner_type = 'organisation';
        payload.owner_id = createForm.owner_org_id;
      }
      await createVocabulary(payload);
      showCreate = false;
      createForm = { title: '', namespace: '', description: '', is_public: false, owner_type: 'user', owner_org_id: '' };
      await load();
    } catch (e) {
      createError = e.message;
    }
    createLoading = false;
  }

  async function handleDelete(id) {
    deleteLoading = true;
    try {
      await deleteVocabulary(id);
    } catch (e) {
      alert(e.message);
    }
    deleteTarget = null;
    deleteLoading = false;
    await load();
  }

  $: filtered = vocabularies.filter(v =>
    !search || v.title.toLowerCase().includes(search.toLowerCase()) || v.id.toLowerCase().includes(search.toLowerCase())
  );
</script>

<div class="space-y-6">
  <!-- Header row -->
  <div class="flex flex-wrap items-center justify-between gap-4">
    <div class="flex items-center gap-3">
      <Library size={22} class="text-[var(--brand-500)]" />
      <h2 class="text-xl font-semibold m-0">{$t('pages.vocabularyRegistry.heading')}</h2>
      <span class="text-sm text-[var(--ink-500)]">{vocabularies.length === 1 ? $t('pages.vocabularyRegistry.countSingular', { values: { count: vocabularies.length } }) : $t('pages.vocabularyRegistry.countPlural', { values: { count: vocabularies.length } })}</span>
    </div>
    <div class="flex items-center gap-2">
      <button class="info-btn" on:click={() => showInfo = !showInfo} aria-expanded={showInfo}>
        <Info size={14} />
        {$t('pages.vocabularyRegistry.about')}
        <ChevronDown size={13} class="transition-transform {showInfo ? 'rotate-180' : ''}" />
      </button>
      {#if $isAdmin}
        <button
          class="info-btn"
          class:select-mode-active={selectMode}
          on:click={() => { selectMode = !selectMode; if (!selectMode) clearSelection(); }}
          title={selectMode ? $t('pages.vocabularyRegistry.exitSelectionMode') : $t('pages.vocabularyRegistry.selectVocabularies')}
        >
          <CheckSquare size={14} />
          {selectMode ? $t('pages.vocabularyRegistry.cancelSelect') : $t('pages.vocabularyRegistry.select')}
        </button>
        <button class="btn btn-primary btn-sm" on:click={() => showCreate = true}>
          <Plus size={16} />
          {$t('pages.vocabularyRegistry.newVocabulary')}
        </button>
      {/if}
    </div>
  </div>

  {#if showInfo}
    <div class="info-panel">
      <p><strong>{$t('pages.vocabularyRegistry.infoTitle')}</strong> {$t('pages.vocabularyRegistry.infoIntro')} <em>draft → staged → published</em> {$t('pages.vocabularyRegistry.infoLifecycle')}</p>
      <ul>
        <li><strong>{$t('pages.vocabularyRegistry.infoUploadLabel')}</strong> {$t('pages.vocabularyRegistry.infoUpload')}</li>
        <li><strong>{$t('pages.vocabularyRegistry.infoDraftLabel')}</strong> {$t('pages.vocabularyRegistry.infoDraft')}</li>
        <li><strong>{$t('pages.vocabularyRegistry.infoDiffLabel')}</strong> {$t('pages.vocabularyRegistry.infoDiff')}</li>
        <li><strong>{$t('pages.vocabularyRegistry.infoPublishedLabel')}</strong> {$t('pages.vocabularyRegistry.infoPublished')} <code>/api/vocabularies/&#123;id&#125;/latest/data</code></li>
      </ul>
      <Link to="/docs" class="docs-link">{$t('pages.vocabularyRegistry.viewDocs')}</Link>
    </div>
  {/if}

  <!-- Search/Filter bar -->
  <div class="filter-row">
    <div class="filter-input">
      <Search size={14} />
      <input
        id="vocabulary-search"
        type="text"
        placeholder={$t('pages.vocabularyRegistry.filterPlaceholder')}
        bind:value={search}
      />
      {#if search}
        <button class="filter-clear" on:click={() => search = ''} aria-label={$t('system.clear')}><X size={12} /></button>
      {/if}
    </div>
    <span class="filter-count">{$t('pages.vocabularyRegistry.filterCount', { values: { shown: filtered.length, total: vocabularies.length } })}</span>
  </div>

  {#if loading}
    <div class="flex items-center justify-center py-16 text-[var(--ink-400)]">
      <Loader2 size={24} class="animate-spin mr-2" />
      {$t('pages.vocabularyRegistry.loadingVocabularies')}
    </div>
  {:else if error}
    <div class="p-4 rounded-xl bg-red-50 border border-red-200 text-red-700 text-sm">{error}</div>
  {:else if filtered.length === 0}
    <div class="text-center py-16 text-[var(--ink-400)]">
      {#if search}{$t('pages.vocabularyRegistry.noMatch')}{:else}{$t('pages.vocabularyRegistry.noneRegistered')}{/if}
    </div>
  {:else}
    <div class="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
      {#each filtered as vocab (vocab.id)}
        {@const isSelected = selected.has(vocab.id)}
        <div
          class="card group relative cursor-pointer hover:border-[var(--brand-300)] transition-colors"
          class:card-selected={isSelected}
          on:click={() => {
            if (selectMode) { toggleSelect(vocab.id); }
            else { navigate(`/vocabularies/${vocab.id}`); }
          }}
          role="button"
          tabindex="0"
          on:keydown={(e) => {
            if (e.key === 'Enter') {
              if (selectMode) toggleSelect(vocab.id);
              else navigate(`/vocabularies/${vocab.id}`);
            }
            if (e.key === ' ' && selectMode) { e.preventDefault(); toggleSelect(vocab.id); }
          }}
        >
          <!-- Select mode checkbox -->
          {#if selectMode}
            <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
            <div class="check-wrap" on:click|stopPropagation>
              <input
                type="checkbox"
                checked={isSelected}
                on:change={() => toggleSelect(vocab.id)}
                aria-label={$t('pages.vocabularyRegistry.selectNamed', { values: { title: vocab.title } })}
                class="check"
              />
            </div>
          {/if}
          <div class="flex items-start justify-between gap-2">
            <div class="min-w-0 flex-1">
              <div class="flex items-center gap-1.5 flex-wrap">
                <h3 class="font-semibold text-[var(--ink-900)] truncate m-0">{vocab.title}</h3>
                {#if vocab.is_public}
                  <span class="vis-badge vis-public"><Globe size={9} /> {$t('pages.vocabularyRegistry.public')}</span>
                {:else}
                  <span class="vis-badge vis-private"><Lock size={9} /> {$t('pages.vocabularyRegistry.private')}</span>
                {/if}
              </div>
              <code class="text-xs text-[var(--ink-400)] break-all">{vocab.id}</code>
            </div>
            <ChevronRight size={18} class="text-[var(--ink-300)] mt-0.5 shrink-0 group-hover:text-[var(--brand-500)] transition-colors" />
          </div>

          {#if vocab.description}
            <p class="text-xs text-[var(--ink-500)] mt-1.5 mb-0 line-clamp-2">{vocab.description}</p>
          {/if}

          <div class="mt-2 text-xs text-[var(--ink-500)] space-y-1">
            {#if vocab.namespace}
              <div class="truncate"><span class="font-medium">{$t('pages.vocabularyRegistry.namespaceShort')}</span> {vocab.namespace}</div>
            {/if}
            {#if vocab.owner_id}
              {@const ownerName = resolveOwner(vocab)}
              <div class="flex items-center gap-1.5">
                <Avatar
                  kind={vocab.owner_type === 'organisation' ? 'organisation' : 'user'}
                  id={vocab.owner_id}
                  name={ownerName || vocab.owner_id}
                  size={18}
                />
                <span class="text-[var(--ink-400)]">{vocab.owner_type === 'organisation' ? $t('pages.vocabularyRegistry.orgLabel') : $t('pages.vocabularyRegistry.byLabel')}</span>
                <span class="truncate font-medium">{ownerName || vocab.owner_id}</span>
              </div>
            {/if}
            <div class="flex items-center gap-3">
              <span><Tag size={11} class="inline mr-1" />{vocab.version_count === 1 ? $t('pages.vocabularyRegistry.versionSingular', { values: { count: vocab.version_count } }) : $t('pages.vocabularyRegistry.versionPlural', { values: { count: vocab.version_count } })}</span>
              {#if vocab.latest_published}
                <span class="px-1.5 py-0.5 rounded-md bg-green-100 text-green-700 font-medium">v{vocab.latest_published}</span>
              {/if}
              {#if vocab.latest_draft}
                <span class="px-1.5 py-0.5 rounded-md bg-amber-100 text-amber-700 font-medium">{$t('pages.vocabularyRegistry.draftVersion', { values: { version: vocab.latest_draft } })}</span>
              {/if}
            </div>
          </div>

          {#if $isAdmin}
            <button
              class="absolute top-3 right-3 p-1.5 rounded-lg text-red-400 opacity-0 group-hover:opacity-100 transition-opacity hover:bg-red-50"
              on:click|stopPropagation={() => deleteTarget = vocab}
              aria-label={$t('pages.vocabularyRegistry.deleteVocabulary')}
            >
              <Trash2 size={14} />
            </button>
          {/if}
        </div>
      {/each}
    </div>
  {/if}

  {#if vocabDatasets.length > 0}
    <div class="role-ds-section">
      <h3 class="role-ds-heading">{$t('pages.vocabularyRegistry.taggedDatasetsHeading')}</h3>
      <p class="role-ds-sub">{$t('pages.vocabularyRegistry.taggedDatasetsSub')}</p>
      <div class="role-ds-list">
        {#each vocabDatasets as ds}
          <a class="role-ds-card" href="/datasets/{ds.id}">
            <span class="role-ds-name">{ds.name}</span>
            {#if ds.description}<span class="role-ds-desc">{ds.description}</span>{/if}
          </a>
        {/each}
      </div>
    </div>
  {/if}
</div>

<!-- Create modal -->
{#if showCreate}
  <div class="modal-backdrop" on:click={() => showCreate = false} role="presentation" on:keydown={(e) => e.key === 'Escape' && (showCreate = false)}>
    <div class="modal-box" on:click|stopPropagation on:keydown|stopPropagation role="dialog" aria-modal="true" aria-label={$t('pages.vocabularyRegistry.createVocabularyAria')} tabindex="-1">
      <!-- Modal header -->
      <div class="create-modal-header">
        <div class="create-modal-icon"><BookOpen size={20} /></div>
        <div>
          <h3 class="create-modal-title">{$t('pages.vocabularyRegistry.newVocabulary')}</h3>
          <p class="create-modal-subtitle">{$t('pages.vocabularyRegistry.createSubtitle')}</p>
        </div>
        <button class="create-modal-close" type="button" on:click={() => showCreate = false} aria-label={$t('system.close')}><X size={16} /></button>
      </div>

      <form on:submit|preventDefault={handleCreate} class="create-modal-body">
        <!-- Title -->
        <div class="field">
          <label class="label" for="vocab-title">{$t('pages.vocabularyRegistry.titleLabel')} <span class="req">*</span></label>
          <input id="vocab-title" type="text" class="input" required bind:value={createForm.title}
            placeholder={$t('pages.vocabularyRegistry.titlePlaceholder')} />
          <span class="field-hint">{$t('pages.vocabularyRegistry.titleHint')}</span>
        </div>

        <!-- Namespace URI -->
        <div class="field">
          <label class="label" for="vocab-ns">{$t('pages.vocabularyRegistry.namespaceLabel')}</label>
          <input id="vocab-ns" type="text" class="input" bind:value={createForm.namespace}
            placeholder="https://example.org/vocab/materials/" />
          <span class="field-hint">{$t('pages.vocabularyRegistry.namespaceHint')}</span>
        </div>

        <!-- Description -->
        <div class="field">
          <label class="label" for="vocab-desc">{$t('pages.vocabularyRegistry.descriptionLabel')}</label>
          <textarea id="vocab-desc" class="input" rows="2" bind:value={createForm.description}
            placeholder={$t('pages.vocabularyRegistry.descriptionPlaceholder')}></textarea>
        </div>

        <!-- Owner -->
        {#if organisations.length > 0}
          <div class="field">
            <span class="label">{$t('pages.vocabularyRegistry.ownership')}</span>
            <div class="owner-options">
              <label class="owner-opt" class:owner-opt-selected={createForm.owner_type === 'user'}>
                <input type="radio" bind:group={createForm.owner_type} value="user" class="sr-only" />
                <User size={14} />
                <span>{$t('pages.vocabularyRegistry.personalAccount')}</span>
              </label>
              <label class="owner-opt" class:owner-opt-selected={createForm.owner_type === 'organisation'}>
                <input type="radio" bind:group={createForm.owner_type} value="organisation" class="sr-only" />
                <Building2 size={14} />
                <span>{$t('pages.vocabularyRegistry.organisation')}</span>
              </label>
            </div>
            {#if createForm.owner_type === 'organisation'}
              <Select
                class="mt-2"
                bind:value={createForm.owner_org_id}
                options={[{ value: '', label: $t('pages.vocabularyRegistry.selectOrganisation') }, ...organisations.map(org => ({ value: org.id, label: org.name }))]}
              />
            {/if}
          </div>
        {/if}

        <!-- Visibility -->
        <div class="field">
          <span class="label">{$t('pages.vocabularyRegistry.visibility')}</span>
          <div class="vis-options">
            <label class="vis-opt" class:vis-opt-selected={createForm.is_public}>
              <input type="checkbox" bind:checked={createForm.is_public} class="sr-only" />
              <Globe size={14} />
              <div>
                <span class="vis-opt-name">{$t('pages.vocabularyRegistry.public')}</span>
                <span class="vis-opt-desc">{$t('pages.vocabularyRegistry.publicDesc')}</span>
              </div>
            </label>
          </div>
        </div>

        {#if createError}
          <div class="create-error">{createError}</div>
        {/if}

        <div class="create-modal-footer">
          <button type="button" class="btn btn-ghost" on:click={() => showCreate = false}>{$t('system.cancel')}</button>
          <button type="submit" class="btn btn-primary" disabled={createLoading || !createForm.title.trim()}>
            {#if createLoading}<Loader2 size={14} class="animate-spin" />{:else}<Plus size={14} />{/if}
            {$t('pages.vocabularyRegistry.createVocabulary')}
          </button>
        </div>
      </form>
    </div>
  </div>
{/if}

<!-- Delete confirm -->
{#if deleteTarget}
  <ConfirmModal
    title={$t('pages.vocabularyRegistry.deleteConfirmTitle', { values: { title: deleteTarget.title } })}
    message={$t('pages.vocabularyRegistry.deleteConfirmMessage')}
    confirmLabel={$t('system.delete')}
    loading={deleteLoading}
    on:confirm={() => handleDelete(deleteTarget.id)}
    on:cancel={() => deleteTarget = null}
  />
{/if}

<!-- Bulk delete confirm -->
{#if confirmBulkDelete}
  <ConfirmModal
    title={selected.size === 1 ? $t('pages.vocabularyRegistry.bulkDeleteTitleSingular', { values: { count: selected.size } }) : $t('pages.vocabularyRegistry.bulkDeleteTitlePlural', { values: { count: selected.size } })}
    message={selected.size === 1 ? $t('pages.vocabularyRegistry.bulkDeleteMessageSingular', { values: { count: selected.size } }) : $t('pages.vocabularyRegistry.bulkDeleteMessagePlural', { values: { count: selected.size } })}
    confirmLabel={selected.size === 1 ? $t('pages.vocabularyRegistry.bulkDeleteConfirmSingular', { values: { count: selected.size } }) : $t('pages.vocabularyRegistry.bulkDeleteConfirmPlural', { values: { count: selected.size } })}
    loading={bulkDeleting}
    on:confirm={bulkDeleteVocabularies}
    on:cancel={() => confirmBulkDelete = false}
  />
{/if}

<!-- Bulk action bar -->
{#if $isAdmin}
  <BulkActionBar
    count={selected.size}
    total={filtered.length}
    itemLabel={$t('pages.vocabularyRegistry.itemLabel')}
    on:clearSelection={clearSelection}
    on:selectAll={() => { filtered.forEach(v => selected.add(v.id)); selected = selected; }}
  >
    <button class="bulk-action-btn danger" on:click={() => confirmBulkDelete = true} disabled={bulkDeleting}>
      <Trash2 size={13} /> {selected.size === 1 ? $t('pages.vocabularyRegistry.bulkDeleteConfirmSingular', { values: { count: selected.size } }) : $t('pages.vocabularyRegistry.bulkDeleteConfirmPlural', { values: { count: selected.size } })}
    </button>
  </BulkActionBar>
{/if}

<style>
  .info-panel {
    background: #f0f9ff;
    border: 1px solid #bae6fd;
    border-radius: 10px;
    padding: 1rem;
    font-size: 0.85rem;
    color: var(--ink-700, #374151);
    line-height: 1.55;
  }
  .info-panel p { margin: 0 0 0.5rem; }
  .info-panel ul { margin: 0 0 0.5rem; padding-left: 1.25rem; }
  .info-panel li { margin-bottom: 0.25rem; }
  :global(.docs-link) { color: var(--brand-600, #0d7490); font-weight: 500; text-decoration: none; }
  :global(.docs-link:hover) { text-decoration: underline; }
  .info-btn {
    display: inline-flex;
    align-items: center;
    gap: 0.3rem;
    padding: 0.375rem 0.65rem;
    border: 1px solid var(--line-soft, #d1d5db);
    border-radius: 8px;
    background: white;
    cursor: pointer;
    font-size: 0.8rem;
    color: var(--ink-600);
    transition: all 0.12s;
  }
  .info-btn:hover { border-color: var(--brand-400); color: var(--brand-600); }
  .select-mode-active { border-color: var(--brand-400); color: var(--brand-600); background: var(--brand-50, #eef2ff); }
  .modal-backdrop {
    position: fixed; inset: 0; background: rgba(0,0,0,0.4);
    display: flex; align-items: center; justify-content: center; z-index: 50;
    padding: 1rem;
  }
  .modal-box {
    background: white; border-radius: 1.25rem;
    width: min(520px, 100%); max-height: 90vh; overflow-y: auto;
    box-shadow: 0 24px 64px rgba(0,0,0,0.18);
    display: flex; flex-direction: column;
  }
  /* Create modal layout */
  .create-modal-header {
    display: flex; align-items: flex-start; gap: 0.875rem;
    padding: 1.5rem 1.5rem 0;
  }
  .create-modal-icon {
    width: 2.5rem; height: 2.5rem; border-radius: 0.75rem;
    background: var(--brand-50, #eef2ff); color: var(--brand-600, #4f46e5);
    display: flex; align-items: center; justify-content: center; flex-shrink: 0;
  }
  .create-modal-title { font-size: 1.05rem; font-weight: 700; margin: 0; color: var(--ink-900, #0f172a); }
  .create-modal-subtitle { font-size: 0.8rem; color: var(--ink-400, #94a3b8); margin: 0.15rem 0 0; }
  .create-modal-close {
    margin-left: auto; width: 2rem; height: 2rem; border-radius: 0.5rem;
    border: none; cursor: pointer; background: transparent; color: var(--ink-400);
    display: flex; align-items: center; justify-content: center; flex-shrink: 0;
    transition: all 0.12s;
  }
  .create-modal-close:hover { background: var(--bg-soft); color: var(--ink-700); }
  .create-modal-body { display: flex; flex-direction: column; gap: 1.1rem; padding: 1.25rem 1.5rem; }
  .create-modal-footer {
    display: flex; gap: 0.75rem; justify-content: flex-end;
    padding-top: 0.25rem; border-top: 1px solid var(--line-soft, #e2e8f0);
    padding-bottom: 0.25rem;
  }
  /* Fields */
  .field { display: flex; flex-direction: column; gap: 0.25rem; }
  .label { display: block; font-size: 0.825rem; font-weight: 600; color: var(--ink-700); }
  .req { color: #ef4444; }
  .field-hint { font-size: 0.75rem; color: var(--ink-400); }
  .input {
    width: 100%; padding: 0.5rem 0.75rem; border: 1px solid var(--line-soft);
    border-radius: 0.75rem; font-size: 0.875rem; box-sizing: border-box; font-family: inherit;
    transition: border-color 0.15s; outline: none;
  }
  .input:focus { border-color: var(--brand-400, #818cf8); box-shadow: 0 0 0 3px rgba(99,102,241,0.1); }
  textarea.input { resize: vertical; }
  /* Owner options */
  .owner-options { display: flex; gap: 0.5rem; }
  .owner-opt {
    flex: 1; display: flex; align-items: center; gap: 0.5rem; padding: 0.5rem 0.75rem;
    border: 1px solid var(--line-soft); border-radius: 0.75rem; cursor: pointer;
    font-size: 0.85rem; font-weight: 500; color: var(--ink-600); transition: all 0.12s;
    user-select: none;
  }
  .owner-opt:hover { border-color: var(--brand-300); color: var(--brand-600); }
  .owner-opt-selected { border-color: var(--brand-400); background: var(--brand-50, #eef2ff); color: var(--brand-700); }
  /* Visibility option */
  .vis-opt {
    display: flex; align-items: flex-start; gap: 0.6rem; padding: 0.6rem 0.875rem;
    border: 1px solid var(--line-soft); border-radius: 0.75rem; cursor: pointer;
    transition: all 0.12s; user-select: none;
  }
  .vis-opt:hover { border-color: var(--brand-300); }
  .vis-opt-selected { border-color: #22c55e; background: #f0fdf4; }
  .vis-opt-name { font-size: 0.85rem; font-weight: 500; color: var(--ink-800); display: block; }
  .vis-opt-desc { font-size: 0.75rem; color: var(--ink-400); display: block; }
  .create-error { padding: 0.6rem 0.875rem; background: #fef2f2; border: 1px solid #fecaca; border-radius: 0.75rem; font-size: 0.825rem; color: #dc2626; }
  .sr-only { position: absolute; width: 1px; height: 1px; overflow: hidden; clip: rect(0,0,0,0); }
  .vis-badge { display: inline-flex; align-items: center; gap: 0.2rem; font-size: 0.65rem; font-weight: 600; padding: 0.1rem 0.4rem; border-radius: 999px; white-space: nowrap; }
  .vis-public { background: #dcfce7; color: #15803d; }
  .vis-private { background: #fef3c7; color: #92400e; }
  .line-clamp-2 { display: -webkit-box; -webkit-line-clamp: 2; line-clamp: 2; -webkit-box-orient: vertical; overflow: hidden; }
  .btn { display: inline-flex; align-items: center; gap: 0.375rem; padding: 0.5rem 1rem; border-radius: 0.75rem; font-size: 0.875rem; font-weight: 500; cursor: pointer; border: none; transition: all 0.15s; }
  .btn-primary { background: var(--brand-500, #6366f1); color: white; }
  .btn-primary:hover { background: var(--brand-600, #4f46e5); }
  .btn-ghost { background: transparent; color: var(--ink-500); }
  .btn-ghost:hover { background: var(--bg-soft); }
  .btn-sm { padding: 0.375rem 0.75rem; font-size: 0.8125rem; }
  .btn:disabled { opacity: 0.6; cursor: not-allowed; }
  .filter-row { display: flex; align-items: center; gap: 0.75rem; flex-wrap: wrap; }
  .filter-input {
    display: flex; align-items: center; gap: 0.5rem;
    border: 1px solid var(--line-soft); border-radius: 8px;
    padding: 0.375rem 0.65rem; background: white; flex: 1; min-width: 180px;
    color: var(--ink-400);
  }
  .filter-input input { border: none; outline: none; font-size: 0.875rem; flex: 1; color: var(--ink-900); background: transparent; }
  .filter-clear { background: none; border: none; cursor: pointer; color: var(--ink-400); padding: 0; display: flex; }
  .filter-clear:hover { color: var(--ink-700); }
  .filter-count { font-size: 0.8rem; color: var(--ink-400); white-space: nowrap; }
  .card-selected { border-color: var(--brand-400) !important; background: var(--brand-50, #eef2ff); }
  .check-wrap { position: absolute; top: 0.75rem; left: 0.75rem; }
  .check { width: 1rem; height: 1rem; accent-color: var(--brand-500); }

  .role-ds-section { margin-top: 2rem; padding-top: 1.5rem; border-top: 1px dashed var(--border, #e2e8f0); }
  .role-ds-heading { font-size: 1rem; font-weight: 700; color: var(--ink-800); margin: 0 0 0.25rem; }
  .role-ds-sub { font-size: 0.82rem; color: var(--ink-400); margin: 0 0 0.75rem; }
  .role-ds-list { display: flex; flex-direction: column; gap: 0.4rem; }
  .role-ds-card { display: flex; flex-direction: column; padding: 0.6rem 0.9rem; border: 1px solid #e2e8f0; border-radius: 8px; background: #f8fafc; text-decoration: none; color: inherit; transition: border-color 0.15s; }
  .role-ds-card:hover { border-color: var(--brand-300); background: #fdf4ff; }
  .role-ds-name { font-size: 0.88rem; font-weight: 600; color: var(--ink-900); }
  .role-ds-desc { font-size: 0.78rem; color: var(--ink-400); margin-top: 0.1rem; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }

  /* ---- Dark mode overrides (scoped rules out-specify global theme.css) ---- */
  :global(:is([data-theme="dark"], .dark)) .info-panel { background: rgba(59,130,246,0.1); border-color: rgba(59,130,246,0.3); color: var(--ink-700); }
  :global(:is([data-theme="dark"], .dark)) .info-btn,
  :global(:is([data-theme="dark"], .dark)) .filter-input { background: var(--bg-soft); }
  :global(:is([data-theme="dark"], .dark)) .select-mode-active,
  :global(:is([data-theme="dark"], .dark)) .create-modal-icon,
  :global(:is([data-theme="dark"], .dark)) .owner-opt-selected,
  :global(:is([data-theme="dark"], .dark)) .card-selected { background: var(--brand-100); }
  :global(:is([data-theme="dark"], .dark)) .modal-box { background: var(--bg-strong); }
  :global(:is([data-theme="dark"], .dark)) .req { color: #fca5a5; }
  :global(:is([data-theme="dark"], .dark)) .vis-opt-selected { border-color: #34d399; background: rgba(16,185,129,0.14); }
  :global(:is([data-theme="dark"], .dark)) .create-error { background: rgba(220,38,38,0.12); border-color: rgba(220,38,38,0.35); color: #fca5a5; }
  :global(:is([data-theme="dark"], .dark)) .vis-public { background: rgba(16,185,129,0.18); color: #6ee7b7; }
  :global(:is([data-theme="dark"], .dark)) .vis-private { background: rgba(245,158,11,0.18); color: #fcd34d; }
  :global(:is([data-theme="dark"], .dark)) .role-ds-card { border-color: var(--line-strong); background: var(--bg-soft); }
  :global(:is([data-theme="dark"], .dark)) .role-ds-card:hover { background: rgba(139,92,246,0.14); }
</style>
