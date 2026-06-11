<script>
  import { onMount } from 'svelte';
  import { t } from 'svelte-i18n';
  import { navigate } from '../lib/router/index.js';
  import {
    ArrowLeft, Plus, Upload, GitCompare, CheckCircle, Trash2, Loader2,
    Download, Copy, CheckCheck, Lock, Globe, Pencil, X, Save, BookOpen,
    ChevronDown, ChevronUp, PlayCircle, Users, Building2, User, Maximize2,
  } from 'lucide-svelte';
  import {
    getDataModel, listDataModelVersions, deleteDataModelVersion, stageDataModelVersion,
    publishDataModelVersion, createDataModelDraft, getDataModelVersionDataUrl,
    updateDataModel, updateDataModelVersionNotes, uploadDataModelVersion,
    getOrganisation, listOrgMembers, listPublicUsers, getDataModelCollaborators,
    subgraphActionDataModel,
  } from '../lib/api.js';
  import { isAdmin, user } from '../lib/stores.js';
  import { copyToClipboard } from '../lib/clipboard.js';
  import UploadVersionDialog from '../components/UploadVersionDialog.svelte';
  import PublishConfirmDialog from '../components/PublishConfirmDialog.svelte';
  import ConfirmModal from '../components/ConfirmModal.svelte';
  import ContentKindWarning from '../components/ContentKindWarning.svelte';
  import OntologyBrowserPanel from '../components/OntologyBrowserPanel.svelte';
  import BranchPanel from '../components/BranchPanel.svelte';
  import CommitHistory from '../components/CommitHistory.svelte';

  // Inline browser — which version is expanded
  let browserVersion = null;
  function toggleBrowser(ver) {
    browserVersion = browserVersion === ver ? null : ver;
  }

  export let id;

  let model = null;
  let versions = [];
  let collaborators = [];
  let loading = false;
  let error = '';

  let showUpload = false;
  let showPublishConfirm = false;
  let publishTarget = null;

  let deleteLoading = '';
  let draftLoading = '';
  let stageLoading = '';

  // Copy-to-clipboard
  let copiedApi = null;
  async function copyApiUrl(key, url) {
    if (await copyToClipboard(url)) {
      copiedApi = key;
      setTimeout(() => { copiedApi = null; }, 2000);
    }
  }

  let currentUser = null;
  user.subscribe(v => currentUser = v);
  $: isPublisher = currentUser?.can_publish || currentUser?.role === 'admin' || currentUser?.role === 'super_admin';

  // ── Edit model metadata ────────────────────────────────────────────────────
  let showEdit = false;
  let editForm = { title: '', namespace: '', description: '', is_public: false };
  let editLoading = false;
  let editError = '';

  function openEdit() {
    editForm = {
      title: model.title,
      namespace: model.namespace,
      description: model.description || '',
      is_public: model.is_public,
    };
    editError = '';
    showEdit = true;
  }

  async function handleSaveEdit() {
    editLoading = true;
    editError = '';
    try {
      await updateDataModel(id, {
        title: editForm.title || undefined,
        namespace: editForm.namespace || undefined,
        description: editForm.description,
        is_public: editForm.is_public,
      });
      showEdit = false;
      await load();
    } catch (e) {
      editError = e.message;
    }
    editLoading = false;
  }

  // ── Inline notes editing ───────────────────────────────────────────────────
  let editingNotes = '';
  let notesValue = '';
  let notesSaving = false;

  function startEditNotes(ver) {
    editingNotes = ver.version;
    notesValue = ver.notes || '';
  }
  function cancelEditNotes() { editingNotes = ''; }

  async function saveNotes(ver) {
    notesSaving = true;
    try {
      await updateDataModelVersionNotes(id, ver, notesValue);
      editingNotes = '';
      await load();
    } catch (e) {
      alert(e.message);
    }
    notesSaving = false;
  }

  // ── Ownership & members ───────────────────────────────────────────────────
  let ownerOrg = null;
  let ownerUser = null;
  let orgMembers = [];
  let userMap = {};

  onMount(load);

  async function load() {
    loading = true;
    error = '';
    try {
      [model, versions] = await Promise.all([
        getDataModel(id),
        listDataModelVersions(id),
      ]);
      loadOwnership(model);
      getDataModelCollaborators(id).then(c => { collaborators = c || []; }).catch(() => {});
    } catch (e) {
      error = e.message;
    }
    loading = false;
  }

  async function loadOwnership(m) {
    if (!m) return;
    try {
      const users = await listPublicUsers().catch(() => []);
      userMap = Object.fromEntries((users || []).map(u => [String(u.id), u.username || u.email || u.id]));
    } catch {}
    if (m.owner_type === 'organisation' && m.owner_id) {
      try {
        ownerOrg = await getOrganisation(m.owner_id);
        orgMembers = await listOrgMembers(m.owner_id).catch(() => []);
      } catch {}
    } else if (m.owner_id) {
      ownerUser = userMap[String(m.owner_id)] || null;
    }
  }

  let deleteVersionTarget = null;

  async function doDeleteVersion() {
    const ver = deleteVersionTarget;
    deleteVersionTarget = null;
    deleteLoading = ver;
    try {
      await deleteDataModelVersion(id, ver);
      await load();
    } catch (e) {
      alert(e.message);
    }
    deleteLoading = '';
  }

  async function handleCreateDraft(fromVer) {
    const targetVer = prompt($t('pages.modelDetail.createDraftPrompt', { values: { version: fromVer } }));
    if (!targetVer) return;
    draftLoading = fromVer;
    try {
      await createDataModelDraft(id, fromVer, targetVer.trim());
      await load();
    } catch (e) {
      alert(e.message);
    }
    draftLoading = '';
  }

  async function handleStage(ver) {
    stageLoading = ver;
    try {
      await stageDataModelVersion(id, ver);
      await load();
    } catch (e) {
      alert(e.message);
    }
    stageLoading = '';
  }

  function openPublish(ver) {
    publishTarget = ver;
    showPublishConfirm = true;
  }

  async function handlePublished() {
    showPublishConfirm = false;
    await load();
  }

  function diffUrl(from, to) {
    return `/models/${id}/diff?from=${encodeURIComponent(from)}&to=${encodeURIComponent(to)}`;
  }

  function statusBadge(status) {
    const map = {
      published: 'bg-green-100 text-green-700',
      staged: 'bg-blue-100 text-blue-700',
      draft: 'bg-amber-100 text-amber-700',
      deprecated: 'bg-gray-100 text-gray-500',
    };
    return map[status] || 'bg-gray-100 text-gray-500';
  }

  // ── Per-subgraph publishing (Phase 6) ──────────────────────────────────────
  let subgraphLoading = '';
  /** Effective status of a subgraph = its override, else the version status. */
  function subgraphStatus(ver, graph) {
    const o = (ver.sub_graph_status || []).find((e) => e.graph_iri === graph);
    return o ? o.status : ver.status;
  }
  async function handleSubgraphAction(ver, graph, action) {
    subgraphLoading = `${ver}::${graph}`;
    try {
      await subgraphActionDataModel(id, ver, action, graph);
      await load();
    } catch (e) {
      alert(e.message);
    }
    subgraphLoading = '';
  }
  function shortGraph(g) {
    return g.split('/').pop() || g;
  }
</script>

<div class="space-y-6">
  <!-- Back -->
  <div class="flex items-center gap-3">
    <button class="btn btn-ghost btn-sm" on:click={() => navigate('/models')}>
      <ArrowLeft size={16} /> {$t('system.back')}
    </button>
  </div>

  {#if loading}
    <div class="flex items-center justify-center py-16 text-[var(--ink-400)]">
      <Loader2 size={24} class="animate-spin mr-2" /> {$t('system.loading')}
    </div>
  {:else if error}
    <div class="p-4 rounded-xl bg-red-50 border border-red-200 text-red-700 text-sm">{error}</div>
  {:else if model}

    <!-- Header -->
    <div class="flex flex-wrap items-start justify-between gap-4">
      <div class="min-w-0 flex-1">
        <div class="flex items-center gap-2 flex-wrap">
          <h2 class="text-2xl font-bold m-0">{model.title}</h2>
          {#if model.is_public}
            <span class="vis-badge vis-public"><Globe size={11} /> {$t('pages.modelDetail.public')}</span>
          {:else}
            <span class="vis-badge vis-private"><Lock size={11} /> {$t('pages.modelDetail.private')}</span>
          {/if}
        </div>
        <div class="text-sm text-[var(--ink-400)] mt-1">
          <code>{id}</code>
          {#if model.namespace}
            · <span class="break-all">{model.namespace}</span>
          {/if}
        </div>
        {#if model.description}
          <p class="text-sm text-[var(--ink-600)] mt-2 m-0">{model.description}</p>
        {/if}
      </div>
      <div class="flex items-center gap-2 flex-wrap">
        {#if isPublisher}
          <button class="btn btn-ghost btn-sm" on:click={openEdit} title={$t('pages.modelDetail.editMetadata')}>
            <Pencil size={15} /> {$t('system.edit')}
          </button>
          <button class="btn btn-primary btn-sm" on:click={() => showUpload = true}>
            <Upload size={15} /> {$t('pages.modelDetail.uploadVersion')}
          </button>
        {/if}
      </div>
    </div>

    <!-- Edit panel -->
    {#if showEdit}
      <div class="edit-panel">
        <div class="flex items-center justify-between mb-4">
          <h3 class="font-semibold m-0">{$t('pages.modelDetail.editModel')}</h3>
          <button class="btn btn-ghost btn-xs" on:click={() => showEdit = false}><X size={14} /></button>
        </div>
        <form on:submit|preventDefault={handleSaveEdit} class="space-y-3">
          <div class="form-row">
            <div>
              <label class="label" for="edit-title">{$t('pages.modelDetail.titleLabel')}</label>
              <input id="edit-title" type="text" class="input" bind:value={editForm.title} required />
            </div>
            <div>
              <label class="label" for="edit-ns">{$t('pages.modelDetail.namespaceUri')}</label>
              <input id="edit-ns" type="text" class="input" bind:value={editForm.namespace} placeholder="https://example.com/model/" />
            </div>
          </div>
          <div>
            <label class="label" for="edit-desc">{$t('pages.modelDetail.descriptionLabel')}</label>
            <textarea id="edit-desc" class="input" rows="2" bind:value={editForm.description} placeholder={$t('pages.modelDetail.descriptionPlaceholder')}></textarea>
          </div>
          <label class="flex items-center gap-2 cursor-pointer text-sm">
            <input type="checkbox" bind:checked={editForm.is_public} class="w-4 h-4 accent-[var(--brand-500)]" />
            <span>{$t('pages.modelDetail.publicAccessLabel')}</span>
          </label>
          {#if editError}
            <p class="text-sm text-red-600">{editError}</p>
          {/if}
          <div class="flex gap-2 justify-end">
            <button type="button" class="btn btn-ghost btn-sm" on:click={() => showEdit = false}>{$t('system.cancel')}</button>
            <button type="submit" class="btn btn-primary btn-sm" disabled={editLoading}>
              {#if editLoading}<Loader2 size={13} class="animate-spin" />{/if}
              <Save size={13} /> {$t('system.save')}
            </button>
          </div>
        </form>
      </div>
    {/if}

    <!-- API Endpoints -->
    <div class="api-bar">
      <span class="api-bar-label">API</span>
      <div class="api-row">
        <span class="api-method">GET</span>
        <code class="api-url">/api/models/{id}/versions</code>
        <button class="btn btn-xs btn-ghost copy-btn" title={$t('system.copy')}
          on:click={() => copyApiUrl('versions', `${window.location.origin}/api/models/${id}/versions`)}>
          {#if copiedApi === 'versions'}<CheckCheck size={12} />{:else}<Copy size={12} />{/if}
        </button>
        <span class="api-note">{$t('pages.modelDetail.listAllVersions')}</span>
      </div>
      <div class="api-row">
        <span class="api-method">GET</span>
        <code class="api-url">/api/models/{id}/latest/data</code>
        <button class="btn btn-xs btn-ghost copy-btn" title={$t('system.copy')}
          on:click={() => copyApiUrl('latest', `${window.location.origin}/api/models/${id}/latest/data`)}>
          {#if copiedApi === 'latest'}<CheckCheck size={12} />{:else}<Copy size={12} />{/if}
        </button>
        <span class="api-note">{$t('pages.modelDetail.latestPublishedData')} (<code>Accept</code>)</span>
      </div>
      {#if !model.is_public}
        <div class="api-auth-hint">
          <Lock size={11} /> {$t('pages.modelDetail.bearerTokenRequired')}
        </div>
      {/if}
    </div>

    <!-- Ownership -->
    {#if model.owner_id}
      <div class="ownership-card">
        <div class="ownership-header">
          {#if model.owner_type === 'organisation'}
            <Building2 size={15} class="text-[var(--brand-500)]" />
            <span class="ownership-label">{$t('pages.modelDetail.organisation')}</span>
          {:else}
            <User size={15} class="text-[var(--brand-500)]" />
            <span class="ownership-label">{$t('pages.modelDetail.owner')}</span>
          {/if}
        </div>
        <div class="ownership-body">
          {#if model.owner_type === 'organisation' && ownerOrg}
            <a href="/organisations/{model.owner_id}" class="ownership-name">{ownerOrg.name}</a>
            {#if ownerOrg.description}
              <p class="ownership-desc">{ownerOrg.description}</p>
            {/if}
            {#if orgMembers.length > 0}
              <div class="members-row">
                <Users size={12} class="text-[var(--ink-400)]" />
                <span class="members-label">{orgMembers.length === 1 ? $t('pages.modelDetail.memberCount', { values: { count: orgMembers.length } }) : $t('pages.modelDetail.memberCountPlural', { values: { count: orgMembers.length } })}</span>
                <div class="members-list">
                  {#each orgMembers.slice(0, 8) as m}
                    <span class="member-chip" title={m.role}>{m.username || m.user_id}</span>
                  {/each}
                  {#if orgMembers.length > 8}
                    <span class="member-chip member-chip-more">{$t('pages.modelDetail.moreCount', { values: { count: orgMembers.length - 8 } })}</span>
                  {/if}
                </div>
              </div>
            {/if}
          {:else}
            <span class="ownership-name">{ownerUser || model.owner_id}</span>
          {/if}
        </div>
      </div>
    {/if}

    <!-- Collaborators -->
    {#if collaborators.length > 0}
      <div class="ownership-card">
        <div class="ownership-header">
          <Users size={15} class="text-[var(--brand-500)]" />
          <span class="ownership-label">{$t('pages.modelDetail.collaborators', { values: { count: collaborators.length } })}</span>
        </div>
        <div class="ownership-body">
          <div class="members-list">
            {#each collaborators as c}
              <span class="member-chip" title={`${c.email} · ${c.source}`}>
                {c.display_name || c.username} · {c.role}
              </span>
            {/each}
          </div>
        </div>
      </div>
    {/if}

    <!-- Branches -->
    {#if model}
      <BranchPanel {id} {versions} canWrite={isPublisher} on:created={load} />
    {/if}

    <!-- Commit history -->
    {#if model}
      <CommitHistory
        kind="data-model"
        {id}
        branches={[...new Set((versions || []).filter(v => v.branch).map(v => v.branch))].map(b => ({ branch: b }))}
      />
    {/if}

    <!-- Versions -->
    <div>
      <h3 class="text-base font-semibold mb-3">{$t('pages.modelDetail.versions', { values: { count: versions.length } })}</h3>

      {#if versions.length === 0}
        <div class="text-center py-8 text-[var(--ink-400)] text-sm">{$t('pages.modelDetail.noVersions')}</div>
      {:else}
        <div class="space-y-3">
          {#each versions as ver (ver.version)}
            <div class="card ver-card" class:ver-browsing={browserVersion === ver.version}>
              <!-- Version header row -->
              <div class="ver-header">
                <!-- Left: version info -->
                <button
                  class="ver-title-btn"
                  on:click={() => toggleBrowser(ver.version)}
                  title={browserVersion === ver.version ? $t('pages.modelDetail.collapseBrowser') : $t('pages.modelDetail.browseThisVersion')}
                >
                  <span class="font-mono font-bold text-[var(--ink-900)]">v{ver.version}</span>
                  <span class="text-xs px-2 py-0.5 rounded-full font-medium {statusBadge(ver.status)}">{$t(`pages.modelDetail.status.${ver.status}`)}</span>
                  {#if ver.derived_from}
                    <span class="text-xs text-[var(--ink-400)]">{$t('pages.modelDetail.derivedFrom', { values: { version: ver.derived_from } })}</span>
                  {/if}
                  {#if browserVersion === ver.version}
                    <ChevronUp size={14} class="ml-1 text-[var(--brand-500)]" />
                  {:else}
                    <ChevronDown size={14} class="ml-1 text-[var(--ink-300)]" />
                  {/if}
                </button>

                <!-- Right: action toolbar -->
                <div class="ver-actions">
                  <!-- Browse button -->
                  <button
                    class="ver-btn ver-btn-browse"
                    class:ver-btn-active={browserVersion === ver.version}
                    on:click={() => toggleBrowser(ver.version)}
                    title={$t('pages.modelDetail.openInlineBrowser')}
                  >
                    <BookOpen size={13} />
                    {browserVersion === ver.version ? $t('system.close') : $t('pages.modelDetail.browse')}
                  </button>
                  <button
                    class="ver-btn"
                    on:click={() => navigate(`/models/${id}/viewer/${ver.version}`)}
                    title={$t('pages.modelDetail.openFullViewer')}
                  >
                    <Maximize2 size={13} /> {$t('pages.modelDetail.openViewer')}
                  </button>

                  <span class="ver-sep"></span>

                  <!-- Download + Copy URL -->
                  <a
                    href={getDataModelVersionDataUrl(id, ver.version, 'trig')}
                    download="{id}-{ver.version}.trig"
                    class="ver-btn"
                    title={$t('pages.modelDetail.downloadTrig')}
                  >
                    <Download size={13} /> {$t('pages.modelDetail.download')}
                  </a>
                  <button
                    class="ver-btn"
                    title={$t('pages.modelDetail.copyEndpointUrl')}
                    on:click={() => copyApiUrl(`ver-${ver.version}`, `${window.location.origin}/api/models/${id}/versions/${ver.version}/data`)}
                  >
                    {#if copiedApi === `ver-${ver.version}`}
                      <CheckCheck size={13} class="text-green-600" /> {$t('system.copied')}
                    {:else}
                      <Copy size={13} /> {$t('pages.modelDetail.copyUrl')}
                    {/if}
                  </button>

                  <span class="ver-sep"></span>

                  <!-- Edit notes -->
                  {#if isPublisher && editingNotes !== ver.version}
                    <button class="ver-btn" title={$t('pages.modelDetail.editReleaseNotes')} on:click={() => startEditNotes(ver)}>
                      <Pencil size={13} /> {$t('pages.modelDetail.notes')}
                    </button>
                  {/if}

                  <!-- Diff -->
                  {#if versions.indexOf(ver) < versions.length - 1}
                    <a
                      href={diffUrl(versions[versions.indexOf(ver) + 1].version, ver.version)}
                      class="ver-btn"
                      title={$t('pages.modelDetail.compareWithPrevious')}
                    >
                      <GitCompare size={13} /> {$t('pages.modelDetail.diff')}
                    </a>
                  {/if}

                  <!-- Create draft -->
                  {#if isPublisher && ver.status === 'published'}
                    <span class="ver-sep"></span>
                    <button
                      class="ver-btn"
                      title={$t('pages.modelDetail.createDraftFromVersion')}
                      disabled={draftLoading === ver.version}
                      on:click={() => handleCreateDraft(ver.version)}
                    >
                      {#if draftLoading === ver.version}
                        <Loader2 size={13} class="animate-spin" /> {$t('pages.modelDetail.creating')}
                      {:else}
                        <Plus size={13} /> {$t('pages.modelDetail.newDraft')}
                      {/if}
                    </button>
                  {/if}

                  <!-- Stage -->
                  {#if isPublisher && ver.status === 'draft'}
                    <span class="ver-sep"></span>
                    <button
                      class="ver-btn ver-btn-stage"
                      disabled={stageLoading === ver.version}
                      on:click={() => handleStage(ver.version)}
                      title={$t('pages.modelDetail.stageForReview')}
                    >
                      {#if stageLoading === ver.version}
                        <Loader2 size={13} class="animate-spin" /> {$t('pages.modelDetail.staging')}
                      {:else}
                        <PlayCircle size={13} /> {$t('pages.modelDetail.stage')}
                      {/if}
                    </button>
                  {/if}

                  <!-- Publish -->
                  {#if isPublisher && (ver.status === 'draft' || ver.status === 'staged')}
                    <button class="ver-btn ver-btn-publish" on:click={() => openPublish(ver.version)}>
                      <CheckCircle size={13} /> {$t('pages.modelDetail.publish')}
                    </button>
                  {/if}

                  <!-- Delete -->
                  {#if $isAdmin}
                    <button
                      class="ver-btn ver-btn-danger"
                      title={$t('pages.modelDetail.deleteVersion')}
                      disabled={deleteLoading === ver.version}
                      on:click|stopPropagation={() => deleteVersionTarget = ver.version}
                    >
                      {#if deleteLoading === ver.version}
                        <Loader2 size={13} class="animate-spin" />
                      {:else}
                        <Trash2 size={13} />
                      {/if}
                    </button>
                  {/if}
                </div>
              </div>

              <!-- Notes editing / display -->
              {#if editingNotes === ver.version}
                <div class="mt-2 flex gap-2 items-start">
                  <!-- svelte-ignore a11y_autofocus -->
                  <textarea
                    class="input text-sm flex-1"
                    rows="2"
                    bind:value={notesValue}
                    placeholder={$t('pages.modelDetail.releaseNotesPlaceholder')}
                    autofocus
                  ></textarea>
                  <div class="flex flex-col gap-1">
                    <button class="btn btn-primary btn-xs" disabled={notesSaving} on:click={() => saveNotes(ver.version)}>
                      {#if notesSaving}<Loader2 size={12} class="animate-spin" />{:else}<Save size={12} />{/if}
                    </button>
                    <button class="btn btn-ghost btn-xs" on:click={cancelEditNotes}><X size={12} /></button>
                  </div>
                </div>
              {:else if ver.notes}
                <p class="text-sm text-[var(--ink-500)] mt-1 mb-0">{ver.notes}</p>
              {/if}

              <!-- Meta line -->
              <div class="text-xs text-[var(--ink-400)] mt-1">
                {#if ver.created_at}{new Date(ver.created_at).toLocaleDateString()}{/if}
                {#if ver.sub_graphs?.length}
                  · {ver.sub_graphs.length === 1 ? $t('pages.modelDetail.subGraphCount', { values: { count: ver.sub_graphs.length } }) : $t('pages.modelDetail.subGraphCountPlural', { values: { count: ver.sub_graphs.length } })}
                {/if}
              </div>

              <!-- Per-subgraph publishing (Phase 6) -->
              {#if ver.sub_graphs?.length}
                <div class="subgraph-list">
                  {#each ver.sub_graphs as sg (sg)}
                    {@const sgStatus = subgraphStatus(ver, sg)}
                    {@const busy = subgraphLoading === `${ver.version}::${sg}`}
                    <div class="subgraph-row">
                      <code class="subgraph-name" title={sg}>{shortGraph(sg)}</code>
                      <span class="text-[10px] px-1.5 py-0.5 rounded-full font-medium {statusBadge(sgStatus)}">{$t(`pages.modelDetail.status.${sgStatus}`)}</span>
                      {#if isPublisher}
                        <span class="flex-1"></span>
                        {#if busy}
                          <Loader2 size={12} class="animate-spin text-[var(--ink-400)]" />
                        {/if}
                        {#if sgStatus === 'draft'}
                          <button class="sg-btn" disabled={busy} on:click={() => handleSubgraphAction(ver.version, sg, 'stage')}>{$t('pages.modelDetail.stage')}</button>
                        {/if}
                        {#if sgStatus === 'draft' || sgStatus === 'staged'}
                          <button class="sg-btn sg-btn-publish" disabled={busy} on:click={() => handleSubgraphAction(ver.version, sg, 'publish')}>{$t('pages.modelDetail.publish')}</button>
                        {/if}
                        {#if sgStatus === 'published'}
                          <button class="sg-btn sg-btn-danger" disabled={busy} on:click={() => handleSubgraphAction(ver.version, sg, 'deprecate')}>{$t('pages.modelDetail.deprecate')}</button>
                        {/if}
                      {/if}
                    </div>
                  {/each}
                </div>
              {/if}

              <!-- Inline browser -->
              {#if browserVersion === ver.version}
                <div class="browser-panel">
                  <ContentKindWarning
                    graphs={[ver.graph_iri, ...(ver.sub_graphs || [])]}
                    expected="model"
                    contextName={model?.title}
                  />
                  <OntologyBrowserPanel
                    graphIri={ver.graph_iri}
                    subGraphs={ver.sub_graphs || []}
                    title={model.title}
                    versionLabel={ver.version}
                    rawDataUrl={getDataModelVersionDataUrl(id, ver.version, 'turtle', 'all')}
                  />
                </div>
              {/if}
            </div>
          {/each}
        </div>
      {/if}
    </div>
  {/if}
</div>

{#if showUpload}
  <UploadVersionDialog
    {id}
    kind="model"
    uploadFn={uploadDataModelVersion}
    on:uploaded={() => { showUpload = false; load(); }}
    on:cancel={() => showUpload = false}
  />
{/if}

{#if showPublishConfirm && publishTarget}
  <PublishConfirmDialog
    registryId={id}
    version={publishTarget}
    publishFn={publishDataModelVersion}
    on:published={handlePublished}
    on:cancel={() => { showPublishConfirm = false; publishTarget = null; }}
  />
{/if}

{#if deleteVersionTarget}
  <ConfirmModal
    title={$t('pages.modelDetail.deleteVersionConfirmTitle', { values: { version: deleteVersionTarget } })}
    message={$t('pages.modelDetail.deleteVersionConfirmMessage')}
    confirmLabel={$t('pages.modelDetail.deleteVersion')}
    on:confirm={doDeleteVersion}
    on:cancel={() => deleteVersionTarget = null}
  />
{/if}

<style>
  .btn { display: inline-flex; align-items: center; gap: 0.375rem; padding: 0.5rem 1rem; border-radius: 0.75rem; font-size: 0.875rem; font-weight: 500; cursor: pointer; border: none; transition: all 0.15s; text-decoration: none; }
  .btn-primary { background: var(--brand-500, #6366f1); color: white; }
  .btn-primary:hover { background: var(--brand-600, #4f46e5); }
  .btn-ghost { background: transparent; color: var(--ink-600, #475569); }
  .btn-ghost:hover { background: var(--bg-soft, #f1f5f9); }
  .btn-sm { padding: 0.375rem 0.75rem; font-size: 0.8125rem; }
  .btn-xs { padding: 0.25rem 0.5rem; font-size: 0.75rem; }
  .btn:disabled { opacity: 0.6; cursor: not-allowed; }

  .vis-badge { display: inline-flex; align-items: center; gap: 0.25rem; font-size: 0.7rem; font-weight: 600; padding: 0.15rem 0.5rem; border-radius: 999px; }
  .vis-public { background: #dcfce7; color: #15803d; }
  .vis-private { background: #fef3c7; color: #92400e; }

  .api-bar { background: var(--bg-soft, #f8fafc); border: 1px solid var(--border, #e2e8f0); border-radius: 0.75rem; padding: 0.75rem 1rem; display: flex; flex-direction: column; gap: 0.45rem; }
  .api-bar-label { font-size: 0.7rem; font-weight: 700; letter-spacing: 0.08em; text-transform: uppercase; color: var(--ink-400, #94a3b8); }
  .api-row { display: flex; align-items: center; gap: 0.5rem; flex-wrap: wrap; }
  .api-method { font-size: 0.7rem; font-weight: 700; color: var(--brand-600, #4f46e5); background: var(--brand-50, #eef2ff); border-radius: 0.3rem; padding: 0.1rem 0.35rem; flex-shrink: 0; }
  .api-url { font-size: 0.78rem; color: var(--ink-700, #334155); background: transparent; padding: 0; border: none; }
  .api-note { font-size: 0.72rem; color: var(--ink-400, #94a3b8); }
  .copy-btn { padding: 0.15rem 0.35rem; flex-shrink: 0; }
  .api-auth-hint { display: flex; align-items: center; gap: 0.3rem; font-size: 0.72rem; color: #b45309; background: #fffbeb; border: 1px solid #fde68a; border-radius: 0.4rem; padding: 0.25rem 0.55rem; width: fit-content; }

  .edit-panel { background: var(--bg-soft, #f8fafc); border: 1px solid var(--border, #e2e8f0); border-radius: 0.75rem; padding: 1rem 1.25rem; }
  .form-row { display: grid; grid-template-columns: 1fr 1fr; gap: 0.75rem; }
  @media (max-width: 600px) { .form-row { grid-template-columns: 1fr; } }
  .label { display: block; font-size: 0.8125rem; font-weight: 500; margin-bottom: 0.2rem; color: var(--ink-700); }
  .input { width: 100%; padding: 0.4rem 0.65rem; border: 1px solid var(--line-soft, #d1d5db); border-radius: 0.5rem; font-size: 0.875rem; font-family: inherit; box-sizing: border-box; }
  .input:focus { outline: none; border-color: var(--brand-400); }
  textarea.input { resize: vertical; }

  .ver-card { transition: border-color 0.15s; }
  .ver-card.ver-browsing { border-color: var(--brand-300, #a5b4fc); }

  .ver-header { display: flex; align-items: center; justify-content: space-between; gap: 0.75rem; flex-wrap: wrap; }

  .ver-title-btn { display: inline-flex; align-items: center; gap: 0.5rem; flex-wrap: wrap; background: none; border: none; cursor: pointer; padding: 0; text-align: left; }

  .ver-actions { display: flex; align-items: center; gap: 0.2rem; flex-wrap: wrap; }

  .ver-btn {
    display: inline-flex; align-items: center; gap: 0.3rem;
    padding: 0.3rem 0.6rem; font-size: 0.75rem; font-weight: 500;
    border: 1px solid var(--line-soft, #e2e8f0); border-radius: 7px;
    background: white; color: var(--ink-600, #475569); cursor: pointer;
    text-decoration: none; transition: all 0.12s; white-space: nowrap;
  }
  .ver-btn:hover:not(:disabled) { background: var(--bg-soft, #f8fafc); border-color: var(--brand-200, #c7d2fe); color: var(--ink-900); }
  .ver-btn:disabled { opacity: 0.55; cursor: not-allowed; }
  .ver-btn-browse:hover:not(:disabled) { color: var(--brand-600); border-color: var(--brand-300); }
  .ver-btn-active { background: #e0f2fe; color: #0369a1; border-color: #bae6fd; }
  .ver-btn-stage { color: #1d4ed8; border-color: #bfdbfe; }
  .ver-btn-stage:hover:not(:disabled) { background: #eff6ff; }
  .ver-btn-publish { color: #15803d; border-color: #bbf7d0; }
  .ver-btn-publish:hover:not(:disabled) { background: #f0fdf4; }
  .ver-btn-danger { color: #ef4444; border-color: #fecaca; }
  .ver-btn-danger:hover:not(:disabled) { background: #fef2f2; }
  .ver-sep { width: 1px; height: 18px; background: var(--line-soft, #e2e8f0); flex-shrink: 0; }

  /* Per-subgraph publishing rows */
  .subgraph-list { margin-top: 0.5rem; display: flex; flex-direction: column; gap: 0.25rem; }
  .subgraph-row { display: flex; align-items: center; gap: 0.4rem; padding: 0.2rem 0.5rem; background: var(--bg-soft, #f8fafc); border: 1px solid var(--border, #e2e8f0); border-radius: 0.4rem; }
  .subgraph-name { font-size: 0.72rem; font-family: var(--mono, monospace); color: var(--ink-600, #475569); }
  .sg-btn { font-size: 0.68rem; padding: 0.1rem 0.45rem; border-radius: 0.3rem; border: 1px solid var(--border, #e2e8f0); background: var(--bg, #fff); color: var(--ink-500); cursor: pointer; }
  .sg-btn:hover:not(:disabled) { background: var(--bg-soft, #f1f5f9); }
  .sg-btn:disabled { opacity: 0.5; cursor: default; }
  .sg-btn-publish { color: #15803d; border-color: #bbf7d0; }
  .sg-btn-danger { color: #ef4444; border-color: #fecaca; }

  .browser-panel { margin-top: 0.75rem; padding-top: 0.75rem; border-top: 1px dashed var(--line-soft, #e2e8f0); }
  .ownership-card { background: var(--bg-soft, #f8fafc); border: 1px solid var(--border, #e2e8f0); border-radius: 0.75rem; padding: 0.875rem 1rem; }
  .ownership-header { display: flex; align-items: center; gap: 0.4rem; margin-bottom: 0.5rem; }
  .ownership-label { font-size: 0.7rem; font-weight: 700; letter-spacing: 0.06em; text-transform: uppercase; color: var(--ink-400, #94a3b8); }
  .ownership-body { display: flex; flex-direction: column; gap: 0.35rem; }
  .ownership-name { font-size: 0.875rem; font-weight: 600; color: var(--brand-600, #4f46e5); text-decoration: none; }
  a.ownership-name:hover { text-decoration: underline; }
  .ownership-desc { font-size: 0.8rem; color: var(--ink-500); margin: 0; }
  .members-row { display: flex; align-items: center; gap: 0.4rem; flex-wrap: wrap; margin-top: 0.25rem; }
  .members-label { font-size: 0.75rem; color: var(--ink-500); white-space: nowrap; }
  .members-list { display: flex; flex-wrap: wrap; gap: 0.3rem; }
  .member-chip { font-size: 0.72rem; padding: 0.15rem 0.45rem; border-radius: 999px; background: var(--brand-50, #eef2ff); color: var(--brand-700, #4338ca); border: 1px solid var(--brand-100, #e0e7ff); }
  .member-chip-more { background: var(--bg-soft); color: var(--ink-400); border-color: var(--border); }

  /* ---- Dark mode overrides (scoped rules out-specify global theme.css) ---- */
  :global(:is([data-theme="dark"], .dark)) .vis-public { background: rgba(16,185,129,0.18); color: #6ee7b7; }
  :global(:is([data-theme="dark"], .dark)) .vis-private { background: rgba(245,158,11,0.18); color: #fcd34d; }
  :global(:is([data-theme="dark"], .dark)) .api-method { background: var(--brand-100); }
  :global(:is([data-theme="dark"], .dark)) .api-auth-hint { color: #fcd34d; background: rgba(245,158,11,0.12); border-color: rgba(245,158,11,0.35); }
  :global(:is([data-theme="dark"], .dark)) .ver-btn { background: var(--bg-soft); }
  :global(:is([data-theme="dark"], .dark)) .ver-btn-active { background: rgba(59,130,246,0.18); color: #93c5fd; border-color: rgba(59,130,246,0.4); }
  :global(:is([data-theme="dark"], .dark)) .ver-btn-stage { color: #93c5fd; border-color: rgba(59,130,246,0.4); }
  :global(:is([data-theme="dark"], .dark)) .ver-btn-stage:hover:not(:disabled) { background: rgba(59,130,246,0.14); }
  :global(:is([data-theme="dark"], .dark)) .ver-btn-publish { color: #6ee7b7; border-color: rgba(16,185,129,0.4); }
  :global(:is([data-theme="dark"], .dark)) .ver-btn-publish:hover:not(:disabled) { background: rgba(16,185,129,0.14); }
  :global(:is([data-theme="dark"], .dark)) .ver-btn-danger { color: #fca5a5; border-color: rgba(239,68,68,0.4); }
  :global(:is([data-theme="dark"], .dark)) .ver-btn-danger:hover:not(:disabled) { background: rgba(239,68,68,0.14); }
  :global(:is([data-theme="dark"], .dark)) .sg-btn { background: var(--bg-soft); }
  :global(:is([data-theme="dark"], .dark)) .sg-btn-publish { color: #6ee7b7; border-color: rgba(16,185,129,0.4); }
  :global(:is([data-theme="dark"], .dark)) .sg-btn-danger { color: #fca5a5; border-color: rgba(239,68,68,0.4); }
  :global(:is([data-theme="dark"], .dark)) .member-chip { background: var(--brand-100); border-color: var(--brand-200); }
</style>
