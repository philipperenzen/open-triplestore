<script lang="ts">
  import { onMount } from 'svelte';
  import { listDatasets, createDataset, deleteDataset, listOrganisations, adminListUsers } from '../lib/api.js';
  import { user as userStore, isAdmin, isAuthenticated } from '../lib/stores.js';
  import { Link, navigate } from '../lib/router/index.js';
  import { t } from 'svelte-i18n';
  import { Plus, Trash2, X, User, Building2, Search, Database, Info, ChevronDown, Tag, ShieldCheck } from 'lucide-svelte';
  import ConfirmModal from '../components/ConfirmModal.svelte';
  import BulkActionBar from '../components/BulkActionBar.svelte';
  import Avatar from '../components/Avatar.svelte';
  import PageHeader from '../components/PageHeader.svelte';
  import Select from '../components/Select.svelte';

  let datasets = [];
  let organisations = [];
  let currentUser = null;
  let isAdminValue = false;
  let adminUserMap = {};
  userStore.subscribe(v => currentUser = v);
  isAdmin.subscribe(v => isAdminValue = v);

  let search = '';
  let roleFilter: string = 'all';

  const ROLE_LABELS: Record<string, { label: string; short: string; cls: string }> = {
    instances:  { label: 'Instances',            short: 'Instances',  cls: 'role-instances' },
    model:      { label: 'Model (OWL/RDFS)',      short: 'Model',      cls: 'role-model' },
    vocabulary: { label: 'Vocabulary (SKOS)',    short: 'Vocabulary', cls: 'role-vocabulary' },
    shapes:     { label: 'SHACL Shapes',         short: 'Shapes',     cls: 'role-shapes' },
    entailment: { label: 'Entailment',           short: 'Entailment', cls: 'role-entailment' },
    system:     { label: 'System',               short: 'System',     cls: 'role-system' },
  };
  let showInfo = false;
  let error = '';
  let showCreate = false;
  let newName = '';
  let newDescription = '';
  let newVisibility = 'private';
  let newOwnerType = 'user';
  let newOwnerOrgId = '';
  let newGraphRole = '';

  onMount(async () => {
    await fetchDatasets();
    try {
      organisations = await listOrganisations();
    } catch {}
    if (isAdminValue && $isAuthenticated) {
      try {
        const resp = await adminListUsers({ limit: 100 });
        adminUserMap = Object.fromEntries(
          (resp?.users || []).map(u => [String(u.id), u.username])
        );
      } catch {}
    }
  });

  async function fetchDatasets() {
    try {
      datasets = await listDatasets();
    } catch (e) {
      error = e.message;
    }
  }

  async function handleCreate() {
    error = '';
    try {
      await createDataset({
        name: newName,
        description: newDescription || null,
        visibility: newVisibility,
        owner_type: newOwnerType,
        owner_id: newOwnerType === 'organisation' ? newOwnerOrgId : currentUser?.id,
        conforms_to_ontology: null,
        conforms_to_version: null,
        graph_role: newGraphRole || null,
      });
      showCreate = false;
      newName = '';
      newDescription = '';
      newOwnerType = 'user';
      newOwnerOrgId = '';
      newGraphRole = '';
      await fetchDatasets();
    } catch (e) {
      error = e.message;
    }
  }

  // ── Multi-select ──────────────────────────────────────────────────────────
  let selected: Set<string> = new Set();
  let confirmBulkDelete = false;
  let bulkDeleting = false;

  function toggleSelect(id: string) {
    if (selected.has(id)) { selected.delete(id); } else { selected.add(id); }
    selected = selected;
  }
  function clearSelection() { selected.clear(); selected = selected; }

  $: selectableFiltered = filtered.filter(ds => ds.can_write);
  $: allFilteredSelected = selectableFiltered.length > 0 && selectableFiltered.every(ds => selected.has(String(ds.id)));
  $: someFilteredSelected = !allFilteredSelected && selectableFiltered.some(ds => selected.has(String(ds.id)));

  let dsHeaderCheckbox: HTMLInputElement | null = null;
  $: if (dsHeaderCheckbox) {
    dsHeaderCheckbox.indeterminate = someFilteredSelected;
    dsHeaderCheckbox.checked = allFilteredSelected;
  }

  function toggleSelectAll() {
    const ids = selectableFiltered.map(ds => String(ds.id));
    if (allFilteredSelected) { ids.forEach(id => selected.delete(id)); }
    else { ids.forEach(id => selected.add(id)); }
    selected = selected;
  }

  async function bulkDeleteDatasets() {
    bulkDeleting = true;
    const toDelete = [...selected];
    const errors: string[] = [];
    for (const id of toDelete) {
      try { await deleteDataset(id); } catch { errors.push(id); }
    }
    confirmBulkDelete = false;
    bulkDeleting = false;
    clearSelection();
    if (errors.length) error = $t('pages.datasets.bulkDeleteError');
    await fetchDatasets();
  }
  // ────────────────────────────────────────────────────────────

  let deleteDatasetItem = null;
  let deleteDatasetLoading = false;

  async function doDeleteDataset() {
    deleteDatasetLoading = true;
    try {
      await deleteDataset(deleteDatasetItem.id);
      deleteDatasetItem = null;
      await fetchDatasets();
    } catch (e) {
      error = e.message;
      deleteDatasetItem = null;
    }
    deleteDatasetLoading = false;
  }

  // Distinct roles present in a dataset. Prefer the per-graph `roles` array
  // returned by the API; fall back to the single dataset-level `graph_role`.
  function datasetRoles(d) {
    if (Array.isArray(d.roles) && d.roles.length) return d.roles;
    return d.graph_role ? [d.graph_role] : [];
  }

  $: filtered = datasets.filter(d => {
    if (search && !d.name.toLowerCase().includes(search.toLowerCase())) return false;
    if (roleFilter !== 'all' && !datasetRoles(d).includes(roleFilter)) return false;
    return true;
  });
</script>

<div class="space-y-4">
  <PageHeader
    icon={Database}
    title={$t('pages.datasets.title')}
    count="{datasets.length} {datasets.length === 1 ? $t('pages.datasets.datasetSingular') : $t('pages.datasets.datasetPlural')}"
  >
    <div slot="actions">
      <button class="info-btn" on:click={() => showInfo = !showInfo} aria-expanded={showInfo}>
        <Info size={14} />
        {$t('pages.datasets.about')}
        <ChevronDown size={13} class="transition-transform {showInfo ? 'rotate-180' : ''}" />
      </button>
      {#if $isAuthenticated}
        <button class="btn" on:click={() => showCreate = true}>
          <Plus size={14} /> {$t('pages.datasets.newDataset')}
        </button>
      {/if}
    </div>
  </PageHeader>

  {#if showInfo}
    <div class="info-panel">
      <!-- eslint-disable-next-line svelte/no-at-html-tags -- trusted static i18n string -->
      <p>{@html $t('pages.datasets.infoIntro')}</p>
      <ul>
        <!-- eslint-disable-next-line svelte/no-at-html-tags -- trusted static i18n string -->
        <li>{@html $t('pages.datasets.infoVisibility')}</li>
        <!-- eslint-disable-next-line svelte/no-at-html-tags -- trusted static i18n string -->
        <li>{@html $t('pages.datasets.infoSparql')}</li>
        <!-- eslint-disable-next-line svelte/no-at-html-tags -- trusted static i18n string -->
        <li>{@html $t('pages.datasets.infoShacl')}</li>
        <!-- eslint-disable-next-line svelte/no-at-html-tags -- trusted static i18n string -->
        <li>{@html $t('pages.datasets.infoOwnership')}</li>
      </ul>
      <Link to="/docs" class="info-docs-link">{$t('pages.datasets.viewDocs')}</Link>
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
        id="dataset-search"
        type="text"
        placeholder={$t('pages.datasets.filterPlaceholder')}
        bind:value={search}
      />
      {#if search}
        <button class="filter-clear" on:click={() => search = ''} aria-label={$t('system.clear')}><X size={12} /></button>
      {/if}
    </div>
    <span class="filter-count">{$t('pages.datasets.countOf', { values: { shown: filtered.length, total: datasets.length } })}</span>
  </div>

  <!-- Role filter tabs -->
  <div class="role-tabs">
    <button class="role-tab" class:active={roleFilter === 'all'} on:click={() => roleFilter = 'all'}>{$t('pages.datasets.roleAll')}</button>
    <button class="role-tab role-tab-instances"  class:active={roleFilter === 'instances'}  on:click={() => roleFilter = 'instances'}>{$t('pages.datasets.roleInstances')}</button>
    <button class="role-tab role-tab-model"      class:active={roleFilter === 'model'}      on:click={() => roleFilter = 'model'}>{$t('pages.datasets.roleModel')}</button>
    <button class="role-tab role-tab-vocabulary" class:active={roleFilter === 'vocabulary'} on:click={() => roleFilter = 'vocabulary'}>{$t('pages.datasets.roleVocabulary')}</button>
    <button class="role-tab role-tab-shapes"     class:active={roleFilter === 'shapes'}     on:click={() => roleFilter = 'shapes'}>{$t('pages.datasets.roleShapes')}</button>
    <button class="role-tab role-tab-entailment" class:active={roleFilter === 'entailment'} on:click={() => roleFilter = 'entailment'}>{$t('pages.datasets.roleEntailment')}</button>
  </div>

  <div class="card overflow-x-auto">
  <table>
    <thead>
      <tr>
        {#if $isAuthenticated}
        <th class="th-check">
          <input
            type="checkbox"
            bind:this={dsHeaderCheckbox}
            on:change={toggleSelectAll}
            aria-label={$t('pages.datasets.selectAllWritable')}
            class="row-check"
          />
        </th>
        {/if}
        <th>{$t('pages.datasets.name')}</th>
        <th class="col-role">{$t('pages.datasets.roleColumn')}</th>
        <th class="col-validation">{$t('pages.datasets.validationColumn')}</th>
        <th class="col-visibility">{$t('pages.datasets.visibility')}</th>
        <th class="col-owner">{$t('pages.datasets.owner')}</th>
        <th class="td-actions"></th>
      </tr>
    </thead>
    <tbody>
      {#each filtered as ds}
        {@const isSelected = selected.has(String(ds.id))}
        <tr
          class="ds-row"
          class:row-selected={isSelected}
          on:click={(e) => { if (!(e.target as HTMLElement).closest('button') && !(e.target as HTMLElement).closest('input[type="checkbox"]')) navigate(`/datasets/${ds.id}`); }}
        >
          {#if $isAuthenticated}
          <td class="td-check" on:click|stopPropagation>
            {#if ds.can_write}
              <input
                type="checkbox"
                checked={isSelected}
                on:change={() => toggleSelect(String(ds.id))}
                aria-label={$t('pages.datasets.selectItem', { values: { name: ds.name } })}
                class="row-check"
              />
            {/if}
          </td>
          {/if}
          <td>
            <Link to={`/datasets/${ds.id}`} class="ds-name-link">{ds.name}</Link>
          </td>
          <td class="col-role">
            {#if datasetRoles(ds).filter(r => ROLE_LABELS[r]).length}
              <span class="role-badges">
                {#each datasetRoles(ds).filter(r => ROLE_LABELS[r]) as r}
                  <span class="role-badge {ROLE_LABELS[r].cls}">
                    <Tag size={10} />
                    {$t(`pages.datasets.roleShort.${r}`)}
                  </span>
                {/each}
              </span>
            {:else}
              <span class="text-[var(--ink-300)] text-xs">—</span>
            {/if}
          </td>
          <td class="col-validation">
            {#if ds.shapes_graph_iri}
              <button
                class="shield-btn"
                on:click|stopPropagation={() => navigate(`/validation?dataset=${ds.id}`)}
                title={$t('pages.datasets.validateTooltip')}
                aria-label={$t('pages.datasets.validateItem', { values: { name: ds.name } })}
              >
                <ShieldCheck size={16} />
              </button>
            {:else}
              <span class="text-[var(--ink-300)] text-xs">—</span>
            {/if}
          </td>
          <td class="col-visibility"><span class="vis vis-{ds.visibility}">{ds.visibility}</span></td>
          <td class="col-owner">
            {#if isAdminValue}
              {@const ownerName = ds.owner_type === 'organisation'
                ? (organisations.find(o => o.id === ds.owner_id)?.name ?? String(ds.owner_id))
                : (adminUserMap[String(ds.owner_id)] ?? String(ds.owner_id))}
              <span class="owner-cell">
                <Avatar
                  kind={ds.owner_type === 'organisation' ? 'organisation' : 'user'}
                  id={String(ds.owner_id)}
                  name={ownerName}
                  size={20}
                />
                <span class="vis {ds.owner_type === 'organisation' ? 'vis-org' : 'vis-user'}">{ownerName}</span>
              </span>
            {:else}
              {ds.owner_type === 'organisation' ? $t('pages.datasets.ownerOrganisation') : $t('pages.datasets.ownerPersonal')}
            {/if}
          </td>
          <td class="td-actions">
            {#if ds.can_write}
              <button class="tbl-btn danger" on:click|stopPropagation={() => deleteDatasetItem = ds} title={$t('system.delete')}><Trash2 size={14} /></button>
            {/if}
          </td>
        </tr>
      {/each}
      {#if filtered.length === 0}
        <tr><td colspan={$isAuthenticated ? 8 : 7}>{datasets.length === 0 ? $t('pages.datasets.noDatasets') : $t('pages.datasets.noMatch')}</td></tr>
      {/if}
    </tbody>
  </table>
  </div>
</div>

<!-- Create dataset modal -->
{#if showCreate}
  <div class="ds-modal-backdrop" on:click={() => showCreate = false} role="presentation" on:keydown={(e) => e.key === 'Escape' && (showCreate = false)}>
    <div class="ds-modal-box" on:click|stopPropagation on:keydown|stopPropagation role="dialog" aria-modal="true" aria-label={$t('pages.datasets.createDataset')} tabindex="-1">
      <div class="ds-modal-header">
        <div class="ds-modal-icon"><Database size={20} /></div>
        <div>
          <h3 class="ds-modal-title">{$t('pages.datasets.newDataset')}</h3>
          <p class="ds-modal-subtitle">{$t('pages.datasets.createSubtitle')}</p>
        </div>
        <button class="ds-modal-close" type="button" on:click={() => showCreate = false} aria-label={$t('system.close')}><X size={16} /></button>
      </div>

      <form on:submit|preventDefault={handleCreate} class="ds-modal-body">
        <!-- Name -->
        <div class="ds-field">
          <label class="ds-label" for="ds-name">{$t('pages.datasets.name')} <span class="ds-req">*</span></label>
          <input id="ds-name" type="text" class="ds-input" bind:value={newName} required
            placeholder={$t('pages.datasets.namePlaceholder')} />
        </div>

        <!-- Description -->
        <div class="ds-field">
          <label class="ds-label" for="ds-desc">{$t('pages.datasets.description')}</label>
          <textarea id="ds-desc" class="ds-input" rows="2" bind:value={newDescription}
            placeholder={$t('pages.datasets.descriptionPlaceholder')}></textarea>
        </div>

        <!-- Owner -->
        <div class="ds-field">
          <span class="ds-label">{$t('pages.datasets.owner')}</span>
          <div class="ds-owner-options">
            <label class="ds-owner-opt" class:ds-owner-opt-selected={newOwnerType === 'user'}>
              <input type="radio" bind:group={newOwnerType} value="user" class="sr-only" />
              <User size={14} />
              <span>{currentUser?.username ?? $t('pages.datasets.ownerPersonal')}</span>
            </label>
            {#each organisations as org}
              <label
                class="ds-owner-opt"
                class:ds-owner-opt-selected={newOwnerType === 'organisation' && newOwnerOrgId === org.id}
              >
                <input
                  type="radio"
                  bind:group={newOwnerType}
                  value="organisation"
                  on:change={() => newOwnerOrgId = org.id}
                  class="sr-only"
                />
                <Building2 size={14} /> {org.name}
              </label>
            {/each}
          </div>
        </div>

        <!-- Visibility -->
        <div class="ds-field">
          <label class="ds-label" for="ds-vis">{$t('pages.datasets.visibility')}</label>
          <Select id="ds-vis" class="ds-input" bind:value={newVisibility} options={[
            { value: 'private', label: `${$t('pages.datasets.private')} — ${$t('pages.datasets.visPrivateHint')}` },
            { value: 'members', label: `${$t('pages.datasets.members')} — ${$t('pages.datasets.visMembersHint')}` },
            { value: 'public', label: `${$t('pages.datasets.public')} — ${$t('pages.datasets.visPublicHint')}` },
          ]} />
        </div>

        <!-- Graph role -->
        <div class="ds-field">
          <label class="ds-label" for="ds-role">{$t('pages.datasets.graphRole')} <span class="ds-hint-inline">{$t('pages.datasets.optional')}</span></label>
          <Select id="ds-role" class="ds-input" bind:value={newGraphRole} options={[
            { value: '', label: $t('pages.datasets.roleUnclassified') },
            { value: 'instances', label: $t('pages.datasets.roleInstancesOption') },
            { value: 'model', label: $t('pages.datasets.roleModelOption') },
            { value: 'shapes', label: $t('pages.datasets.roleShapesOption') },
            { value: 'entailment', label: $t('pages.datasets.roleEntailmentOption') },
            { value: 'system', label: $t('pages.datasets.roleSystemOption') },
          ]} />
          <span class="ds-hint">{$t('pages.datasets.graphRoleHint')}</span>
        </div>

        {#if error}
          <div class="ds-error">{error}</div>
        {/if}

        <div class="ds-modal-footer">
          <button type="button" class="btn btn-sm btn-ghost" on:click={() => showCreate = false}>{$t('system.cancel')}</button>
          <button type="submit" class="btn btn-sm" disabled={!newName.trim()}>
            <Plus size={14} /> {$t('pages.datasets.createDataset')}
          </button>
        </div>
      </form>
    </div>
  </div>
{/if}

{#if deleteDatasetItem !== null}
  <ConfirmModal
    title={$t('pages.datasets.deleteConfirm')}
    message={$t('pages.datasets.deleteMessage')}
    confirmLabel={$t('pages.datasets.deleteDataset')}
    requirePhrase={deleteDatasetItem.name}
    loading={deleteDatasetLoading}
    on:confirm={doDeleteDataset}
    on:cancel={() => deleteDatasetItem = null}
  />
{/if}

<!-- Bulk delete confirm -->
{#if confirmBulkDelete}
  <ConfirmModal
    title={$t('pages.datasets.bulkDeleteTitle', { values: { count: selected.size, noun: selected.size === 1 ? $t('pages.datasets.datasetSingular') : $t('pages.datasets.datasetPlural') } })}
    message={$t('pages.datasets.bulkDeleteMessage', { values: { count: selected.size, noun: selected.size === 1 ? $t('pages.datasets.datasetSingular') : $t('pages.datasets.datasetPlural') } })}
    confirmLabel={$t('pages.datasets.bulkDeleteConfirm', { values: { count: selected.size, noun: selected.size === 1 ? $t('pages.datasets.datasetSingular') : $t('pages.datasets.datasetPlural') } })}
    loading={bulkDeleting}
    on:confirm={bulkDeleteDatasets}
    on:cancel={() => confirmBulkDelete = false}
  />
{/if}

<!-- Bulk action bar -->
{#if $isAuthenticated}
  <BulkActionBar
    count={selected.size}
    total={selectableFiltered.length}
    itemLabel={$t('pages.datasets.datasetSingular')}
    on:clearSelection={clearSelection}
    on:selectAll={() => { selectableFiltered.forEach(ds => selected.add(String(ds.id))); selected = selected; }}
  >
    <button class="bulk-action-btn danger" on:click={() => confirmBulkDelete = true} disabled={bulkDeleting}>
      <Trash2 size={13} /> {$t('pages.datasets.bulkDeleteConfirm', { values: { count: selected.size, noun: selected.size === 1 ? $t('pages.datasets.datasetSingular') : $t('pages.datasets.datasetPlural') } })}
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
  :global(.info-docs-link) { color: var(--brand-600, #0d7490); font-weight: 500; text-decoration: none; }
  :global(.info-docs-link:hover) { text-decoration: underline; }

  .create-form {
    background: #f0f4ff;
    padding: 1rem;
    border-radius: 6px;
    margin-bottom: 1rem;
  }
  .vis {
    padding: 0.15rem 0.5rem;
    border-radius: 3px;
    font-size: 0.8rem;
    font-weight: 500;
  }
  .vis-public { background: #d4edda; color: #155724; }
  .vis-members { background: #fff3cd; color: #856404; }
  .vis-private { background: #f8d7da; color: #721c24; }
  .vis-org { background: #ddd6fe; color: #5b21b6; }
  .vis-user { background: #e0e7ff; color: #3730a3; }
  .owner-options {
    display: flex;
    flex-wrap: wrap;
    gap: 0.5rem;
  }
  .owner-opt {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    padding: 0.35rem 0.75rem;
    border: 1px solid var(--line-soft, #d1d5db);
    border-radius: 6px;
    cursor: pointer;
    font-size: 0.85rem;
    background: white;
    transition: all 0.15s;
  }
  .owner-opt.selected {
    border-color: var(--brand-500, #0d9488);
    background: var(--bg-accent-soft, #f0fdf4);
    font-weight: 600;
  }
  .ds-row {
    cursor: pointer;
    transition: background 0.12s ease;
  }
  .ds-row:hover { background: var(--bg-accent-soft, #f0fdf4); }
  :global(.ds-name-link) {
    font-weight: 600;
    color: var(--brand-600, #0d9488);
    text-decoration: none;
  }
  :global(.ds-name-link:hover) { text-decoration: underline; }
  :global(.onto-link) { color: var(--brand-600, #0d7490); text-decoration: none; font-size: 0.82rem; display: inline-flex; align-items: center; gap: 0.3rem; }
  :global(.onto-link:hover) { text-decoration: underline; }
  .onto-ver { font-size: 0.75rem; color: var(--ink-400, #9ca3af); }
  .hint { font-size: 0.78rem; color: var(--ink-400, #9ca3af); margin-top: 0.2rem; }

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

  /* Role filter tabs */
  .role-tabs {
    display: flex;
    gap: 0.35rem;
    flex-wrap: wrap;
  }
  .role-tab {
    padding: 0.25rem 0.65rem;
    border: 1px solid var(--line-soft, #d1d5db);
    border-radius: 20px;
    background: white;
    cursor: pointer;
    font-size: 0.78rem;
    color: var(--ink-500);
    transition: all 0.12s;
  }
  .role-tab:hover { border-color: var(--brand-400); color: var(--brand-600); }
  .role-tab.active { background: var(--brand-100, #e0f2fe); border-color: var(--brand-500); color: var(--brand-700, #0369a1); font-weight: 600; }
  .role-tab-instances.active  { background: #dbeafe; border-color: #3b82f6; color: #1d4ed8; }
  .role-tab-model.active      { background: #dcfce7; border-color: #22c55e; color: #15803d; }
  .role-tab-vocabulary.active { background: #fce7f3; border-color: #ec4899; color: #9d174d; }
  .role-tab-shapes.active     { background: #fef9c3; border-color: #eab308; color: #854d0e; }
  .role-tab-entailment.active { background: #ede9fe; border-color: #8b5cf6; color: #5b21b6; }

  /* Role badges */
  .role-badges {
    display: inline-flex;
    flex-wrap: wrap;
    gap: 0.25rem;
  }
  .role-badge {
    display: inline-flex;
    align-items: center;
    gap: 0.25rem;
    padding: 0.15rem 0.45rem;
    border-radius: 4px;
    font-size: 0.75rem;
    font-weight: 600;
  }
  .role-instances  { background: #dbeafe; color: #1d4ed8; }
  .role-model      { background: #dcfce7; color: #15803d; }
  .role-vocabulary { background: #fce7f3; color: #9d174d; }
  .role-shapes     { background: #fef9c3; color: #854d0e; }
  .role-entailment { background: #ede9fe; color: #5b21b6; }
  .role-system     { background: #f1f5f9; color: #475569; }

  /* Multi-select */
  .th-check, .td-check {
    width: 36px;
    padding: 0.5rem 0.25rem 0.5rem 0.75rem;
  }
  .row-check {
    width: 15px;
    height: 15px;
    cursor: pointer;
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

  /* Validation column */
  .col-validation { width: 1%; white-space: nowrap; }
  .shield-btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 1.75rem;
    height: 1.75rem;
    border: none;
    border-radius: 0.5rem;
    cursor: pointer;
    background: var(--brand-100);
    color: var(--brand-700);
    transition: all 0.12s;
  }
  .shield-btn:hover { background: var(--brand-200); color: var(--brand-700); }

  /* Responsive: hide less important columns on narrow screens */
  @media (max-width: 700px) {
    .col-owner { display: none; }
  }
  @media (max-width: 480px) {
    .col-role, .col-visibility { display: none; }
  }

  /* Dataset create modal */
  .ds-modal-backdrop { position: fixed; inset: 0; background: rgba(0,0,0,0.4); display: flex; align-items: center; justify-content: center; z-index: 50; padding: 1rem; }
  .ds-modal-box { background: white; border-radius: 1.25rem; width: min(520px, 100%); max-height: 90vh; overflow-y: auto; box-shadow: 0 24px 64px rgba(0,0,0,0.18); display: flex; flex-direction: column; }
  .ds-modal-header { display: flex; align-items: flex-start; gap: 0.875rem; padding: 1.5rem 1.5rem 0; }
  .ds-modal-icon { width: 2.5rem; height: 2.5rem; border-radius: 0.75rem; background: var(--brand-50, #eef2ff); color: var(--brand-600, #4f46e5); display: flex; align-items: center; justify-content: center; flex-shrink: 0; }
  .ds-modal-title { font-size: 1.05rem; font-weight: 700; margin: 0; color: var(--ink-900, #0f172a); }
  .ds-modal-subtitle { font-size: 0.8rem; color: var(--ink-400, #94a3b8); margin: 0.15rem 0 0; }
  .ds-modal-close { margin-left: auto; width: 2rem; height: 2rem; border-radius: 0.5rem; border: none; cursor: pointer; background: transparent; color: var(--ink-400); display: flex; align-items: center; justify-content: center; flex-shrink: 0; transition: all 0.12s; }
  .ds-modal-close:hover { background: var(--bg-soft); color: var(--ink-700); }
  .ds-modal-body { display: flex; flex-direction: column; gap: 1.1rem; padding: 1.25rem 1.5rem; }
  .ds-modal-footer { display: flex; gap: 0.75rem; justify-content: flex-end; padding-top: 0.25rem; border-top: 1px solid var(--line-soft, #e2e8f0); padding-bottom: 0.25rem; }
  .ds-field { display: flex; flex-direction: column; gap: 0.25rem; }
  .ds-label { font-size: 0.825rem; font-weight: 600; color: var(--ink-700); }
  .ds-req { color: #ef4444; }
  .ds-hint { font-size: 0.75rem; color: var(--ink-400); }
  .ds-hint-inline { font-size: 0.75rem; font-weight: 400; color: var(--ink-400); }
  .ds-input { width: 100%; padding: 0.5rem 0.75rem; border: 1px solid var(--line-soft); border-radius: 0.75rem; font-size: 0.875rem; box-sizing: border-box; font-family: inherit; outline: none; transition: border-color 0.15s; }
  .ds-input:focus { border-color: var(--brand-400); box-shadow: 0 0 0 3px rgba(99,102,241,0.1); }
  textarea.ds-input { resize: vertical; }
  .ds-owner-options { display: flex; gap: 0.5rem; flex-wrap: wrap; }
  .ds-owner-opt { display: flex; align-items: center; gap: 0.4rem; padding: 0.45rem 0.75rem; border: 1px solid var(--line-soft); border-radius: 0.75rem; cursor: pointer; font-size: 0.85rem; font-weight: 500; color: var(--ink-600); transition: all 0.12s; user-select: none; }
  .ds-owner-opt:hover { border-color: var(--brand-300); color: var(--brand-600); }
  .ds-owner-opt-selected { border-color: var(--brand-400); background: var(--brand-50, #eef2ff); color: var(--brand-700); }
  .ds-error { padding: 0.6rem 0.875rem; background: #fef2f2; border: 1px solid #fecaca; border-radius: 0.75rem; font-size: 0.825rem; color: #dc2626; }

  .sr-only { position: absolute; width: 1px; height: 1px; overflow: hidden; clip: rect(0,0,0,0); }

  /* ─── Dark theme overrides ───────────────────────────────────────────────── */
  /* Re-map the hardcoded light surfaces, role colour-sets and the --brand-50
     fills (which the dark theme doesn't redefine) to dark-aware values. */
  :global(:is([data-theme="dark"], .dark)) .info-panel { background: rgba(59,130,246,0.1); border-color: rgba(59,130,246,0.3); }
  :global(:is([data-theme="dark"], .dark)) .create-form { background: rgba(99,102,241,0.1); }
  :global(:is([data-theme="dark"], .dark)) .owner-opt,
  :global(:is([data-theme="dark"], .dark)) .role-tab { background: var(--bg-strong); }

  :global(:is([data-theme="dark"], .dark)) .vis-public { background: rgba(16,185,129,0.18); color: #6ee7b7; }
  :global(:is([data-theme="dark"], .dark)) .vis-members { background: rgba(245,158,11,0.18); color: #fcd34d; }
  :global(:is([data-theme="dark"], .dark)) .vis-private { background: rgba(220,38,38,0.18); color: #fca5a5; }
  :global(:is([data-theme="dark"], .dark)) .vis-org { background: rgba(124,58,237,0.22); color: #c4b5fd; }
  :global(:is([data-theme="dark"], .dark)) .vis-user { background: rgba(99,102,241,0.2); color: #c7d2fe; }

  :global(:is([data-theme="dark"], .dark)) .filter-input { background: var(--bg-strong); color: var(--ink-600); }
  :global(:is([data-theme="dark"], .dark)) .filter-input input { color: var(--ink-900); }
  :global(:is([data-theme="dark"], .dark)) .filter-clear { background: rgba(255,255,255,0.1); color: var(--ink-600); }
  :global(:is([data-theme="dark"], .dark)) .filter-clear:hover { background: rgba(255,255,255,0.16); }

  :global(:is([data-theme="dark"], .dark)) .role-tab-instances.active,
  :global(:is([data-theme="dark"], .dark)) .role-instances { background: rgba(59,130,246,0.2); color: #93c5fd; }
  :global(:is([data-theme="dark"], .dark)) .role-tab-instances.active { border-color: rgba(59,130,246,0.6); }
  :global(:is([data-theme="dark"], .dark)) .role-tab-model.active,
  :global(:is([data-theme="dark"], .dark)) .role-model { background: rgba(34,197,94,0.2); color: #86efac; }
  :global(:is([data-theme="dark"], .dark)) .role-tab-model.active { border-color: rgba(34,197,94,0.6); }
  :global(:is([data-theme="dark"], .dark)) .role-tab-vocabulary.active,
  :global(:is([data-theme="dark"], .dark)) .role-vocabulary { background: rgba(236,72,153,0.2); color: #f9a8d4; }
  :global(:is([data-theme="dark"], .dark)) .role-tab-vocabulary.active { border-color: rgba(236,72,153,0.6); }
  :global(:is([data-theme="dark"], .dark)) .role-tab-shapes.active,
  :global(:is([data-theme="dark"], .dark)) .role-shapes { background: rgba(234,179,8,0.2); color: #fde047; }
  :global(:is([data-theme="dark"], .dark)) .role-tab-shapes.active { border-color: rgba(234,179,8,0.6); }
  :global(:is([data-theme="dark"], .dark)) .role-tab-entailment.active,
  :global(:is([data-theme="dark"], .dark)) .role-entailment { background: rgba(139,92,246,0.2); color: #c4b5fd; }
  :global(:is([data-theme="dark"], .dark)) .role-tab-entailment.active { border-color: rgba(139,92,246,0.6); }
  :global(:is([data-theme="dark"], .dark)) .role-system { background: rgba(255,255,255,0.06); color: var(--ink-600); }

  :global(:is([data-theme="dark"], .dark)) .row-selected { background: rgba(126,214,208,0.1) !important; }
  :global(:is([data-theme="dark"], .dark)) .ds-modal-box { background: var(--bg-strong); }
  :global(:is([data-theme="dark"], .dark)) .ds-modal-icon,
  :global(:is([data-theme="dark"], .dark)) .ds-owner-opt-selected { background: var(--brand-100); color: var(--brand-700); }
  :global(:is([data-theme="dark"], .dark)) .ds-req { color: #f87171; }
  :global(:is([data-theme="dark"], .dark)) .ds-error { background: rgba(220,38,38,0.12); border-color: rgba(220,38,38,0.35); color: #fca5a5; }
</style>
