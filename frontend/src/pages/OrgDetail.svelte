<script>
  import { onMount } from 'svelte';
  import {
    getOrganisation,
    listOrganisations,
    updateOrganisation,
    deleteOrganisation,
    listOrgMembers,
    addOrgMember,
    removeOrgMember,
    updateOrgMemberRole,
    listGroups,
    listGroupMembers,
    createGroup,
    deleteGroup,
    addGroupMember,
    removeGroupMember,
    listPublicUsers,
    listDatasets,
    createDataset,
    uploadOrgImage,
    getOrgImageUrl,
    uploadOrgBanner,
    setOrgBannerPreset,
    clearOrgBanner,
    getOrgBannerUrl,
    listDatasetGrants,
    setDatasetGrant,
    revokeDatasetGrant,
  } from '../lib/api.js';
  import { t } from 'svelte-i18n';
  import { Link } from '../lib/router/index.js';
  import { navigate } from '../lib/router/index.js';
  import { isAdmin, user as userStore } from '../lib/stores.js';
  import { VISIBILITIES } from '../lib/permissions.js';
  import { safeExternalUrl } from '../lib/safeUrl.js';
  import { copyToClipboard } from '../lib/clipboard.js';
  import { Plus, Trash2, X, UserPlus, Terminal, Database, Network, Rows3, Activity, Edit2, ShieldCheck, Loader2, Upload, Copy, CheckCheck, Users, Building2, Globe, Mail, Link as LinkIcon, ChevronRight, Info, Hash, Bookmark } from 'lucide-svelte';
  import ConfirmModal from '../components/ConfirmModal.svelte';
  import OrganisationMetadataDialog from '../components/OrganisationMetadataDialog.svelte';
  import Avatar from '../components/Avatar.svelte';
  import BannerBackdrop from '../components/BannerBackdrop.svelte';
  import PageHeader from '../components/PageHeader.svelte';
  import Select from '../components/Select.svelte';

  export let id;

  let org = null;
  let members = [];
  let groups = [];
  let orgDatasets = [];
  let allOrgs = [];
  let error = '';

  const ORG_TYPE_KEY = {
    FormalOrganization: 'pages.orgDetail.orgTypeFormal',
    OrganizationalUnit: 'pages.orgDetail.orgTypeUnit',
    Organization: 'pages.orgDetail.orgTypeOrganization',
  };
  $: orgTypeLabel = (type) => type && ORG_TYPE_KEY[type]
    ? $t(ORG_TYPE_KEY[type])
    : (type || $t('pages.orgDetail.orgTypeFormal'));
  function fmtDate(s) {
    if (!s) return '';
    const d = new Date(s);
    return isNaN(d.getTime()) ? s : d.toLocaleDateString(undefined, { year: 'numeric', month: 'short', day: 'numeric' });
  }
  // Hierarchy derived from the full org list.
  $: parentOrg = org?.parent_org_id ? allOrgs.find(o => o.id === org.parent_org_id) : null;
  $: childOrgs = org ? allOrgs.filter(o => o.parent_org_id === org.id) : [];
  $: hasAboutMeta = !!(org && (org.identifier || org.homepage || org.contact_name
    || org.contact_email || org.contact_url || org.parent_org_id));

  // Metadata dialog state
  let metadataDialogOpen = false;
  let savingMetadata = false;
  let metadataError = '';

  // Delete org state
  let deletingOrg = false;

  // Image / banner upload
  let uploadingImage = false;
  let imageKey = null;
  let imageVersion = 0;
  let uploadingBanner = false;
  let bannerKey = null;
  let bannerVersion = 0;

  // New dataset form
  let showNewDataset = false;
  let newDsName = '';
  let newDsDesc = '';
  let newDsVisibility = 'members';
  let creatingDs = false;

  // Add member form
  let newMemberUserId = '';
  let newMemberRole = 'member';
  let showInviteModal = false;
  let inviteError = '';
  let showAccessMatrix = false;

  // Editable access matrix: explicit per-dataset role grants keyed by
  // datasetId -> { "user:<id>" | "group:<id>" -> "viewer"|"editor"|"admin" }.
  let grantsByDataset = {};
  let loadingMatrix = false;
  let matrixError = '';
  let savingCell = '';   // "<datasetId>:<principalKey>" while a change is in flight

  async function toggleAccessMatrix() {
    showAccessMatrix = !showAccessMatrix;
    if (showAccessMatrix) await loadMatrixGrants();
  }

  async function loadMatrixGrants() {
    loadingMatrix = true;
    matrixError = '';
    try {
      const pairs = await Promise.all(
        orgDatasets.map(async ds => {
          try {
            const grants = await listDatasetGrants(ds.id);
            const map = {};
            for (const g of grants) map[`${g.principal_type}:${g.principal_id}`] = g.role;
            return [ds.id, map];
          } catch {
            // Caller may not manage every dataset; treat as empty.
            return [ds.id, {}];
          }
        })
      );
      grantsByDataset = Object.fromEntries(pairs);
    } catch (e) {
      matrixError = e.message;
    } finally {
      loadingMatrix = false;
    }
  }

  // The role a principal inherits from membership when no explicit grant exists,
  // shown as the "Default" hint. Mirrors the backend resolution.
  function inheritedRole(memberRole, visibility) {
    if (visibility === 'private' && memberRole !== 'admin') return null; // no access
    if (memberRole === 'admin') return 'admin';
    if (memberRole === 'member') return 'editor';
    return 'viewer';
  }

  async function changeGrant(datasetId, principalType, principalId, role) {
    const key = `${principalType}:${principalId}`;
    savingCell = `${datasetId}:${key}`;
    matrixError = '';
    try {
      if (role === '') {
        await revokeDatasetGrant(datasetId, principalType, principalId);
        const map = { ...(grantsByDataset[datasetId] || {}) };
        delete map[key];
        grantsByDataset = { ...grantsByDataset, [datasetId]: map };
      } else {
        await setDatasetGrant(datasetId, { principal_type: principalType, principal_id: principalId, role });
        grantsByDataset = {
          ...grantsByDataset,
          [datasetId]: { ...(grantsByDataset[datasetId] || {}), [key]: role },
        };
      }
    } catch (e) {
      matrixError = e.message;
    } finally {
      savingCell = '';
    }
  }

  // Create group modal
  let newGroupName = '';
  let showGroupModal = false;
  let creatingGroup = false;
  let groupError = '';

  // Manage group members modal
  let manageGroup = null;          // group object whose members are being managed
  let groupMemberUserId = '';      // selected user in the picker
  let groupMemberRole = 'member';
  let addingGroupMember = false;
  let manageError = '';
  let allUsers = [];               // {id, username, avatar_key} from listPublicUsers

  // Copy SPARQL endpoint URL
  let copiedSparql = false;
  async function copyOrgSparqlUrl() {
    const url = `${window.location.origin}/api/organisations/${id}/sparql`;
    if (await copyToClipboard(url)) {
      copiedSparql = true;
      setTimeout(() => { copiedSparql = false; }, 2000);
    }
  }

  onMount(async () => {
    await Promise.all([fetchOrg(), fetchMembers(), fetchGroups(), fetchOrgDatasets(), fetchAllOrgs()]);
  });

  async function fetchAllOrgs() {
    try {
      allOrgs = await listOrganisations();
    } catch (_) { /* non-admins may get a filtered list; ignore errors */ }
  }

  async function fetchOrgDatasets() {
    try {
      const all = await listDatasets();
      // Filter datasets owned by this organisation using the canonical owner fields
      orgDatasets = all.filter(d => d.owner_type === 'organisation' && String(d.owner_id) === String(id));
    } catch (_) { /* ignore */ }
  }

  async function fetchOrg() {
    try {
      org = await getOrganisation(id);
      imageKey = org.image_key;
      bannerKey = org.banner_key;
    } catch (e) {
      error = e.message;
    }
  }

  async function doUploadImage(file) {
    if (!file) return;
    uploadingImage = true;
    try {
      await uploadOrgImage(id, file);
      imageKey = true;
      imageVersion++;
    } catch (e) {
      metadataError = e.message;
    } finally {
      uploadingImage = false;
    }
  }

  async function doUploadBanner(file) {
    if (!file) return;
    uploadingBanner = true;
    try {
      const res = await uploadOrgBanner(id, file);
      bannerKey = res?.banner_key || true;
      bannerVersion++;
    } catch (e) {
      metadataError = e.message;
    } finally {
      uploadingBanner = false;
    }
  }

  async function doSetBannerPreset(preset) {
    uploadingBanner = true;
    metadataError = '';
    try {
      await setOrgBannerPreset(id, preset);
      bannerKey = `preset:${preset}`;
      bannerVersion++;
    } catch (e) {
      metadataError = e.message;
    } finally {
      uploadingBanner = false;
    }
  }

  async function doClearBanner() {
    uploadingBanner = true;
    metadataError = '';
    try {
      await clearOrgBanner(id);
      bannerKey = null;
      bannerVersion++;
    } catch (e) {
      metadataError = e.message;
    } finally {
      uploadingBanner = false;
    }
  }

  async function handleMetadataSave(e) {
    savingMetadata = true;
    metadataError = '';
    try {
      org = await updateOrganisation(id, e.detail);
      await fetchAllOrgs();
      metadataDialogOpen = false;
    } catch (err) {
      metadataError = err.message;
    } finally {
      savingMetadata = false;
    }
  }

  async function doDeleteOrg() {
    deletingOrg = true;
    try {
      await deleteOrganisation(id);
      navigate('/organisations');
    } catch (e) {
      metadataError = e.message;
    } finally {
      deletingOrg = false;
    }
  }

  async function createOrgDataset() {
    if (!newDsName) return;
    creatingDs = true;
    try {
      const ds = await createDataset({
        name: newDsName,
        description: newDsDesc || null,
        visibility: newDsVisibility,
        owner_type: 'organisation',
        owner_id: id,
      });
      newDsName = '';
      newDsDesc = '';
      showNewDataset = false;
      await fetchOrgDatasets();
      navigate(`/datasets/${ds.id}`);
    } catch (e) {
      error = e.message;
    } finally {
      creatingDs = false;
    }
  }

  async function fetchMembers() {
    try {
      members = await listOrgMembers(id);
    } catch (_) { /* ignore */ }
  }

  async function fetchGroups() {
    try {
      const raw = await listGroups(id);
      groups = await Promise.all(
        raw.map(async g => {
          try {
            const members = await listGroupMembers(id, g.id);
            return { ...g, members };
          } catch {
            return { ...g, members: [] };
          }
        })
      );
    } catch (_) { /* ignore */ }
  }

  async function handleAddMember() {
    if (!newMemberUserId) return;
    inviteError = '';
    try {
      await addOrgMember(id, { user_id: newMemberUserId, role: newMemberRole });
      newMemberUserId = '';
      newMemberRole = 'member';
      showInviteModal = false;
      await fetchMembers();
    } catch (e) {
      inviteError = e.message;
    }
  }

  async function handleChangeRole(userId, newRole) {
    try {
      await updateOrgMemberRole(id, userId, newRole);
      await fetchMembers();
    } catch (e) {
      error = e.message;
    }
  }

  async function handleRemoveMember(userId) {
    try {
      await removeOrgMember(id, userId);
      await fetchMembers();
    } catch (e) {
      error = e.message;
    }
  }

  function openGroupModal() {
    newGroupName = '';
    groupError = '';
    showGroupModal = true;
  }

  async function handleCreateGroup() {
    if (!newGroupName.trim()) return;
    creatingGroup = true;
    groupError = '';
    try {
      await createGroup(id, { name: newGroupName.trim() });
      newGroupName = '';
      showGroupModal = false;
      await fetchGroups();
    } catch (e) {
      groupError = e.message;
    } finally {
      creatingGroup = false;
    }
  }

  async function openManageGroup(g) {
    manageGroup = g;
    manageError = '';
    groupMemberUserId = '';
    groupMemberRole = 'member';
    if (allUsers.length === 0) {
      try { allUsers = await listPublicUsers(); } catch { /* ignore */ }
    }
    // Refresh this group's member list so the modal is current.
    try {
      manageGroup = { ...g, members: await listGroupMembers(id, g.id) };
    } catch { /* keep existing */ }
  }

  async function refreshManageGroup() {
    if (!manageGroup) return;
    const members = await listGroupMembers(id, manageGroup.id);
    manageGroup = { ...manageGroup, members };
    groups = groups.map(gr => gr.id === manageGroup.id ? { ...gr, members } : gr);
  }

  async function handleAddGroupMember() {
    if (!groupMemberUserId || !manageGroup) return;
    addingGroupMember = true;
    manageError = '';
    try {
      await addGroupMember(id, manageGroup.id, { user_id: groupMemberUserId, role: groupMemberRole });
      groupMemberUserId = '';
      groupMemberRole = 'member';
      await refreshManageGroup();
    } catch (e) {
      manageError = e.message;
    } finally {
      addingGroupMember = false;
    }
  }

  async function handleRemoveGroupMember(userId) {
    if (!manageGroup) return;
    manageError = '';
    try {
      await removeGroupMember(id, manageGroup.id, userId);
      await refreshManageGroup();
    } catch (e) {
      manageError = e.message;
    }
  }

  // Users not already in the group being managed — candidates for the add picker.
  $: groupCandidates = manageGroup
    ? allUsers.filter(u => !(manageGroup.members || []).some(m => (m.user_id || m.user?.id) === u.id))
    : [];

  let deleteGroupId = null;

  $: membersByRole = members.reduce((acc, m) => {
    const r = m.role || 'member';
    acc[r] = (acc[r] || 0) + 1;
    return acc;
  }, {});

  // True if the current user is a system admin OR an admin-role member of this org.
  $: canManageOrg = $isAdmin || members.some(
    m => (m.user_id || m.user?.id) === $userStore?.id && m.role === 'admin'
  );

  async function doDeleteGroup() {
    try {
      await deleteGroup(id, deleteGroupId);
      deleteGroupId = null;
      await fetchGroups();
    } catch (e) {
      deleteGroupId = null;
      error = e.message;
    }
  }
</script>

<div class="detail-stack">
<PageHeader breadcrumbs={[{ label: $t('pages.orgDetail.breadcrumbOrgs'), href: '/organisations' }, { label: org?.name || id }]} />

<div class="card">
  {#if org}
    <div class="org-cover">
      <BannerBackdrop bannerKey={bannerKey} imageUrl="{getOrgBannerUrl(id)}?v={bannerVersion}" seed={id} />
      <div class="org-header glass">
        <div class="org-info">
          <h2 class="org-title">{org.name}</h2>
          {#if org.description}<p class="meta org-desc">{org.description}</p>{/if}
        </div>
        <div class="org-actions">
        {#if canManageOrg}
          <button class="btn btn-sm btn-ghost" on:click={() => { metadataError = ''; metadataDialogOpen = true; }}>
            <Edit2 size={13} /> {$t('pages.orgDetail.editPage')}
          </button>
        {/if}
        <Link to="/organisations/{id}/sparql" class="btn btn-sm">
          <Terminal size={13} /> {$t('pages.orgDetail.openSparql')}
        </Link>
        <button class="btn btn-sm btn-ghost" title={$t('pages.orgDetail.copySparqlUrlTitle')} on:click={copyOrgSparqlUrl}>
          {#if copiedSparql}<CheckCheck size={13} /> {$t('system.copied')}{:else}<Copy size={13} /> {$t('pages.orgDetail.copyUrl')}{/if}
        </button>
      </div>
    </div>
    </div>
  {:else if error}
    <p class="error">{error}</p>
  {:else}
    <p>{$t('system.loading')}</p>
  {/if}
</div>

<!-- About / metadata -->
{#if org}
<div class="card about-card">
  <div class="explore-head">
    <Info size={15} />
    <h3>{$t('pages.orgDetail.about')}</h3>
  </div>

  <dl class="meta-grid">
    <div class="meta-item">
      <dt>{$t('pages.orgDetail.fieldType')}</dt>
      <dd><span class="md-pill"><Building2 size={11} /> {$t(ORG_TYPE_KEY[org.org_type] || 'pages.orgDetail.orgTypeFormal')}</span></dd>
    </div>

    {#if org.identifier}
      <div class="meta-item">
        <dt>{$t('pages.orgDetail.fieldIdentifier')}</dt>
        <dd><span class="md-mono"><Hash size={11} /> {org.identifier}</span></dd>
      </div>
    {/if}

    {#if org.homepage}
      <div class="meta-item">
        <dt>{$t('pages.orgDetail.fieldHomepage')}</dt>
        <dd><a href={safeExternalUrl(org.homepage)} target="_blank" rel="noopener" class="md-link"><Globe size={11} /> {org.homepage}</a></dd>
      </div>
    {/if}

    {#if org.contact_name || org.contact_email || org.contact_url}
      <div class="meta-item meta-wide">
        <dt>{$t('pages.orgDetail.fieldContactPoint')}</dt>
        <dd class="contact-dd">
          {#if org.contact_name}<span class="contact-name">{org.contact_name}</span>{/if}
          {#if org.contact_email}<a href="mailto:{org.contact_email}" class="md-link"><Mail size={11} /> {org.contact_email}</a>{/if}
          {#if org.contact_url}<a href={safeExternalUrl(org.contact_url)} target="_blank" rel="noopener" class="md-link"><LinkIcon size={11} /> {org.contact_url}</a>{/if}
        </dd>
      </div>
    {/if}

    {#if org.created_at}
      <div class="meta-item">
        <dt>{$t('pages.orgDetail.fieldCreated')}</dt>
        <dd>{fmtDate(org.created_at)}</dd>
      </div>
    {/if}
  </dl>

  {#if !hasAboutMeta && canManageOrg}
    <p class="about-empty">{$t('pages.orgDetail.aboutEmptyBefore')} <strong>{$t('pages.orgDetail.editPage')}</strong> {$t('pages.orgDetail.aboutEmptyAfter')}</p>
  {/if}
</div>

<!-- Organisation hierarchy -->
<div class="card">
  <div class="explore-head">
    <Network size={15} />
    <h3>{$t('pages.orgDetail.hierarchyHeading')}</h3>
  </div>

  {#if parentOrg}
    <div class="hier-block">
      <span class="hier-label">{$t('pages.orgDetail.hierPartOf')}</span>
      <Link to="/organisations/{parentOrg.id}" class="hier-chip hier-parent">
        <Building2 size={13} /> {parentOrg.name}
        {#if parentOrg.org_type}<span class="hier-type">{orgTypeLabel(parentOrg.org_type)}</span>{/if}
      </Link>
    </div>
  {/if}

  <div class="hier-block">
    <span class="hier-label">{$t('pages.orgDetail.hierThisOrg')}</span>
    <span class="hier-chip hier-self">
      <Building2 size={13} /> {org.name}
      <span class="hier-type">{$t(ORG_TYPE_KEY[org.org_type] || 'pages.orgDetail.orgTypeFormal')}</span>
    </span>
  </div>

  <div class="hier-block">
    <span class="hier-label">{$t('pages.orgDetail.hierSubUnits', { values: { count: childOrgs.length } })}</span>
    {#if childOrgs.length > 0}
      <div class="hier-children">
        {#each childOrgs as c (c.id)}
          <Link to="/organisations/{c.id}" class="hier-chip hier-child">
            <ChevronRight size={13} /> {c.name}
            {#if c.org_type}<span class="hier-type">{orgTypeLabel(c.org_type)}</span>{/if}
          </Link>
        {/each}
      </div>
    {:else}
      <span class="hier-empty">{$t('pages.orgDetail.hierNoSubUnits')} {#if canManageOrg}{$t('pages.orgDetail.hierNoSubUnitsManage')}{/if}</span>
    {/if}
  </div>
</div>
{/if}

<!-- Explore -->
<div class="card explore-card">
  <div class="explore-head">
    <Activity size={15} />
    <h3>{$t('pages.orgDetail.exploreHeading')}</h3>
  </div>
  <div class="explore-actions">
    <Link to="/browse?org={id}" class="action-tile">
      <Rows3 size={22} />
      <strong>{$t('pages.orgDetail.tileBrowseTitle')}</strong>
      <span>{$t('pages.orgDetail.tileBrowseDesc')}</span>
    </Link>
    <Link to="/browse?view=graph&org={id}" class="action-tile">
      <Network size={22} />
      <strong>{$t('pages.orgDetail.tileGraphTitle')}</strong>
      <span>{$t('pages.orgDetail.tileGraphDesc')}</span>
    </Link>
    <Link to="/organisations/{id}/sparql" class="action-tile">
      <Terminal size={22} />
      <strong>SPARQL</strong>
      <span>{$t('pages.orgDetail.tileSparqlDesc')}</span>
    </Link>
    <Link to="/organisations/{id}/api-services" class="action-tile">
      <Bookmark size={22} />
      <strong>{$t('pages.orgDetail.tileApiTitle')}</strong>
      <span>{$t('pages.orgDetail.tileApiDesc')}</span>
    </Link>
    <Link to="/validation?org={id}" class="action-tile">
      <ShieldCheck size={22} />
      <strong>{$t('pages.orgDetail.tileValidateTitle')}</strong>
      <span>{$t('pages.orgDetail.tileValidateDesc')}</span>
    </Link>
    <Link to="/import?org={id}" class="action-tile">
      <Upload size={22} />
      <strong>{$t('pages.orgDetail.tileImportTitle')}</strong>
      <span>{$t('pages.orgDetail.tileImportDesc')}</span>
    </Link>
  </div>
</div>

<!-- Datasets -->
<div class="card">
  <div class="explore-head">
    <Database size={15} />
    <h3>{$t('pages.orgDetail.datasetsHeading')}</h3>
    <button class="btn btn-sm" on:click={() => showNewDataset = !showNewDataset}>
      {#if showNewDataset}<X size={13} /> {$t('system.cancel')}{:else}<Plus size={13} /> {$t('pages.orgDetail.newDataset')}{/if}
    </button>
  </div>

  {#if showNewDataset}
    <div class="new-ds-form">
      <input bind:value={newDsName} placeholder={$t('pages.orgDetail.datasetNamePlaceholder')} />
      <input bind:value={newDsDesc} placeholder={$t('pages.orgDetail.datasetDescPlaceholder')} />
      <Select bind:value={newDsVisibility}
        options={VISIBILITIES.map(v => ({ value: v.value, label: v.label }))} />
      <button class="btn btn-sm" on:click={createOrgDataset} disabled={creatingDs || !newDsName}>
        {#if creatingDs}{$t('pages.orgDetail.creating')}{:else}{$t('system.create')}{/if}
      </button>
    </div>
  {/if}

  {#if orgDatasets.length > 0}
    <div class="ds-grid">
      {#each orgDatasets as ds}
        <Link to="/datasets/{ds.id}" class="ds-tile">
          <div class="ds-tile-top">
            <strong>{ds.name}</strong>
            <span class="vis vis-{ds.visibility}">{ds.visibility}</span>
          </div>
          {#if ds.description}
            <p>{ds.description}</p>
          {/if}
        </Link>
      {/each}
    </div>
  {:else}
    <p class="empty-ds">{$t('pages.orgDetail.noDatasetsBefore')} <Link to="/import">{$t('pages.orgDetail.dataImportLink')}</Link>.</p>
  {/if}
</div>

<!-- Members -->
<div class="card">
  <div class="members-header">
    <div>
      <h3>{$t('pages.orgDetail.members')}</h3>
      <!-- Role summary line -->
      <div class="members-summary">
        <span>{$t('pages.orgDetail.memberCount', { values: { count: members.length } })}</span>
        {#if membersByRole.admin}<span class="role-chip role-admin">{$t('pages.orgDetail.ownerCount', { values: { count: membersByRole.admin } })}</span>{/if}
        {#if membersByRole.member}<span class="role-chip role-member">{$t('pages.orgDetail.memberRoleCount', { values: { count: membersByRole.member } })}</span>{/if}
        {#if membersByRole.viewer}<span class="role-chip role-viewer">{$t('pages.orgDetail.viewerCount', { values: { count: membersByRole.viewer } })}</span>{/if}
      </div>
    </div>
    <div class="members-header-actions">
      <button class="btn btn-sm btn-ghost" on:click={toggleAccessMatrix} title={$t('pages.orgDetail.accessMatrixTitle')}>
        <Database size={13} /> {showAccessMatrix ? $t('pages.orgDetail.hideMatrix') : $t('pages.orgDetail.accessMatrix')}
      </button>
      <button class="btn btn-sm" on:click={() => { showInviteModal = true; inviteError = ''; }}>
        <UserPlus size={13} /> {$t('pages.orgDetail.addMember')}
      </button>
    </div>
  </div>

  <!-- Editable dataset access matrix -->
  {#if showAccessMatrix}
    <div class="matrix-wrap">
      <p class="matrix-caption">
        {$t('pages.orgDetail.matrixCaptionIntro')} <strong>{$t('pages.orgDetail.roleViewer')}</strong> {$t('pages.orgDetail.matrixCaptionViewer')}
        <strong>{$t('pages.orgDetail.roleEditor')}</strong> {$t('pages.orgDetail.matrixCaptionEditor')} <strong>{$t('pages.orgDetail.roleAdmin')}</strong> {$t('pages.orgDetail.matrixCaptionAdmin')}
        {$t('pages.orgDetail.matrixCaptionDefaultBefore')} <strong>{$t('pages.orgDetail.defaultLabel')}</strong> {$t('pages.orgDetail.matrixCaptionDefaultAfter')}
      </p>
      {#if matrixError}<p class="error">{matrixError}</p>{/if}
      {#if orgDatasets.length === 0}
        <p class="empty-ds">{$t('pages.orgDetail.matrixNoDatasets')}</p>
      {:else if loadingMatrix}
        <p class="meta"><Loader2 size={14} class="animate-spin" /> {$t('pages.orgDetail.loadingAccess')}</p>
      {:else}
        <div class="access-matrix">
          <table class="matrix-table">
            <thead>
              <tr>
                <th>{$t('pages.orgDetail.colPrincipal')}</th>
                <th>{$t('pages.orgDetail.colOrgRole')}</th>
                {#each orgDatasets as ds}
                  <th class="ds-col" title={ds.name}>
                    {ds.name.length > 14 ? ds.name.slice(0,14) + '…' : ds.name}
                    <span class="vis vis-{ds.visibility} ds-col-vis">{ds.visibility}</span>
                  </th>
                {/each}
              </tr>
            </thead>
            <tbody>
              {#each members as m}
                {@const pid = m.user_id || m.user?.id}
                <tr>
                  <td>
                    <span class="principal-cell">
                      <Avatar kind="user" id={pid} name={m.username || m.user?.username || m.user_id} hasImage={!!(m.avatar_key || m.user?.avatar_key)} size={18} />
                      {m.username || m.user?.username || m.user_id}
                    </span>
                  </td>
                  <td>
                    <span class="role-chip {m.role === 'admin' ? 'role-admin' : m.role === 'viewer' ? 'role-viewer' : 'role-member'}">
                      {m.role === 'admin' ? $t('pages.orgDetail.roleOwner') : m.role === 'viewer' ? $t('pages.orgDetail.roleViewer') : $t('pages.orgDetail.roleMember')}
                    </span>
                  </td>
                  {#each orgDatasets as ds}
                    {@const key = `user:${pid}`}
                    {@const grant = (grantsByDataset[ds.id] || {})[key]}
                    {@const inh = inheritedRole(m.role, ds.visibility)}
                    <td class="cell">
                      <Select
                        class="grant-select"
                        size="sm"
                        disabled={savingCell === `${ds.id}:${key}`}
                        value={grant || ''}
                        on:change={(e) => changeGrant(ds.id, 'user', pid, e.detail)}
                        options={[
                          { value: '', label: inh ? $t('pages.orgDetail.defaultWithRole', { values: { role: inh } }) : $t('pages.orgDetail.defaultNoAccess') },
                          { value: 'viewer', label: $t('pages.orgDetail.roleViewer') },
                          { value: 'editor', label: $t('pages.orgDetail.roleEditor') },
                          { value: 'admin', label: $t('pages.orgDetail.roleAdmin') },
                        ]} />
                    </td>
                  {/each}
                </tr>
              {/each}
              {#each groups as g}
                <tr class="group-row">
                  <td>
                    <span class="principal-cell">
                      <Avatar kind="group" id={g.id} name={g.name} size={18} />
                      {g.name}
                    </span>
                  </td>
                  <td><span class="role-chip role-group">{$t('pages.orgDetail.groupRoleChip')}</span></td>
                  {#each orgDatasets as ds}
                    {@const key = `group:${g.id}`}
                    {@const grant = (grantsByDataset[ds.id] || {})[key]}
                    <td class="cell">
                      <Select
                        class="grant-select"
                        size="sm"
                        disabled={savingCell === `${ds.id}:${key}`}
                        value={grant || ''}
                        on:change={(e) => changeGrant(ds.id, 'group', g.id, e.detail)}
                        options={[
                          { value: '', label: $t('pages.orgDetail.defaultNone') },
                          { value: 'viewer', label: $t('pages.orgDetail.roleViewer') },
                          { value: 'editor', label: $t('pages.orgDetail.roleEditor') },
                          { value: 'admin', label: $t('pages.orgDetail.roleAdmin') },
                        ]} />
                    </td>
                  {/each}
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
      {/if}
    </div>
  {/if}

  <table>
    <thead><tr><th>{$t('pages.orgDetail.username')}</th><th>{$t('pages.orgDetail.role')}</th><th></th></tr></thead>
    <tbody>
      {#each members as m}
        <tr>
          <td>
            <span style="display:inline-flex;align-items:center;gap:0.5rem;">
              <Avatar kind="user" id={m.user_id || m.user?.id} name={m.username || m.user?.username || m.user_id} hasImage={!!(m.avatar_key || m.user?.avatar_key)} size={22} />
              <span>{m.username || m.user?.username || m.user_id}</span>
              {#if m.user?.role === 'super_admin'}<span class="sa-badge" title={$t('pages.orgDetail.superAdminTitle')}>SA</span>{/if}
            </span>
          </td>
          <td>
            {#if m.user?.role !== 'super_admin'}
              <Select
                size="sm"
                value={m.role}
                on:change={(e) => handleChangeRole(m.user_id || m.user?.id, e.detail)}
                options={[
                  { value: 'admin', label: $t('pages.orgDetail.roleOwner') },
                  { value: 'member', label: $t('pages.orgDetail.roleMember') },
                  { value: 'viewer', label: $t('pages.orgDetail.roleViewer') },
                ]} />
            {:else}
              <span class="text-xs px-2 py-0.5 rounded-full font-semibold bg-amber-100 text-amber-800">{$t('pages.orgDetail.roleOwner')}</span>
            {/if}
          </td>
          <td>
            {#if m.user?.role !== 'super_admin'}
              <button class="btn btn-sm btn-danger" on:click={() => handleRemoveMember(m.user_id || m.user?.id)}><Trash2 size={14} /> {$t('pages.orgDetail.removeMember')}</button>
            {/if}
          </td>
        </tr>
      {/each}
    </tbody>
  </table>
</div>

<!-- Invite Member Modal -->
{#if showInviteModal}
  <div class="modal-overlay" on:click|self={() => showInviteModal = false} on:keydown={(e) => e.key === 'Escape' && (showInviteModal = false)} role="dialog" aria-modal="true" aria-label={$t('pages.orgDetail.inviteMemberAria')} tabindex="-1">
    <div class="modal-card">
      <div class="modal-header">
        <h3><UserPlus size={16} /> {$t('pages.orgDetail.inviteMember')}</h3>
        <button class="btn btn-sm btn-ghost" on:click={() => showInviteModal = false}><X size={14} /></button>
      </div>
      <div class="modal-body">
        <div class="form-group">
          <label for="invite-uid">{$t('pages.orgDetail.userId')}</label>
          <input id="invite-uid" bind:value={newMemberUserId} placeholder={$t('pages.orgDetail.enterUserId')} required />
        </div>
        <div class="form-group">
          <label for="invite-role">{$t('pages.orgDetail.role')}</label>
          <Select id="invite-role" bind:value={newMemberRole}
            options={[
              { value: 'admin', label: $t('pages.orgDetail.inviteRoleOwner') },
              { value: 'member', label: $t('pages.orgDetail.inviteRoleMember') },
              { value: 'viewer', label: $t('pages.orgDetail.inviteRoleViewer') },
            ]} />
          <span class="hint">
            {#if newMemberRole === 'admin'}{$t('pages.orgDetail.inviteHintOwner')}
            {:else if newMemberRole === 'member'}{$t('pages.orgDetail.inviteHintMember')}
            {:else}{$t('pages.orgDetail.inviteHintViewer')}{/if}
          </span>
        </div>
        {#if inviteError}<p class="error">{inviteError}</p>{/if}
      </div>
      <div class="modal-footer">
        <button class="btn" on:click={handleAddMember} disabled={!newMemberUserId}><UserPlus size={14} /> {$t('pages.orgDetail.addMemberButton')}</button>
        <button class="btn btn-ghost" on:click={() => showInviteModal = false}>{$t('system.cancel')}</button>
      </div>
    </div>
  </div>
{/if}

<!-- Groups -->
<div class="card">
  <div class="header">
    <h3>{$t('pages.orgDetail.groups')}</h3>
    {#if canManageOrg}
      <button class="btn btn-sm" on:click={openGroupModal}><Plus size={14} /> {$t('pages.orgDetail.addGroup')}</button>
    {/if}
  </div>

  <table>
    <thead><tr><th>{$t('pages.orgDetail.groupName')}</th><th>{$t('pages.orgDetail.members')}</th><th></th></tr></thead>
    <tbody>
      {#each groups as g}
        <tr>
          <td class="font-medium">{g.name}</td>
          <td class="text-sm text-[var(--ink-500)]">
            {#if g.members?.length > 0}
              {g.members.map(m => m.username || m.user?.username || m.user_id).join(', ')}
            {:else}
              <span class="italic text-[var(--ink-400)]">{$t('pages.orgDetail.noMembers')}</span>
            {/if}
          </td>
          <td class="group-row-actions">
            <Link to="/groups/{g.id}/api-services" class="btn btn-sm btn-ghost" title={$t('pages.orgDetail.apiServicesTitle')}><Bookmark size={14} /> {$t('pages.orgDetail.apiServices')}</Link>
            {#if canManageOrg}
              <button class="btn btn-sm btn-ghost" on:click={() => openManageGroup(g)}><Users size={14} /> {$t('pages.orgDetail.members')}</button>
              <button class="btn btn-sm btn-danger" on:click={() => deleteGroupId = g.id}><Trash2 size={14} /> {$t('pages.orgDetail.deleteGroup')}</button>
            {/if}
          </td>
        </tr>
      {/each}
      {#if groups.length === 0}
        <tr><td colspan="3">{$t('pages.orgDetail.noGroups')}</td></tr>
      {/if}
    </tbody>
  </table>
</div>
</div><!-- /detail-stack -->

{#if deleteGroupId !== null}
  <ConfirmModal
    title={$t('pages.orgDetail.deleteGroupConfirm')}
    message={$t('pages.orgDetail.deleteGroupMessage')}
    confirmLabel={$t('pages.orgDetail.deleteGroupButton')}
    on:confirm={doDeleteGroup}
    on:cancel={() => deleteGroupId = null}
  />
{/if}

<!-- Create Group Modal -->
{#if showGroupModal}
  <div class="modal-overlay" on:click|self={() => showGroupModal = false} on:keydown={(e) => e.key === 'Escape' && (showGroupModal = false)} role="dialog" aria-modal="true" aria-label={$t('pages.orgDetail.createGroupAria')} tabindex="-1">
    <div class="modal-card">
      <div class="modal-header">
        <h3><Plus size={16} /> {$t('pages.orgDetail.addGroup')}</h3>
        <button class="btn btn-sm btn-ghost" on:click={() => showGroupModal = false}><X size={14} /></button>
      </div>
      <div class="modal-body">
        <div class="form-group">
          <label for="group-name">{$t('pages.orgDetail.groupName')}</label>
          <input id="group-name" bind:value={newGroupName} placeholder={$t('pages.orgDetail.groupNamePlaceholder')}
            on:keydown={(e) => e.key === 'Enter' && handleCreateGroup()} />
        </div>
        {#if groupError}<p class="error">{groupError}</p>{/if}
      </div>
      <div class="modal-footer">
        <button class="btn" on:click={handleCreateGroup} disabled={creatingGroup || !newGroupName.trim()}>
          {#if creatingGroup}<Loader2 size={14} class="animate-spin" /> {$t('pages.orgDetail.creating')}{:else}<Plus size={14} /> {$t('pages.orgDetail.createGroup')}{/if}
        </button>
        <button class="btn btn-ghost" on:click={() => showGroupModal = false}>{$t('system.cancel')}</button>
      </div>
    </div>
  </div>
{/if}

<!-- Manage Group Members Modal -->
{#if manageGroup}
  <div class="modal-overlay" on:click|self={() => manageGroup = null} on:keydown={(e) => e.key === 'Escape' && (manageGroup = null)} role="dialog" aria-modal="true" aria-label={$t('pages.orgDetail.manageGroupAria')} tabindex="-1">
    <div class="modal-card">
      <div class="modal-header">
        <h3><Users size={16} /> {$t('pages.orgDetail.groupMembersTitle', { values: { name: manageGroup.name } })}</h3>
        <button class="btn btn-sm btn-ghost" on:click={() => manageGroup = null}><X size={14} /></button>
      </div>
      <div class="modal-body">
        {#if manageError}<p class="error">{manageError}</p>{/if}

        <!-- Current members -->
        <div class="gm-section-label">
          <span>{$t('pages.orgDetail.members')}</span>
          <span class="gm-count">{(manageGroup.members || []).length}</span>
        </div>
        <ul class="group-member-list">
          {#each (manageGroup.members || []) as m}
            <li class="group-member-item">
              <span class="group-member-name">
                <Avatar kind="user" id={m.user_id || m.user?.id} name={m.username || m.user?.username || m.user_id} hasImage={!!(m.avatar_key || m.user?.avatar_key)} size={26} />
                <span class="gm-name-text">{m.username || m.user?.username || m.user_id}</span>
                {#if m.role}<span class="member-role-chip">{m.role}</span>{/if}
              </span>
              <button class="btn btn-xs btn-ghost btn-danger" title={$t('pages.orgDetail.removeFromTeam')} on:click={() => handleRemoveGroupMember(m.user_id || m.user?.id)}>
                <Trash2 size={13} />
              </button>
            </li>
          {/each}
          {#if !(manageGroup.members || []).length}
            <li class="group-member-empty">{$t('pages.orgDetail.noTeamMembers')}</li>
          {/if}
        </ul>

        <!-- Add a person -->
        <div class="gm-add">
          <div class="gm-add-label"><UserPlus size={13} /> {$t('pages.orgDetail.addPersonToTeam')}</div>
          {#if groupCandidates.length > 0}
            <div class="gm-add-controls">
              <Select bind:value={groupMemberUserId} class="gm-picker"
                options={[
                  { value: '', label: $t('pages.orgDetail.selectPerson') },
                  ...groupCandidates.map(u => ({ value: u.id, label: u.username })),
                ]} />
              <Select bind:value={groupMemberRole} class="gm-role"
                options={[
                  { value: 'admin', label: $t('pages.orgDetail.roleAdmin') },
                  { value: 'member', label: $t('pages.orgDetail.roleMember') },
                  { value: 'viewer', label: $t('pages.orgDetail.roleViewer') },
                ]} />
              <button class="btn btn-sm" on:click={handleAddGroupMember} disabled={addingGroupMember || !groupMemberUserId}>
                {#if addingGroupMember}<Loader2 size={13} class="animate-spin" />{:else}<UserPlus size={13} />{/if} {$t('system.add')}
              </button>
            </div>
            <p class="gm-role-hint">
              {#if groupMemberRole === 'admin'}{$t('pages.orgDetail.teamHintAdmin')}
              {:else if groupMemberRole === 'member'}{$t('pages.orgDetail.teamHintMember')}
              {:else}{$t('pages.orgDetail.teamHintViewer')}{/if}
            </p>
          {:else}
            <p class="hint">{allUsers.length > 0 ? $t('pages.orgDetail.everyoneInTeam') : $t('pages.orgDetail.noPeopleToAdd')}</p>
          {/if}
        </div>
      </div>
      <div class="modal-footer">
        <button class="btn btn-ghost" on:click={() => manageGroup = null}>{$t('pages.orgDetail.done')}</button>
      </div>
    </div>
  </div>
{/if}

<OrganisationMetadataDialog
  open={metadataDialogOpen}
  organisation={org}
  organisations={allOrgs}
  saving={savingMetadata}
  error={metadataError}
  hasImage={!!imageKey}
  imageUrl={`${getOrgImageUrl(id)}?v=${imageVersion}`}
  uploadingImage={uploadingImage}
  bannerKey={bannerKey}
  bannerUrl={`${getOrgBannerUrl(id)}?v=${bannerVersion}`}
  uploadingBanner={uploadingBanner}
  deleting={deletingOrg}
  on:save={handleMetadataSave}
  on:uploadImage={(e) => doUploadImage(e.detail.file)}
  on:uploadBanner={(e) => doUploadBanner(e.detail.file)}
  on:selectBannerPreset={(e) => doSetBannerPreset(e.detail.preset)}
  on:clearBanner={() => doClearBanner()}
  on:delete={doDeleteOrg}
  on:close={() => { if (!savingMetadata && !deletingOrg) metadataDialogOpen = false; }}
/>

<style>
  .detail-stack {
    display: flex;
    flex-direction: column;
    gap: 1.5rem;
  }

  .breadcrumb {
    display: none;
  }

  .org-title {
    font-size: 1.6rem;
    font-weight: 700;
    margin: 0 0 0.35rem 0;
    color: #fff;
    line-height: 1.2;
    text-shadow: 0 1px 3px rgba(0, 0, 0, 0.35);
  }
  .org-desc {
    font-size: 0.93rem;
    color: rgba(255, 255, 255, 0.88);
    margin: 0;
    text-shadow: 0 1px 2px rgba(0, 0, 0, 0.3);
  }

  /* About / metadata */
  .meta-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(220px, 1fr));
    gap: 0.85rem 1.5rem;
    margin: 0;
  }
  .meta-item { display: flex; flex-direction: column; gap: 0.25rem; min-width: 0; }
  .meta-item.meta-wide { grid-column: 1 / -1; }
  .meta-item dt {
    font-size: 0.7rem; font-weight: 700; text-transform: uppercase;
    letter-spacing: 0.05em; color: var(--ink-400);
  }
  .meta-item dd {
    margin: 0; font-size: 0.9rem; color: var(--ink-800);
    display: flex; flex-wrap: wrap; align-items: center; gap: 0.35rem;
    min-width: 0; word-break: break-word;
  }
  .md-pill {
    display: inline-flex; align-items: center; gap: 0.3rem;
    background: var(--bg-subtle, #eef2f7); border: 1px solid var(--line-soft, #e2e8f0);
    border-radius: 6px; padding: 0.1rem 0.5rem; font-weight: 600; font-size: 0.84rem;
  }
  .md-mono { display: inline-flex; align-items: center; gap: 0.3rem; font-family: ui-monospace, monospace; font-size: 0.84rem; }
  .md-link { color: var(--brand-600, #2563eb); text-decoration: none; display: inline-flex; align-items: center; gap: 0.25rem; word-break: break-all; }
  .md-link:hover { text-decoration: underline; }
  .contact-dd { flex-direction: column; align-items: flex-start; gap: 0.2rem; }
  .contact-name { font-weight: 600; }
  .about-empty { margin: 0.6rem 0 0; font-size: 0.86rem; color: var(--ink-500); line-height: 1.5; }

  /* Hierarchy */
  .hier-block { display: flex; align-items: center; flex-wrap: wrap; gap: 0.5rem; padding: 0.4rem 0; }
  .hier-block + .hier-block { border-top: 1px dashed var(--line-soft, #e5e7eb); }
  .hier-label {
    font-size: 0.7rem; font-weight: 700; text-transform: uppercase; letter-spacing: 0.05em;
    color: var(--ink-400); min-width: 120px;
  }
  :global(.hier-chip) {
    display: inline-flex; align-items: center; gap: 0.35rem;
    padding: 0.25rem 0.6rem; border-radius: 999px; font-size: 0.85rem; font-weight: 600;
    border: 1px solid var(--line-soft, #e2e8f0); background: rgba(255,255,255,0.7);
    color: var(--ink-800); text-decoration: none;
  }
  :global(a.hier-chip:hover) { border-color: var(--brand-300, #93c5fd); box-shadow: 0 2px 8px rgba(0,0,0,0.06); }
  .hier-self { background: var(--brand-50, #eff6ff); border-color: var(--brand-200, #bfdbfe); }
  .hier-type {
    font-size: 0.66rem; font-weight: 600; text-transform: uppercase; letter-spacing: 0.04em;
    color: var(--ink-400); background: var(--bg-subtle, #f1f5f9); padding: 0.05rem 0.35rem; border-radius: 4px;
  }
  .hier-children { display: flex; flex-wrap: wrap; gap: 0.4rem; }
  .hier-empty { font-size: 0.85rem; color: var(--ink-400); font-style: italic; }

  h2 { margin-top: 0; }
  h3 { margin-top: 0; }
  .meta { color: #666; }
  .header { display: flex; justify-content: space-between; align-items: center; }

  /* Org cover: banner image (or a fallback gradient) behind an animated
     linked-data layer, with a liquid-glass header panel floating on top. */
  .org-cover {
    position: relative;
    isolation: isolate;
    overflow: hidden;
    border-radius: 14px;
    min-height: 188px;
    padding: 14px;
    display: flex;
    align-items: flex-end;
    border: 1px solid var(--line-soft);
    background: linear-gradient(135deg, #0f2a33 0%, #1e5663 55%, #2f7a8c 100%);
    margin-bottom: 1rem;
  }
  .org-header {
    position: relative;
    z-index: 1;
    width: min(640px, 100%);
    display: flex;
    justify-content: space-between;
    align-items: flex-end;
    gap: 1rem;
    flex-wrap: wrap;
    background: rgba(10, 24, 30, 0.46);
    backdrop-filter: blur(var(--glass-blur)) saturate(125%);
    -webkit-backdrop-filter: blur(var(--glass-blur)) saturate(125%);
    border: 1px solid rgba(255, 255, 255, 0.14);
    border-radius: 12px;
    padding: 0.85rem 1.05rem;
    box-shadow:
      inset 0 1px 0 rgba(255, 255, 255, 0.1),
      0 8px 24px rgba(0, 0, 0, 0.22);
  }
  .org-info { flex: 1; min-width: 0; }
  .org-actions { display: flex; gap: 0.4rem; flex-shrink: 0; align-items: flex-end; flex-wrap: wrap; }
  /* Buttons sit on the dark glass — lift their contrast. */
  .org-header :global(.btn-ghost) { color: rgba(255, 255, 255, 0.92); }
  .org-header :global(.btn-ghost:hover) { background: rgba(255, 255, 255, 0.14); }

  :global(.org-actions a.btn), :global(.org-actions .btn) {
    display: inline-flex;
    align-items: center;
    gap: 0.3rem;
    white-space: nowrap;
  }

  .new-ds-form {
    display: flex;
    gap: 0.5rem;
    margin-bottom: 1rem;
    flex-wrap: wrap;
  }
  .new-ds-form input { flex: 1; min-width: 140px; }
  :global(.new-ds-form .sel-trigger) { width: auto; }
  .empty-ds { color: #888; font-style: italic; margin: 0; }

  /* Group rows + member management modal */
  .group-row-actions { display: flex; gap: 0.4rem; justify-content: flex-end; }
  .gm-section-label {
    display: flex; align-items: center; gap: 0.4rem;
    font-size: 0.74rem; text-transform: uppercase; letter-spacing: 0.05em;
    font-weight: 700; color: var(--ink-500); margin: 0 0 0.5rem;
  }
  .gm-count {
    background: var(--bg-subtle, #f1f5f9); color: var(--ink-500);
    border-radius: 10px; padding: 0.02rem 0.4rem; font-size: 0.72rem;
  }
  .gm-name-text { font-weight: 600; }
  .gm-add {
    margin-top: 0.9rem; padding: 0.75rem;
    border: 1px solid var(--line-soft); border-radius: 10px;
    background: var(--bg-soft, #f8fafc);
  }
  .gm-add-label {
    display: flex; align-items: center; gap: 0.35rem;
    font-size: 0.82rem; font-weight: 600; color: var(--ink-700); margin-bottom: 0.55rem;
  }
  .gm-add-controls { display: flex; gap: 0.5rem; align-items: center; }
  .gm-picker { flex: 1; min-width: 0; }
  .gm-role { width: auto; flex-shrink: 0; }
  .gm-role-hint { font-size: 0.74rem; color: var(--ink-400); margin: 0.45rem 0 0; }
  .group-member-list { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 0.3rem; }
  .group-member-item {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.5rem;
    padding: 0.4rem 0.55rem;
    border: 1px solid var(--line-soft);
    border-radius: 8px;
    background: rgba(255,255,255,0.6);
  }
  .group-member-name { display: inline-flex; align-items: center; gap: 0.5rem; font-size: 0.88rem; min-width: 0; flex: 1; }
  /* Keep the icon-only remove button compact even at the mobile width where
     the global rule stretches .btn to full width. */
  .group-member-item .btn { width: auto; flex: 0 0 auto; }
  .member-role-chip {
    font-size: 0.68rem;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    padding: 0.1rem 0.4rem;
    border-radius: 10px;
    background: var(--bg-subtle, #f1f5f9);
    color: var(--ink-500);
  }
  .group-member-empty { color: var(--ink-400); font-style: italic; font-size: 0.85rem; padding: 0.4rem 0; }

  .explore-card { padding-bottom: 0.25rem; }

  .explore-head {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    margin-bottom: 0.9rem;
  }
  .explore-head h3 { margin: 0; flex: 1; }

  .ds-link {
    font-size: 0.85rem;
    font-weight: 600;
    color: var(--brand-600);
    text-decoration: none;
  }
  .ds-link:hover { text-decoration: underline; }

  .explore-actions {
    display: grid;
    grid-template-columns: repeat(4, minmax(0, 1fr));
    gap: 0.75rem;
    margin-bottom: 0.25rem;
  }

  :global(.action-tile) {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 0.3rem;
    padding: 1rem;
    border-radius: 16px;
    border: 1px solid var(--line-soft);
    background: linear-gradient(160deg, rgba(255,255,255,0.9), rgba(247,241,231,0.7));
    text-decoration: none;
    color: var(--ink-900);
    transition: transform 0.15s ease, box-shadow 0.15s ease, border-color 0.15s ease;
  }
  :global(.action-tile:hover) {
    transform: translateY(-2px);
    box-shadow: 0 4px 16px rgba(0,0,0,0.08);
    border-color: var(--brand-300);
  }
  :global(.action-tile strong) { font-size: 0.9rem; }
  :global(.action-tile span) {
    font-size: 0.78rem;
    color: var(--ink-500);
  }
  :global(.action-tile svg) {
    color: var(--brand-500);
    margin-bottom: 0.2rem;
  }

  .ds-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(200px, 1fr));
    gap: 0.75rem;
  }

  :global(.ds-tile) {
    display: flex;
    flex-direction: column;
    gap: 0.35rem;
    padding: 0.9rem 1rem;
    border-radius: 14px;
    border: 1px solid var(--line-soft);
    background: rgba(255,255,255,0.75);
    text-decoration: none;
    transition: transform 0.15s ease, box-shadow 0.15s ease;
  }
  :global(.ds-tile:hover) {
    transform: translateY(-2px);
    box-shadow: 0 4px 12px rgba(0,0,0,0.07);
  }
  .ds-tile-top {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.5rem;
  }
  :global(.ds-tile strong) { color: var(--ink-900); font-size: 0.9rem; }
  :global(.ds-tile p) {
    margin: 0;
    font-size: 0.78rem;
    color: var(--ink-500);
    line-height: 1.4;
  }

  .sa-badge { display: inline-block; padding: 0.1rem 0.35rem; background: #ede7f6; color: #4527a0; border-radius: 4px; font-size: 0.68rem; font-weight: 700; margin-left: 0.3rem; vertical-align: middle; }
  .vis { padding: 0.15rem 0.5rem; border-radius: 3px; font-size: 0.75rem; white-space: nowrap; }
  .vis-public { background: #d4edda; color: #155724; }
  .vis-members { background: #fff3cd; color: #856404; }
  .vis-private { background: #f8d7da; color: #721c24; }

  /* Members section */
  .members-header {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    gap: 0.5rem;
    margin-bottom: 0.75rem;
  }
  .members-header h3 { margin: 0 0 0.25rem; }
  .members-header-actions { display: flex; gap: 0.4rem; flex-shrink: 0; }
  .members-summary {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    font-size: 0.8rem;
    color: var(--ink-500);
  }
  .role-chip {
    padding: 0.1rem 0.45rem;
    border-radius: 10px;
    font-size: 0.72rem;
    font-weight: 600;
  }
  .role-admin  { background: #fef3c7; color: #92400e; }
  .role-member { background: #dbeafe; color: #1e40af; }
  .role-viewer { background: #f1f5f9; color: #475569; }

  .role-select {
    font-size: 0.8rem;
    padding: 0.2rem 0.4rem;
    border: 1px solid var(--line-soft);
    border-radius: 6px;
    background: white;
    cursor: pointer;
  }

  /* Dataset access matrix */
  .matrix-wrap { margin-bottom: 1rem; }
  .matrix-caption {
    font-size: 0.8rem;
    color: var(--ink-500);
    margin: 0 0 0.6rem;
    line-height: 1.5;
  }
  .access-matrix {
    overflow-x: auto;
    border: 1px solid var(--line-soft);
    border-radius: 8px;
  }
  .matrix-table {
    width: 100%;
    border-collapse: collapse;
    font-size: 0.8rem;
  }
  .matrix-table th, .matrix-table td {
    padding: 0.4rem 0.6rem;
    border-bottom: 1px solid var(--line-soft);
    text-align: left;
    white-space: nowrap;
  }
  .matrix-table thead { background: var(--bg-soft, #f8fafc); }
  .ds-col { max-width: 130px; font-size: 0.72rem; }
  .ds-col-vis { display: inline-block; margin-left: 0.3rem; font-size: 0.6rem; padding: 0.05rem 0.3rem; vertical-align: middle; }
  .principal-cell { display: inline-flex; align-items: center; gap: 0.4rem; }
  .matrix-table .group-row { background: rgba(99,102,241,0.04); }
  .role-group { background: #ede9fe; color: #5b21b6; }
  .cell { text-align: center; }
  .grant-select {
    font-size: 0.75rem;
    padding: 0.15rem 0.3rem;
    border: 1px solid var(--line-soft);
    border-radius: 6px;
    background: white;
    cursor: pointer;
    max-width: 130px;
  }
  .grant-select:disabled { opacity: 0.5; cursor: progress; }

  /* Invite modal */
  .modal-overlay {
    position: fixed;
    inset: 0;
    background: rgba(0,0,0,0.35);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 100;
  }
  .modal-card {
    background: white;
    border-radius: 16px;
    box-shadow: 0 20px 60px rgba(0,0,0,0.2);
    width: 420px;
    max-width: calc(100vw - 2rem);
  }
  .modal-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 1rem 1.25rem 0.75rem;
    border-bottom: 1px solid var(--line-soft);
  }
  .modal-header h3 { margin: 0; display: flex; align-items: center; gap: 0.5rem; font-size: 1rem; }
  .modal-body { padding: 1rem 1.25rem; display: flex; flex-direction: column; gap: 0.75rem; }
  .modal-footer { padding: 0.75rem 1.25rem 1rem; display: flex; gap: 0.5rem; justify-content: flex-end; border-top: 1px solid var(--line-soft); }
  .form-group { display: flex; flex-direction: column; gap: 0.25rem; }
  .form-group label { font-size: 0.82rem; font-weight: 600; color: var(--ink-700); }
  .hint { font-size: 0.75rem; color: var(--ink-400); margin-top: 0.1rem; }

  @media (max-width: 720px) {
    .explore-actions { grid-template-columns: repeat(2, minmax(0, 1fr)); }
    .members-header { flex-wrap: wrap; }
    .members-header-actions { flex-wrap: wrap; }
  }

  /* ---- Dark mode overrides (scoped rules out-specify global theme.css) ---- */
  :global(:is([data-theme="dark"], .dark)) .meta { color: var(--ink-600); }
  :global(:is([data-theme="dark"], .dark)) .empty-ds { color: var(--ink-500); }
  :global(:is([data-theme="dark"], .dark) .hier-chip) { background: rgba(255,255,255,0.06); }
  :global(:is([data-theme="dark"], .dark)) .hier-self { background: var(--brand-100); }
  :global(:is([data-theme="dark"], .dark)) .group-member-item { background: rgba(255,255,255,0.04); }
  :global(:is([data-theme="dark"], .dark) .action-tile) { background: linear-gradient(160deg, rgba(255,255,255,0.05), rgba(255,255,255,0.02)); }
  :global(:is([data-theme="dark"], .dark) .ds-tile) { background: rgba(255,255,255,0.04); }
  :global(:is([data-theme="dark"], .dark)) .sa-badge { background: rgba(139,92,246,0.2); color: #c4b5fd; }
  :global(:is([data-theme="dark"], .dark)) .vis-public { background: rgba(16,185,129,0.18); color: #6ee7b7; }
  :global(:is([data-theme="dark"], .dark)) .vis-members { background: rgba(245,158,11,0.18); color: #fcd34d; }
  :global(:is([data-theme="dark"], .dark)) .vis-private { background: rgba(239,68,68,0.18); color: #fca5a5; }
  :global(:is([data-theme="dark"], .dark)) .role-admin { background: rgba(245,158,11,0.18); color: #fcd34d; }
  :global(:is([data-theme="dark"], .dark)) .role-member { background: rgba(59,130,246,0.2); color: #93c5fd; }
  :global(:is([data-theme="dark"], .dark)) .role-viewer { background: rgba(255,255,255,0.06); color: var(--ink-500); }
  :global(:is([data-theme="dark"], .dark)) .role-group { background: rgba(139,92,246,0.2); color: #c4b5fd; }
  :global(:is([data-theme="dark"], .dark)) .role-select,
  :global(:is([data-theme="dark"], .dark)) .grant-select { background: var(--bg-soft); }
  :global(:is([data-theme="dark"], .dark)) .matrix-table .group-row { background: rgba(139,92,246,0.1); }
  :global(:is([data-theme="dark"], .dark)) .modal-card { background: var(--bg-strong); }
</style>
