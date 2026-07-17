<script>
  import { onMount } from 'svelte';
  import { autofocus } from '../lib/actions/autofocus.js';
  import { listShapeGraphs, createShapeGraph, deleteShapeGraph, cloneShapeGraph, listOrganisations } from '../lib/api.js';
  import { FileCode, Plus, Search, X, Copy, Trash2, Lock, Users, Globe, Sparkles, Database, Building2, User as UserIcon, Layers } from 'lucide-svelte';
  import { Link, navigate } from '../lib/router/index.js';
  import ShaclStudioNav from '../components/ShaclStudioNav.svelte';
  import ShapesCatalog from '../components/ShapesCatalog.svelte';
  import Select from '../components/Select.svelte';
  import { isAuthenticated, authInitialized, user } from '../lib/stores.js';
  import { toastError, toastSuccess } from '../lib/toast.ts';
  import { t } from 'svelte-i18n';

  let sets = [];
  let orgs = [];
  let loading = false;
  let error = '';
  // Two views under the Shapes tab: the shape-graph Library, and the flat
  // catalog of every shape across all graphs (to pick from / compose).
  let view = 'graphs';

  // Toolbar state — mirrors the shared search/filter pattern: substring search,
  // toggle-able facets, AND across facet groups, OR within a group.
  let search = '';
  let sourceFilter = new Set();   // values: manual | derived | imported | ai
  let visibilityFilter = new Set();
  let ownerFilter = new Set();    // owner_id strings

  // Create-modal state.
  let creating = false;
  let newName = '';
  let newDescription = '';
  let newVisibility = 'private';
  let newOwnerType = 'user';
  let newOwnerId = '';

  let _guardChecked = false;
  $: if ($authInitialized && !_guardChecked) {
    _guardChecked = true;
    if (!$isAuthenticated) navigate('/login');
  }

  onMount(async () => {
    loading = true;
    try {
      sets = await listShapeGraphs();
      try { orgs = await listOrganisations(); } catch {}
    } catch (e) {
      error = e.message;
    } finally {
      loading = false;
    }
  });

  // Facet counts are computed off the full list so toggling a facet doesn't
  // change *other* facets' counts (the standard "scope-aware" behaviour).
  $: sources = countBy(sets, (s) => s.source);
  $: visibilities = countBy(sets, (s) => s.visibility);
  $: ownerOptions = uniqueOwners(sets);

  $: filtered = sets.filter((s) => {
    if (sourceFilter.size && !sourceFilter.has(s.source)) return false;
    if (visibilityFilter.size && !visibilityFilter.has(s.visibility)) return false;
    if (ownerFilter.size && !ownerFilter.has(s.owner_id)) return false;
    if (search.trim()) {
      const q = search.toLowerCase();
      const hay = [s.name, s.description || '', ...(s.tags || []), ...(s.target_classes || [])].join(' ').toLowerCase();
      if (!hay.includes(q)) return false;
    }
    return true;
  });

  // Standards are seeded with 'standard'/'builtin' tags (their record source
  // is 'imported'); tags are the display signal that separates them from the
  // user's own shape graphs.
  function isStandard(s) {
    return (s.tags || []).some((tag) => tag === 'standard' || tag === 'builtin');
  }
  // Relevance-first default: the user's own/imported shape graphs (newest
  // first), then the bundled standards alphabetically.
  $: ownSets = filtered.filter((s) => !isStandard(s)).sort((a, b) => String(b.updated_at || '').localeCompare(String(a.updated_at || '')));
  $: standardSets = filtered.filter(isStandard).sort((a, b) => a.name.localeCompare(b.name));
  $: groups = [
    { key: 'own', label: $t('pages.shapeLibrary.groupYours'), items: ownSets },
    { key: 'std', label: $t('pages.shapeLibrary.groupStandards'), items: standardSets },
  ].filter((g) => g.items.length);

  function countBy(arr, key) {
    const m = new Map();
    for (const x of arr) {
      const k = key(x);
      m.set(k, (m.get(k) || 0) + 1);
    }
    return [...m.entries()].map(([value, count]) => ({ value, count })).sort((a, b) => b.count - a.count);
  }
  function uniqueOwners(arr) {
    const m = new Map();
    for (const s of arr) {
      const id = String(s.owner_id);
      const existing = m.get(id);
      if (!existing) m.set(id, { id, type: s.owner_type, count: 1 });
      else existing.count += 1;
    }
    return [...m.values()];
  }
  function ownerLabel(o) {
    if (o.type === 'organisation') {
      const org = orgs.find((x) => String(x.id) === o.id);
      return org ? org.name : o.id;
    }
    if (String(o.id) === String($user?.id)) return $t('pages.shapeLibrary.ownerYou');
    return o.id;
  }

  function toggle(set, value) { if (set.has(value)) set.delete(value); else set.add(value); return new Set(set); }

  function shortIRI(iri) {
    if (!iri) return '';
    const m = String(iri).match(/[^#/]+$/);
    return m ? m[0] : iri;
  }

  function relativeTime(iso) {
    if (!iso) return '';
    const sec = Math.round((Date.now() - new Date(iso).getTime()) / 1000);
    if (sec < 60) return $t('pages.shapeLibrary.timeJustNow');
    const min = Math.round(sec / 60); if (min < 60) return $t('pages.shapeLibrary.timeMinutesAgo', { values: { n: min } });
    const hr = Math.round(min / 60); if (hr < 24) return $t('pages.shapeLibrary.timeHoursAgo', { values: { n: hr } });
    const day = Math.round(hr / 24); if (day < 30) return $t('pages.shapeLibrary.timeDaysAgo', { values: { n: day } });
    return $t('pages.shapeLibrary.timeMonthsAgo', { values: { n: Math.round(day / 30) } });
  }

  async function submitCreate() {
    if (!newName.trim()) return;
    try {
      const body = {
        name: newName.trim(),
        description: newDescription.trim() || undefined,
        visibility: newVisibility,
        owner_type: newOwnerType === 'organisation' ? 'organisation' : 'user',
        owner_id: newOwnerType === 'organisation' ? newOwnerId : undefined,
      };
      const set = await createShapeGraph(body);
      toastSuccess($t('pages.shapeLibrary.toastCreated', { values: { name: set.name } }));
      creating = false;
      newName = ''; newDescription = ''; newVisibility = 'private'; newOwnerType = 'user'; newOwnerId = '';
      navigate(`/shacl/shapes/${set.id}`);
    } catch (e) {
      toastError(e.message);
    }
  }

  async function doClone(set) {
    try {
      const clone = await cloneShapeGraph(set.id, {});
      toastSuccess($t('pages.shapeLibrary.toastCloned', { values: { name: clone.name } }));
      sets = await listShapeGraphs();
    } catch (e) { toastError(e.message); }
  }

  async function doDelete(set) {
    if (!confirm($t('pages.shapeLibrary.confirmDelete', { values: { name: set.name } }))) return;
    try {
      await deleteShapeGraph(set.id);
      sets = sets.filter((s) => s.id !== set.id);
    } catch (e) { toastError(e.message); }
  }

  function clearFilters() {
    search = '';
    sourceFilter = new Set();
    visibilityFilter = new Set();
    ownerFilter = new Set();
  }
  $: anyFilter = !!(search.trim() || sourceFilter.size || visibilityFilter.size || ownerFilter.size);
</script>

<div class="library-page">
  <ShaclStudioNav />

  <div class="view-toggle">
    <button class="vt" class:active={view === 'graphs'} on:click={() => (view = 'graphs')}><FileCode size={13} /> {$t('pages.shapeLibrary.viewShapeGraphs')}</button>
    <button class="vt" class:active={view === 'shapes'} on:click={() => (view = 'shapes')}><Layers size={13} /> {$t('pages.shapeLibrary.viewShapes')}</button>
  </div>

  {#if view === 'shapes'}
    <ShapesCatalog />
  {:else}
  <div class="card toolbar">
    <div class="search-wrap">
      <Search size={14} />
      <input class="search-input" placeholder={$t('pages.shapeLibrary.searchPlaceholder')} bind:value={search} />
      {#if search}<button class="icon-btn" on:click={() => (search = '')} title={$t('system.clear')}><X size={13} /></button>{/if}
    </div>
    <div class="toolbar-actions">
      {#if anyFilter}<button class="btn btn-sm btn-ghost" on:click={clearFilters}><X size={12} /> {$t('pages.shapeLibrary.clearFilters')}</button>{/if}
      <button class="btn" on:click={() => (creating = true)}><Plus size={14} /> {$t('pages.shapeLibrary.newShapeGraph')}</button>
    </div>
  </div>

  <div class="layout">
    <aside class="card facets">
      <div class="facet-group">
        <h4>{$t('pages.shapeLibrary.facetSource')}</h4>
        {#each sources as f}
          <button class="facet" class:active={sourceFilter.has(f.value)} on:click={() => (sourceFilter = toggle(sourceFilter, f.value))}>
            <span class="facet-label">{f.value}</span>
            <span class="facet-count">{f.count}</span>
          </button>
        {:else}
          <span class="facet-empty">{$t('pages.shapeLibrary.noShapeGraphsYet')}</span>
        {/each}
      </div>
      <div class="facet-group">
        <h4>{$t('pages.shapeLibrary.facetVisibility')}</h4>
        {#each visibilities as f}
          <button class="facet" class:active={visibilityFilter.has(f.value)} on:click={() => (visibilityFilter = toggle(visibilityFilter, f.value))}>
            <span class="facet-label">
              {#if f.value === 'public'}<Globe size={11} />{:else if f.value === 'members'}<Users size={11} />{:else}<Lock size={11} />{/if}
              {f.value}
            </span>
            <span class="facet-count">{f.count}</span>
          </button>
        {/each}
      </div>
      <div class="facet-group">
        <h4>{$t('pages.shapeLibrary.facetOwner')}</h4>
        {#each ownerOptions as o}
          <button class="facet" class:active={ownerFilter.has(o.id)} on:click={() => (ownerFilter = toggle(ownerFilter, o.id))}>
            <span class="facet-label">
              {#if o.type === 'organisation'}<Building2 size={11} />{:else}<UserIcon size={11} />{/if}
              {ownerLabel(o)}
            </span>
            <span class="facet-count">{o.count}</span>
          </button>
        {/each}
      </div>
    </aside>

    <section class="results">
      {#if error}<div class="error">{error}</div>{/if}
      {#if loading}
        <div class="placeholder"><p>{$t('pages.shapeLibrary.loadingShapeGraphs')}</p></div>
      {:else if filtered.length === 0}
        <div class="placeholder">
          <FileCode size={42} strokeWidth={1.2} />
          {#if sets.length === 0}
            <h3>{$t('pages.shapeLibrary.noShapeGraphsYet')}</h3>
            <p>{$t('pages.shapeLibrary.emptyHelp')}</p>
            <button class="btn" on:click={() => (creating = true)}><Plus size={14} /> {$t('pages.shapeLibrary.newShapeGraph')}</button>
          {:else}
            <h3>{$t('pages.shapeLibrary.noShapeGraphsMatch')}</h3>
            <button class="btn btn-sm btn-ghost" on:click={clearFilters}><X size={12} /> {$t('pages.shapeLibrary.clearFilters')}</button>
          {/if}
        </div>
      {:else}
        {#each groups as g (g.key)}
        {#if groups.length > 1}
          <h3 class="group-label">{g.label} <span class="group-count">{g.items.length}</span></h3>
        {/if}
        <ul class="set-grid">
          {#each g.items as set (set.id)}
            <li class="set-card">
              <Link to={`/shacl/shapes/${set.id}`} class="set-card-main">
                <div class="set-head">
                  <FileCode size={14} class="set-icon" />
                  <span class="set-name">{set.name}</span>
                  {#if set.visibility === 'public'}
                    <span class="chip chip-vis"><Globe size={10} /> {$t('pages.shapeLibrary.visibilityPublic')}</span>
                  {:else if set.visibility === 'members'}
                    <span class="chip chip-vis"><Users size={10} /> {$t('pages.shapeLibrary.visibilityMembers')}</span>
                  {:else}
                    <span class="chip chip-vis"><Lock size={10} /> {$t('pages.shapeLibrary.visibilityPrivate')}</span>
                  {/if}
                  {#if isStandard(set)}
                    <span class="chip chip-source chip-source-standard" title={$t('pages.shapeLibrary.sourceStandardTitle')}>{$t('pages.shapeLibrary.sourceStandard')}</span>
                  {:else if set.source !== 'manual'}
                    <span class="chip chip-source chip-source-{set.source}"
                      title={set.source === 'imported' ? $t('pages.shapeLibrary.sourceImportedTitle') : undefined}>
                      {#if set.source === 'ai'}<Sparkles size={10} />{/if}
                      {set.source}
                    </span>
                  {/if}
                  {#if set.status}<span class="chip chip-status chip-status-{set.status}">{set.status}</span>{/if}
                </div>
                {#if set.description}<p class="set-desc">{set.description}</p>{/if}
                <div class="set-meta">
                  <span class="set-stat"><strong>{set.shape_count}</strong> {set.shape_count === 1 ? $t('pages.shapeLibrary.shapeSingular') : $t('pages.shapeLibrary.shapePlural')}</span>
                  <span class="set-stat">v{set.version}</span>
                  <span class="set-stat dim">{$t('pages.shapeLibrary.updatedPrefix')} {relativeTime(set.updated_at)}</span>
                </div>
                {#if (set.target_classes || []).length}
                  <div class="targets">
                    {#each set.target_classes.slice(0, 6) as tc}<span class="chip chip-target"><Database size={10} /> {shortIRI(tc)}</span>{/each}
                    {#if set.target_classes.length > 6}<span class="chip chip-more">+{set.target_classes.length - 6}</span>{/if}
                  </div>
                {/if}
              </Link>
              <div class="set-actions">
                <button class="icon-btn" on:click={() => doClone(set)} title={$t('pages.shapeLibrary.cloneTitle')}><Copy size={13} /></button>
                <button class="icon-btn icon-danger" on:click={() => doDelete(set)} title={$t('system.delete')}><Trash2 size={13} /></button>
              </div>
            </li>
          {/each}
        </ul>
        {/each}
      {/if}
    </section>
  </div>
  {/if}

  {#if creating}
    <div class="modal-backdrop" on:click={() => (creating = false)} role="presentation">
      <div class="modal" on:click|stopPropagation on:keydown|stopPropagation role="dialog" aria-modal="true" tabindex="-1">
        <header class="modal-head">
          <h3>{$t('pages.shapeLibrary.newShapeGraph')}</h3>
          <button class="icon-btn" on:click={() => (creating = false)}><X size={14} /></button>
        </header>
        <form class="modal-body" on:submit|preventDefault={submitCreate}>
          <label>
            <span>{$t('pages.shapeLibrary.nameLabel')}</span>
            <input bind:value={newName} required placeholder={$t('pages.shapeLibrary.namePlaceholder')} use:autofocus />
          </label>
          <label>
            <span>{$t('pages.shapeLibrary.descriptionLabel')}</span>
            <textarea bind:value={newDescription} rows="2" placeholder={$t('pages.shapeLibrary.descriptionPlaceholder')}></textarea>
          </label>
          <label>
            <span>{$t('pages.shapeLibrary.visibilityLabel')}</span>
            <Select bind:value={newVisibility} options={[
              { value: 'private', label: $t('pages.shapeLibrary.visOptPrivate') },
              { value: 'members', label: $t('pages.shapeLibrary.visOptMembers') },
              { value: 'public', label: $t('pages.shapeLibrary.visOptPublic') },
            ]} />
          </label>
          <label>
            <span>{$t('pages.shapeLibrary.ownerLabel')}</span>
            <Select bind:value={newOwnerType} options={[
              { value: 'user', label: $t('pages.shapeLibrary.ownerYou') },
              ...(orgs.length ? [{ value: 'organisation', label: $t('pages.shapeLibrary.ownerOrganisation') }] : []),
            ]} />
          </label>
          {#if newOwnerType === 'organisation'}
            <label>
              <span>{$t('pages.shapeLibrary.organisationLabel')}</span>
              <Select bind:value={newOwnerId} options={[
                { value: '', label: $t('pages.shapeLibrary.pickOrganisation') },
                ...orgs.map((org) => ({ value: org.id, label: org.name })),
              ]} />
            </label>
          {/if}
          <div class="modal-actions">
            <button type="button" class="btn btn-ghost" on:click={() => (creating = false)}>{$t('system.cancel')}</button>
            <button type="submit" class="btn"><Plus size={13} /> {$t('system.create')}</button>
          </div>
        </form>
      </div>
    </div>
  {/if}
</div>

<style>
  .library-page { display: flex; flex-direction: column; }
  .view-toggle { display: inline-flex; gap: 0.2rem; padding: 0.25rem; background: var(--surface, #fff); border: 1px solid var(--line-soft); border-radius: 10px; margin-bottom: 0.85rem; align-self: flex-start; }
  .vt { display: inline-flex; align-items: center; gap: 0.35rem; padding: 0.35rem 0.7rem; border: none; border-radius: 7px; background: transparent; color: #64748b; font-weight: 600; font-size: 0.82rem; cursor: pointer; }
  .vt:hover { background: #f1f5f9; color: #334155; }
  .vt.active { background: #ecfeff; color: #0e7490; }
  .error { color: #dc2626; background: #fef2f2; border: 1px solid #fecaca; padding: 0.6rem 0.8rem; border-radius: 10px; font-size: 0.85rem; margin-bottom: 0.75rem; }
  .toolbar { display: flex; align-items: center; gap: 0.75rem; padding: 0.55rem 0.85rem !important; margin-bottom: 1rem; }
  .search-wrap { flex: 1; display: flex; align-items: center; gap: 0.5rem; padding: 0 0.4rem; border: 1px solid var(--line-soft); border-radius: 10px; background: #fff; color: #94a3b8; }
  .search-input { flex: 1; border: none; outline: none; background: transparent; font-size: 0.9rem; color: #1e293b; padding: 0.5rem 0.2rem; }
  .toolbar-actions { display: flex; gap: 0.4rem; align-items: center; }
  .icon-btn { display: grid; place-items: center; width: 26px; height: 26px; border-radius: 7px; border: 1px solid transparent; background: transparent; color: #64748b; cursor: pointer; }
  .icon-btn:hover { background: #f1f5f9; color: #334155; }
  .icon-danger:hover { background: #fef2f2; color: #b91c1c; }

  .layout { display: grid; grid-template-columns: minmax(200px, 230px) minmax(0, 1fr); gap: 1rem; align-items: start; }
  .facets { padding: 0.85rem !important; display: flex; flex-direction: column; gap: 0.85rem; max-height: calc(100vh - 14rem); overflow: auto; }
  .facet-group h4 { margin: 0 0 0.4rem; font-size: 0.7rem; font-weight: 700; text-transform: uppercase; letter-spacing: 0.08em; color: #64748b; }
  .facet { display: flex; justify-content: space-between; align-items: center; gap: 0.5rem; width: 100%; padding: 0.3rem 0.5rem; border: 1px solid transparent; border-radius: 8px; background: transparent; font-size: 0.8rem; color: #334155; cursor: pointer; text-transform: capitalize; }
  .facet:hover { background: #f8fafc; }
  .facet.active { background: #ecfeff; color: #0e7490; border-color: #7ED6D0; }
  .facet-label { display: inline-flex; align-items: center; gap: 0.3rem; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .facet-count { font-size: 0.72rem; color: #94a3b8; font-weight: 600; flex-shrink: 0; }
  .facet-empty { font-size: 0.8rem; color: #94a3b8; }

  .results { min-width: 0; }
  .placeholder { display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 0.7rem; padding: 4rem 1.5rem; color: #64748b; text-align: center; background: var(--surface); border: 1px dashed var(--line-soft); border-radius: 14px; }
  .placeholder h3 { margin: 0; }
  .placeholder p { margin: 0; max-width: 32rem; font-size: 0.9rem; color: #94a3b8; }

  .group-label { display: flex; align-items: baseline; gap: 0.4rem; margin: 0 0 0.55rem; font-size: 0.72rem; font-weight: 700; text-transform: uppercase; letter-spacing: 0.08em; color: #64748b; }
  .group-label + .set-grid { margin-bottom: 1.1rem; }
  .group-count { font-size: 0.7rem; font-weight: 600; color: #94a3b8; }
  .set-grid { list-style: none; padding: 0; margin: 0; display: grid; grid-template-columns: repeat(auto-fill, minmax(320px, 1fr)); gap: 0.85rem; }
  .set-card { border: 1px solid var(--line-soft); border-radius: 12px; background: #fff; display: flex; flex-direction: column; transition: border-color 0.12s, box-shadow 0.12s; }
  .set-card:hover { border-color: #7ED6D0; box-shadow: var(--shadow-sm); }
  :global(.set-card-main) { display: block; padding: 0.85rem 1rem; color: inherit; text-decoration: none; }
  .set-head { display: flex; align-items: center; gap: 0.4rem; flex-wrap: wrap; }
  :global(.set-icon) { color: #2F7A8C; flex-shrink: 0; }
  .set-name { font-weight: 600; font-size: 0.95rem; color: #1e293b; }
  .set-desc { margin: 0.4rem 0 0; font-size: 0.82rem; color: #64748b; line-height: 1.4; max-height: 2.6em; overflow: hidden; }
  .set-meta { display: flex; gap: 0.7rem; margin-top: 0.6rem; align-items: center; flex-wrap: wrap; font-size: 0.78rem; color: #475569; }
  .set-stat strong { color: #1e293b; font-weight: 700; }
  .set-stat.dim { color: #94a3b8; margin-left: auto; }
  .targets { display: flex; gap: 0.25rem; flex-wrap: wrap; margin-top: 0.55rem; }
  .chip { display: inline-flex; align-items: center; gap: 0.2rem; font-size: 0.68rem; padding: 2px 7px; border-radius: 999px; font-weight: 600; }
  .chip-vis { background: #f1f5f9; color: #475569; text-transform: capitalize; }
  .chip-target { background: #ecfeff; color: #0e7490; font-family: 'IBM Plex Mono', monospace; font-weight: 500; }
  .chip-source { background: #ede9fe; color: #5b21b6; text-transform: capitalize; }
  .chip-source-derived { background: #fef3c7; color: #92400e; }
  .chip-source-ai { background: #fce7f3; color: #9d174d; }
  .chip-source-imported { background: #dbeafe; color: #1d4ed8; }
  .chip-source-standard { background: #f1f5f9; color: #475569; }
  .chip-status { text-transform: capitalize; }
  .chip-status-draft { background: #f3f4f6; color: #6b7280; }
  .chip-status-staged { background: #fef3c7; color: #92400e; }
  .chip-status-published { background: #dcfce7; color: #166534; }
  .chip-status-deprecated { background: #fee2e2; color: #991b1b; }
  .chip-more { background: #f1f5f9; color: #64748b; }
  .set-actions { display: flex; gap: 0.2rem; padding: 0 0.5rem 0.6rem; }

  .modal-backdrop { position: fixed; inset: 0; background: rgba(15,23,42,0.45); display: grid; place-items: center; z-index: 100; }
  .modal { background: #fff; border-radius: 14px; width: min(440px, 92vw); box-shadow: 0 20px 50px rgba(15,23,42,0.25); }
  .modal-head { display: flex; align-items: center; justify-content: space-between; padding: 0.85rem 1rem; border-bottom: 1px solid var(--line-soft); }
  .modal-head h3 { margin: 0; font-size: 1rem; }
  .modal-body { padding: 1rem; display: flex; flex-direction: column; gap: 0.7rem; }
  .modal-body label { display: flex; flex-direction: column; gap: 0.25rem; font-size: 0.82rem; color: #475569; font-weight: 600; }
  .modal-body input, .modal-body textarea { padding: 0.45rem 0.6rem; font-size: 0.88rem; border: 1px solid var(--line-soft); border-radius: 8px; }
  .modal-actions { display: flex; justify-content: flex-end; gap: 0.5rem; margin-top: 0.4rem; }

  @media (max-width: 760px) {
    .layout { grid-template-columns: 1fr; }
    .facets { max-height: none; }
  }

  /* ---- Dark mode overrides (scoped rules out-specify global theme.css) ---- */
  :global(:is([data-theme="dark"], .dark)) .view-toggle { background: var(--bg-strong); }
  :global(:is([data-theme="dark"], .dark)) .vt { color: var(--ink-500); }
  :global(:is([data-theme="dark"], .dark)) .vt:hover { background: rgba(255,255,255,0.06); color: var(--ink-800); }
  :global(:is([data-theme="dark"], .dark)) .vt.active { background: var(--brand-100); color: var(--brand-700); }
  :global(:is([data-theme="dark"], .dark)) .error { color: #fca5a5; background: rgba(220,38,38,0.12); border-color: rgba(220,38,38,0.35); }
  :global(:is([data-theme="dark"], .dark)) .search-wrap { background: var(--bg-soft); color: var(--ink-500); }
  :global(:is([data-theme="dark"], .dark)) .search-input { color: var(--ink-900); }
  :global(:is([data-theme="dark"], .dark)) .icon-btn { color: var(--ink-500); }
  :global(:is([data-theme="dark"], .dark)) .icon-btn:hover { background: rgba(255,255,255,0.06); color: var(--ink-800); }
  :global(:is([data-theme="dark"], .dark)) .icon-danger:hover { background: rgba(239,68,68,0.14); color: #fca5a5; }
  :global(:is([data-theme="dark"], .dark)) .facet-group h4 { color: var(--ink-500); }
  :global(:is([data-theme="dark"], .dark)) .group-label { color: var(--ink-500); }
  :global(:is([data-theme="dark"], .dark)) .group-count { color: var(--ink-400); }
  :global(:is([data-theme="dark"], .dark)) .facet { color: var(--ink-800); }
  :global(:is([data-theme="dark"], .dark)) .facet:hover { background: rgba(255,255,255,0.06); }
  :global(:is([data-theme="dark"], .dark)) .facet.active { background: var(--brand-100); color: var(--brand-700); border-color: var(--brand-300); }
  :global(:is([data-theme="dark"], .dark)) .placeholder { color: var(--ink-500); }
  :global(:is([data-theme="dark"], .dark)) .set-card { background: var(--bg-strong); }
  :global(:is([data-theme="dark"], .dark)) .set-card:hover { border-color: var(--brand-300); }
  :global(:is([data-theme="dark"], .dark) .set-icon) { color: var(--brand-700); }
  :global(:is([data-theme="dark"], .dark)) .set-name,
  :global(:is([data-theme="dark"], .dark)) .set-stat strong { color: var(--ink-900); }
  :global(:is([data-theme="dark"], .dark)) .set-desc { color: var(--ink-500); }
  :global(:is([data-theme="dark"], .dark)) .set-meta { color: var(--ink-700); }
  :global(:is([data-theme="dark"], .dark)) .chip-vis,
  :global(:is([data-theme="dark"], .dark)) .chip-more { background: rgba(255,255,255,0.06); color: var(--ink-500); }
  :global(:is([data-theme="dark"], .dark)) .chip-target { background: var(--brand-100); color: var(--brand-700); }
  :global(:is([data-theme="dark"], .dark)) .chip-source { background: rgba(139,92,246,0.2); color: #c4b5fd; }
  :global(:is([data-theme="dark"], .dark)) .chip-source-derived { background: rgba(245,158,11,0.18); color: #fcd34d; }
  :global(:is([data-theme="dark"], .dark)) .chip-source-ai { background: rgba(236,72,153,0.2); color: #f9a8d4; }
  :global(:is([data-theme="dark"], .dark)) .chip-source-imported { background: rgba(59,130,246,0.2); color: #93c5fd; }
  :global(:is([data-theme="dark"], .dark)) .chip-source-standard { background: rgba(255,255,255,0.06); color: var(--ink-500); }
  :global(:is([data-theme="dark"], .dark)) .modal { background: var(--bg-strong); }
  :global(:is([data-theme="dark"], .dark)) .modal-body label { color: var(--ink-700); }
</style>
