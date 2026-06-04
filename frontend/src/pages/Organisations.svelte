<script>
  import { onMount } from 'svelte';
  import { listOrganisations, createOrganisation, deleteOrganisation, isLoggedIn } from '../lib/api.js';
  import { isAdmin } from '../lib/stores.js';
  import { Link } from '../lib/router/index.js';
  import { t } from 'svelte-i18n';
  import { Plus, Trash2, X, Search, Building2, Info, ChevronDown, Users } from 'lucide-svelte';
  import ConfirmModal from '../components/ConfirmModal.svelte';
  import BulkActionBar from '../components/BulkActionBar.svelte';
  import Avatar from '../components/Avatar.svelte';
  import PageHeader from '../components/PageHeader.svelte';

  let orgs = [];
  let search = '';
  let showInfo = false;
  let error = '';
  let showCreate = false;
  let newName = '';
  let newSlug = '';
  let newDescription = '';
  let slugManuallyEdited = false;

  $: if (!slugManuallyEdited) {
    newSlug = newName.toLowerCase().replace(/\s+/g, '-').replace(/[^a-z0-9-]/g, '');
  }

  onMount(fetchOrgs);

  async function fetchOrgs() {
    try {
      orgs = await listOrganisations();
    } catch (e) {
      error = e.message;
    }
  }

  async function handleCreate() {
    error = '';
    try {
      await createOrganisation({
        name: newName,
        slug: newSlug,
        description: newDescription || null,
      });
      showCreate = false;
      newName = '';
      newSlug = '';
      newDescription = '';
      slugManuallyEdited = false;
      await fetchOrgs();
    } catch (e) {
      error = e.message;
    }
  }

  let deleteOrgItem = null;
  let deleteOrgLoading = false;

  async function doDeleteOrg() {
    deleteOrgLoading = true;
    try {
      await deleteOrganisation(deleteOrgItem.id);
      deleteOrgItem = null;
      await fetchOrgs();
    } catch (e) {
      error = e.message;
      deleteOrgItem = null;
    }
    deleteOrgLoading = false;
  }

  // ── Multi-select ──────────────────────────────────────────────────────────
  let selected = new Set();
  let confirmBulkDelete = false;
  let bulkDeleting = false;

  function toggleSelect(id) {
    if (selected.has(id)) { selected.delete(id); } else { selected.add(id); }
    selected = selected;
  }
  function clearSelection() { selected.clear(); selected = selected; }

  $: allFilteredSelected = filtered.length > 0 && filtered.every(o => selected.has(o.id));
  $: someFilteredSelected = !allFilteredSelected && filtered.some(o => selected.has(o.id));

  let orgHeaderCheckbox = null;
  $: if (orgHeaderCheckbox) {
    orgHeaderCheckbox.indeterminate = someFilteredSelected;
    orgHeaderCheckbox.checked = allFilteredSelected;
  }

  function toggleSelectAll() {
    if (allFilteredSelected) { filtered.forEach(o => selected.delete(o.id)); }
    else { filtered.forEach(o => selected.add(o.id)); }
    selected = selected;
  }

  async function bulkDeleteOrgs() {
    bulkDeleting = true;
    const toDelete = [...selected];
    for (const id of toDelete) {
      try { await deleteOrganisation(id); } catch {}
    }
    confirmBulkDelete = false;
    bulkDeleting = false;
    clearSelection();
    await fetchOrgs();
  }
  // ────────────────────────────────────────────────────────────

  $: filtered = orgs.filter(o =>
    !search || o.name.toLowerCase().includes(search.toLowerCase())
  );
</script>

<div class="space-y-4">
  <PageHeader
    icon={Building2}
    title={$t('pages.organisations.title')}
    count="{orgs.length} {orgs.length === 1 ? $t('pages.organisations.orgSingular') : $t('pages.organisations.orgPlural')}"
  >
    <div slot="actions">
      <button class="info-btn" on:click={() => showInfo = !showInfo} aria-expanded={showInfo}>
        <Info size={14} />
        {$t('pages.organisations.about')}
        <ChevronDown size={13} class="transition-transform {showInfo ? 'rotate-180' : ''}" />
      </button>
      {#if isLoggedIn()}
        <button class="btn" on:click={() => showCreate = true}>
          <Plus size={14} /> {$t('pages.organisations.newOrg')}
        </button>
      {/if}
    </div>
  </PageHeader>

  {#if showInfo}
    <div class="info-panel">
      <!-- eslint-disable-next-line svelte/no-at-html-tags -- trusted static i18n string -->
      <p>{@html $t('pages.organisations.infoIntro')}</p>
      <ul>
        <!-- eslint-disable-next-line svelte/no-at-html-tags -- trusted static i18n string -->
        <li>{@html $t('pages.organisations.infoMembership')}</li>
        <!-- eslint-disable-next-line svelte/no-at-html-tags -- trusted static i18n string -->
        <li>{@html $t('pages.organisations.infoOwnership')}</li>
        <!-- eslint-disable-next-line svelte/no-at-html-tags -- trusted static i18n string -->
        <li>{@html $t('pages.organisations.infoSlug')}</li>
        <!-- eslint-disable-next-line svelte/no-at-html-tags -- trusted static i18n string -->
        <li>{@html $t('pages.organisations.infoScopedSparql')}</li>
      </ul>
      <Link to="/docs" class="info-docs-link">{$t('pages.organisations.viewFullDocs')}</Link>
    </div>
  {/if}

  {#if error}
    <p class="error">{error}</p>
  {/if}

  <!-- Search/Filter bar -->
  <div class="filter-row">
    <div class="filter-input">
      <Search size={14} />
      <input
        id="org-search"
        type="text"
        placeholder={$t('pages.organisations.filterPlaceholder')}
        bind:value={search}
      />
      {#if search}
        <button class="filter-clear" on:click={() => search = ''} aria-label={$t('system.clear')}><X size={12} /></button>
      {/if}
    </div>
    <span class="filter-count">{$t('pages.organisations.filterCount', { values: { shown: filtered.length, total: orgs.length } })}</span>
  </div>

  <div class="card overflow-x-auto">
  <table>
    <thead>
      <tr>
        {#if isLoggedIn()}
        <th class="th-check">
          <input
            type="checkbox"
            bind:this={orgHeaderCheckbox}
            on:change={toggleSelectAll}
            aria-label={$t('pages.organisations.selectAllAria')}
            class="row-check"
          />
        </th>
        {/if}
        <th>{$t('pages.organisations.name')}</th>
        <th class="col-created">{$t('pages.organisations.created')}</th>
        <th class="td-actions"></th>
      </tr>
    </thead>
    <tbody>
      {#each filtered as org}
        {@const isSelected = selected.has(org.id)}
        <tr class:row-selected={isSelected}>
          {#if isLoggedIn()}
          <td class="td-check" on:click|stopPropagation>
            <input
              type="checkbox"
              checked={isSelected}
              on:change={() => toggleSelect(org.id)}
              aria-label={$t('pages.organisations.selectRowAria', { values: { name: org.name } })}
              class="row-check"
            />
          </td>
          {/if}
          <td>
            <span class="org-cell">
              <Avatar kind="organisation" id={org.id} name={org.name} hasImage={!!org.image_key} size={22} />
              <Link to={`/organisations/${org.id}`}>{org.name}</Link>
            </span>
          </td>
          <td class="col-created">{new Date(org.created_at).toLocaleDateString()}</td>
          <td class="td-actions">
            {#if $isAdmin}
              <button class="tbl-btn danger" on:click={() => deleteOrgItem = org} title={$t('system.delete')}><Trash2 size={14} /></button>
            {/if}
          </td>
        </tr>
      {/each}
      {#if filtered.length === 0}
        <tr><td colspan={isLoggedIn() ? 4 : 3}>{orgs.length === 0 ? $t('pages.organisations.noOrgs') : $t('pages.organisations.noMatch')}</td></tr>
      {/if}
    </tbody>
  </table>
  </div>
</div>

<!-- Bulk action bar -->
{#if $isAdmin}
  <BulkActionBar
    count={selected.size}
    total={filtered.length}
    itemLabel={$t('pages.organisations.orgSingular')}
    on:clearSelection={clearSelection}
    on:selectAll={() => { filtered.forEach(o => selected.add(o.id)); selected = selected; }}
  >
    <button class="bulk-action-btn danger" on:click={() => confirmBulkDelete = true} disabled={bulkDeleting}>
      <Trash2 size={13} /> {selected.size === 1 ? $t('pages.organisations.bulkDeleteBtn', { values: { count: selected.size } }) : $t('pages.organisations.bulkDeleteBtnPlural', { values: { count: selected.size } })}
    </button>
  </BulkActionBar>
{/if}

{#if deleteOrgItem !== null}
  <ConfirmModal
    title={$t('pages.organisations.deleteConfirm')}
    message={$t('pages.organisations.deleteMessage')}
    confirmLabel={$t('pages.organisations.deleteConfirmLabel')}
    requirePhrase={deleteOrgItem.name}
    loading={deleteOrgLoading}
    on:confirm={doDeleteOrg}
    on:cancel={() => deleteOrgItem = null}
  />
{/if}

<!-- Create organisation modal -->
{#if showCreate}
  <div class="modal-backdrop" on:click={() => showCreate = false} role="presentation" on:keydown={(e) => e.key === 'Escape' && (showCreate = false)}>
    <div class="modal-box" on:click|stopPropagation on:keydown|stopPropagation role="dialog" aria-modal="true" aria-label={$t('pages.organisations.createOrgAria')} tabindex="-1">
      <div class="modal-header">
        <div class="modal-icon"><Building2 size={20} /></div>
        <div>
          <h3 class="modal-title">{$t('pages.organisations.newOrg')}</h3>
          <p class="modal-subtitle">{$t('pages.organisations.createSubtitle')}</p>
        </div>
        <button class="modal-close-btn" type="button" on:click={() => showCreate = false} aria-label={$t('system.close')}><X size={16} /></button>
      </div>

      <form on:submit|preventDefault={handleCreate} class="modal-body">
        <div class="field">
          <label class="flabel" for="org-name">{$t('pages.organisations.orgNameLabel')} <span class="req">*</span></label>
          <input id="org-name" type="text" class="finput" bind:value={newName} required placeholder={$t('pages.organisations.orgNamePlaceholder')} />
        </div>

        <div class="field">
          <label class="flabel" for="org-slug">{$t('pages.organisations.urlSlugLabel')} <span class="req">*</span></label>
          <div class="slug-row">
            <span class="slug-prefix">/organisations/</span>
            <input
              id="org-slug"
              type="text"
              class="finput slug-input"
              bind:value={newSlug}
              on:input={() => slugManuallyEdited = true}
              placeholder="acme-corp"
              pattern="[a-z0-9-]+"
              title={$t('pages.organisations.slugTitle')}
              required
            />
          </div>
          <span class="fhint">{$t('pages.organisations.slugHint')}</span>
        </div>

        <div class="field">
          <label class="flabel" for="org-desc">{$t('pages.organisations.description')}</label>
          <textarea id="org-desc" class="finput" rows="2" bind:value={newDescription}
            placeholder={$t('pages.organisations.descPlaceholder')}></textarea>
        </div>

        <div class="members-hint">
          <Users size={13} />
          <span>{$t('pages.organisations.membersHint')}</span>
        </div>

        {#if error}
          <div class="modal-error">{error}</div>
        {/if}

        <div class="modal-footer">
          <button type="button" class="btn btn-ghost" on:click={() => showCreate = false}>{$t('system.cancel')}</button>
          <button type="submit" class="btn" disabled={!newName.trim() || !newSlug.trim()}>
            <Plus size={14} /> {$t('pages.organisations.createOrgBtn')}
          </button>
        </div>
      </form>
    </div>
  </div>
{/if}

<!-- Bulk delete confirm -->
{#if confirmBulkDelete}
  <ConfirmModal
    title={selected.size === 1 ? $t('pages.organisations.bulkDeleteTitle', { values: { count: selected.size } }) : $t('pages.organisations.bulkDeleteTitlePlural', { values: { count: selected.size } })}
    message={selected.size === 1 ? $t('pages.organisations.bulkDeleteMessage', { values: { count: selected.size } }) : $t('pages.organisations.bulkDeleteMessagePlural', { values: { count: selected.size } })}
    confirmLabel={selected.size === 1 ? $t('pages.organisations.bulkDeleteConfirmLabel', { values: { count: selected.size } }) : $t('pages.organisations.bulkDeleteConfirmLabelPlural', { values: { count: selected.size } })}
    loading={bulkDeleting}
    on:confirm={bulkDeleteOrgs}
    on:cancel={() => confirmBulkDelete = false}
  />
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
  :global(.info-docs-link) { color: var(--brand-600, #0d7490); font-weight: 500; text-decoration: none; }
  :global(.info-docs-link:hover) { text-decoration: underline; }

  /* Organisation create modal */
  .modal-backdrop { position: fixed; inset: 0; background: rgba(0,0,0,0.4); display: flex; align-items: center; justify-content: center; z-index: 50; padding: 1rem; }
  .modal-box { background: white; border-radius: 1.25rem; width: min(480px, 100%); max-height: 90vh; overflow-y: auto; box-shadow: 0 24px 64px rgba(0,0,0,0.18); display: flex; flex-direction: column; }
  .modal-header { display: flex; align-items: flex-start; gap: 0.875rem; padding: 1.5rem 1.5rem 0; }
  .modal-icon { width: 2.5rem; height: 2.5rem; border-radius: 0.75rem; background: var(--brand-50, #eef2ff); color: var(--brand-600, #4f46e5); display: flex; align-items: center; justify-content: center; flex-shrink: 0; }
  .modal-title { font-size: 1.05rem; font-weight: 700; margin: 0; color: var(--ink-900, #0f172a); }
  .modal-subtitle { font-size: 0.8rem; color: var(--ink-400, #94a3b8); margin: 0.15rem 0 0; }
  .modal-close-btn { margin-left: auto; width: 2rem; height: 2rem; border-radius: 0.5rem; border: none; cursor: pointer; background: transparent; color: var(--ink-400); display: flex; align-items: center; justify-content: center; flex-shrink: 0; transition: all 0.12s; }
  .modal-close-btn:hover { background: var(--bg-soft); color: var(--ink-700); }
  .modal-body { display: flex; flex-direction: column; gap: 1rem; padding: 1.25rem 1.5rem; }
  .modal-footer { display: flex; gap: 0.75rem; justify-content: flex-end; padding-top: 0.25rem; border-top: 1px solid var(--line-soft, #e2e8f0); padding-bottom: 0.25rem; }
  .modal-error { padding: 0.6rem 0.875rem; background: #fef2f2; border: 1px solid #fecaca; border-radius: 0.75rem; font-size: 0.825rem; color: #dc2626; }
  .field { display: flex; flex-direction: column; gap: 0.25rem; }
  .flabel { font-size: 0.825rem; font-weight: 600; color: var(--ink-700); }
  .req { color: #ef4444; }
  .fhint { font-size: 0.75rem; color: var(--ink-400); }
  .finput { width: 100%; padding: 0.5rem 0.75rem; border: 1px solid var(--line-soft); border-radius: 0.75rem; font-size: 0.875rem; box-sizing: border-box; font-family: inherit; outline: none; transition: border-color 0.15s; }
  .finput:focus { border-color: var(--brand-400); box-shadow: 0 0 0 3px rgba(99,102,241,0.1); }
  textarea.finput { resize: vertical; }
  .slug-row { display: flex; align-items: center; border: 1px solid var(--line-soft); border-radius: 0.75rem; overflow: hidden; transition: border-color 0.15s; }
  .slug-row:focus-within { border-color: var(--brand-400); box-shadow: 0 0 0 3px rgba(99,102,241,0.1); }
  .slug-prefix { padding: 0.5rem 0.6rem; background: var(--bg-soft, #f8fafc); font-size: 0.8rem; color: var(--ink-400); border-right: 1px solid var(--line-soft); white-space: nowrap; }
  .slug-input { border: none; border-radius: 0; flex: 1; box-shadow: none; }
  .slug-input:focus { border: none; box-shadow: none; }
  .members-hint { display: flex; align-items: center; gap: 0.5rem; font-size: 0.8rem; color: var(--ink-500); background: var(--bg-soft); border-radius: 0.75rem; padding: 0.5rem 0.75rem; }
  .filter-row {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    flex-wrap: wrap;
  }
  .filter-input {
    display: flex; align-items: center; gap: 0.4rem;
    padding: 0.4rem 0.7rem;
    border: 1px solid var(--line-soft);
    border-radius: 10px;
    background: #fff;
    color: #64748b;
    min-width: 220px;
    flex: 1;
  }
  .filter-input input { flex: 1; border: none; outline: none; background: transparent; font-size: 0.85rem; color: #1e293b; }
  .filter-clear { display: grid; place-items: center; width: 18px; height: 18px; border-radius: 50%; border: none; background: #e2e8f0; color: #64748b; cursor: pointer; }
  .filter-clear:hover { background: #cbd5e1; }
  .filter-count { color: #94a3b8; font-size: 0.8rem; white-space: nowrap; }

  /* Multi-select */
  .th-check, .td-check {
    width: 36px;
    padding: 0.5rem 0.25rem 0.5rem 0.75rem;
  }
  .row-check {
    width: 15px; height: 15px; cursor: pointer;
    accent-color: var(--brand-500, #0d9488);
  }
  .row-selected { background: #f0fdfa !important; }

  /* Actions cell */
  .td-actions {
    white-space: nowrap;
    text-align: right;
    width: 1%;
    padding-left: 0.25rem;
  }

  /* Responsive: hide created date on small screens */
  @media (max-width: 480px) {
    .col-created { display: none; }
  }

  /* ---- Dark mode overrides (scoped rules out-specify global theme.css) ---- */
  :global(:is([data-theme="dark"], .dark)) .info-panel { background: rgba(59,130,246,0.1); border-color: rgba(59,130,246,0.3); color: var(--ink-700); }
  :global(:is([data-theme="dark"], .dark)) .modal-box { background: var(--bg-strong); }
  :global(:is([data-theme="dark"], .dark)) .modal-icon { background: var(--brand-100); }
  :global(:is([data-theme="dark"], .dark)) .modal-error { background: rgba(220,38,38,0.12); border-color: rgba(220,38,38,0.35); color: #fca5a5; }
  :global(:is([data-theme="dark"], .dark)) .req { color: #fca5a5; }
  :global(:is([data-theme="dark"], .dark)) .filter-input { background: var(--bg-soft); color: var(--ink-500); }
  :global(:is([data-theme="dark"], .dark)) .filter-input input { color: var(--ink-900); }
  :global(:is([data-theme="dark"], .dark)) .filter-clear { background: var(--line-strong); color: var(--ink-600); }
  :global(:is([data-theme="dark"], .dark)) .filter-clear:hover { background: var(--ink-400); }
  :global(:is([data-theme="dark"], .dark)) .row-selected { background: var(--bg-accent-soft) !important; }
</style>
