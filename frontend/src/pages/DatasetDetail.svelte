<script>
  import { onMount } from 'svelte';
  import {
    getDataset,
    updateDataset,
    deleteDataset,
    listDatasetVersions,
    getDatasetVersionDataUrl,
    getOrganisation,
    listDatasetGraphs,
    listServices,
    addDatasetGraph,
    removeDatasetGraph,
    createService,
    updateService,
    deleteService,
    validateDataset,
    sparqlQuery,
    datasetSparqlQuery,
    uploadDatasetImage,
    getDatasetImageUrl,
    uploadDatasetBanner,
    setDatasetBannerPreset,
    clearDatasetBanner,
    getDatasetBannerUrl,
    listAssets,
    uploadAsset,
    deleteAsset,
    updateAssetVisibility,
    updateAssetMetadata,
    fetchAssetContent,
    uploadToGraph,
    listServiceGraphs,
    addServiceGraph,
    removeServiceGraph,
    updateGraphRole,
    setGraphPrivacy,
    listDatasetGrants,
    setDatasetGrant,
    revokeDatasetGrant,
    listOrgMembers,
    listGroups,
    listGroupMembers,
    listPublicUsers,
    listOrganisations,
    getDatasetEffectiveShapes,
    listBindingsForTarget,
    getLatestValidationRun,
    getValidationHistory,
    getGeoStats,
  } from '../lib/api.js';
  import { unwrapValidationRun, validationErrorMessage } from '../lib/validationReport.js';
  import { toastSuccess } from '../lib/toast.ts';
  import { RESOURCE_ROLES, RESOURCE_ROLE_RANK, VISIBILITY_LABEL } from '../lib/permissions.js';
  import { t as i18nT } from 'svelte-i18n';
  import { Link, navigate } from '../lib/router/index.js';
  import { isAuthenticated, user } from '../lib/stores.js';
  import { graphResultsToElements, detectRdfFormat, normalizeGraphRole, graphRoleLabel } from '../lib/rdf-utils.js';
  import { safeExternalUrl } from '../lib/safeUrl.js';
  import { copyToClipboard } from '../lib/clipboard.js';
  import GraphCanvas from '../components/GraphCanvas.svelte';
  import RdfTerm from '../components/RdfTerm.svelte';
  import ContextMenu from '../components/ContextMenu.svelte';
  import { Plus, Trash2, Check, X as XIcon, Loader2, ShieldCheck, LayoutGrid, Terminal, Network, Bookmark, Boxes, MapPin, Rows3, Activity, Copy, CheckCheck, Edit2, Power, Upload, FileText, Download, Link as LinkIcon, Clipboard, Globe, Lock, Eye, Database, Pencil, Info, Tag, ChevronLeft, ChevronRight, Unlink, Users, UserPlus, History } from 'lucide-svelte';
  import { Parser as N3Parser } from 'n3';
  import ConfirmModal from '../components/ConfirmModal.svelte';
  import AttachShapesDialog from '../components/AttachShapesDialog.svelte';
  import Avatar from '../components/Avatar.svelte';
  import AssetPreview from '../components/AssetPreview.svelte';
  import ContentKindWarning from '../components/ContentKindWarning.svelte';
  import DatasetMetadataDialog from '../components/DatasetMetadataDialog.svelte';
  import PageHeader from '../components/PageHeader.svelte';
  import BannerBackdrop from '../components/BannerBackdrop.svelte';
  import CommitHistory from '../components/CommitHistory.svelte';
  import DatasetVersions from '../components/DatasetVersions.svelte';
  import Select from '../components/Select.svelte';
  import { findLicense, LICENSE_CATEGORY_LABEL } from '../lib/vocab/licenses';
  import { findTheme, findAdmsStatus } from '../lib/vocab/themes';

  // Ontology registry removed — detection/linking no longer available

  export let id;

  let dataset = null;
  let graphs = [];
  let services = [];
  let error = '';
  // Geo capability of the dataset — gates the map / 3D-viewer action tile so it
  // only appears when there is something to show (coordinates and/or 3D data).
  let geoStats = null;
  let newGraphIri = '';

  // Per-graph role editing
  let editingGraphRoleIri = null;
  let updatingGraphRole = false;

  function graphIri(g) {
    return typeof g === 'string' ? g : g.graph_iri;
  }
  function graphRole(g) {
    return typeof g === 'string' ? null : (g.graph_role ?? null);
  }
  function graphPrivate(g) {
    return typeof g === 'string' ? false : !!g.private;
  }

  $: GRAPH_ROLE_OPTIONS = [
    { value: '', label: $i18nT('pages.datasetDetail.roleOptDefault') },
    { value: 'model', label: $i18nT('pages.datasetDetail.roleOptModel') },
    { value: 'vocabulary', label: $i18nT('pages.datasetDetail.roleOptVocabulary') },
    { value: 'shapes', label: $i18nT('pages.datasetDetail.roleOptShapes') },
    { value: 'entailment', label: $i18nT('pages.datasetDetail.roleOptEntailment') },
    { value: 'instances', label: $i18nT('pages.datasetDetail.roleOptInstances') },
  ];

  async function setGraphRole(g, role) {
    updatingGraphRole = true;
    const iri = graphIri(g);
    try {
      await updateGraphRole(id, iri, role || null);
      graphs = graphs.map(gr => graphIri(gr) === iri ? { ...(typeof gr === 'string' ? { graph_iri: gr } : gr), graph_role: role || null } : gr);
      editingGraphRoleIri = null;
      if (role === 'shapes') onShapesRoleAssigned();
    } catch (e) {
      error = e.message;
    } finally {
      updatingGraphRole = false;
    }
  }

  // A graph just got the 'shapes' role: the backend auto-registers it in the
  // SHACL Studio library, so refresh the effective-shapes panel and say so.
  function onShapesRoleAssigned() {
    shapesRoleNotice = true;
    toastSuccess($i18nT('pages.datasetDetail.shapesRoleSetToast'));
    void loadEffectiveShapes();
  }

  let updatingGraphPrivacy = null; // IRI currently being toggled

  async function toggleGraphPrivacy(g) {
    const iri = graphIri(g);
    const next = !graphPrivate(g);
    updatingGraphPrivacy = iri;
    try {
      await setGraphPrivacy(id, iri, next);
      graphs = graphs.map(gr => graphIri(gr) === iri ? { ...(typeof gr === 'string' ? { graph_iri: gr } : gr), private: next } : gr);
    } catch (e) {
      error = e.message;
    } finally {
      updatingGraphPrivacy = null;
    }
  }

  // ── Add Service modal ──────────────────────────────────────────────────────
  let showAddServiceModal = false;
  let newServiceName = '';
  let newServiceSlug = '';
  let newServiceDesc = '';
  let newServiceGraphs = new Set(); // graph IRIs to include in the new service
  let addingService = false;
  let addServiceError = '';

  // Auto-generate slug from name
  $: if (newServiceName && !newServiceSlugEdited) {
    newServiceSlug = newServiceName.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '');
  }
  let newServiceSlugEdited = false;

  function openAddServiceModal() {
    newServiceName = '';
    newServiceSlug = '';
    newServiceDesc = '';
    newServiceSlugEdited = false;
    newServiceGraphs = new Set(graphs.map(g => typeof g === 'string' ? g : g.graph_iri));
    addServiceError = '';
    showAddServiceModal = true;
  }

  // ── SHACL validation modal ─────────────────────────────────────────────────
  let showValidationModal = false;

  // ── Asset edit modal ───────────────────────────────────────────────────────
  let editingAsset = null; // the asset being edited
  let assetEditTitle = '';
  let assetEditDesc = '';
  let assetEditSaving = false;
  let assetEditError = '';

  function openAssetEdit(asset) {
    editingAsset = asset;
    assetEditTitle = asset.title || '';
    assetEditDesc = asset.description || '';
    assetEditError = '';
  }

  async function saveAssetMetadata() {
    if (!editingAsset) return;
    assetEditSaving = true;
    assetEditError = '';
    try {
      const updated = await updateAssetMetadata(id, editingAsset.id, {
        title: assetEditTitle || null,
        description: assetEditDesc || null,
      });
      assets = assets.map(a => a.id === updated.asset.id ? { ...updated.asset, iri: updated.iri } : a);
      editingAsset = null;
    } catch (e) {
      assetEditError = e.message || $i18nT('pages.datasetDetail.failedToSave');
    } finally {
      assetEditSaving = false;
    }
  }

  // Dataset metadata editing — handled by DatasetMetadataDialog
  let metadataDialogOpen = false;
  let savingDataset = false;
  let dialogError = '';

  // ── Role-based access management (users + groups) ──────────────────────────
  const ROLE_RANK = RESOURCE_ROLE_RANK;
  let grants = [];            // raw [{principal_type, principal_id, role}]
  let orgMembers = [];        // [{user_id, username, avatar_key, role}] for owning org
  let orgGroups = [];         // [{id, name, members:[...]}] for owning org
  let allOrgs = [];           // organisations the manager may grant access to
  let allUsers = [];          // public users, for the add-person picker
  let accessLoading = false;
  let accessError = '';
  let savingPrincipal = '';   // "user:<id>" | "group:<id>" | "organisation:<id>" while a change is in flight
  let addUserId = '';
  let addUserRole = 'viewer';
  let showAddPersonModal = false;

  function openAddPerson() {
    addUserId = '';
    addUserRole = 'viewer';
    accessError = '';
    showAddPersonModal = true;
  }

  // Map "type:id" -> role for explicit grants.
  $: grantMap = Object.fromEntries(grants.map(g => [`${g.principal_type}:${g.principal_id}`, g.role]));

  async function fetchAccess() {
    if (!dataset?.can_manage) { grants = []; return; }
    accessLoading = true;
    try {
      const tasks = [listDatasetGrants(id)];
      // Owning org context lets us list members & teams to assign.
      const orgId = dataset.owner_type === 'organisation' ? dataset.owner_id : null;
      if (orgId) {
        tasks.push(listOrgMembers(orgId), listGroups(orgId));
      }
      const [g, members, groupsRaw] = await Promise.all(tasks);
      grants = g || [];
      orgMembers = members || [];
      if (groupsRaw) {
        orgGroups = await Promise.all(groupsRaw.map(async grp => {
          try { return { ...grp, members: await listGroupMembers(dataset.owner_id, grp.id) }; }
          catch { return { ...grp, members: [] }; }
        }));
      } else {
        orgGroups = [];
      }
      if (allUsers.length === 0) {
        try { allUsers = await listPublicUsers(); } catch { /* ignore */ }
      }
      // Organisations the manager can grant access to (excludes the owning org,
      // whose members already inherit access through membership).
      try {
        const orgs = await listOrganisations();
        allOrgs = (orgs || []).filter(o => o.id !== dataset.owner_id);
      } catch { /* ignore — org grants simply won't be offered */ }
    } catch (e) {
      accessError = e.message || $i18nT('pages.datasetDetail.failedToLoadAccess');
    } finally {
      accessLoading = false;
    }
  }

  function strongest(roles) {
    return roles.filter(Boolean).sort((a, b) => ROLE_RANK[b] - ROLE_RANK[a])[0] || null;
  }

  // The role a member inherits from their org role when there is no explicit grant.
  function membershipDefault(orgRole) {
    if (!orgRole) return null;
    if (dataset.visibility === 'private' && orgRole !== 'admin') return null;
    if (orgRole === 'admin') return 'admin';
    if (orgRole === 'member') return 'editor';
    return 'viewer';
  }

  function groupHasMember(grp, userId) {
    return (grp.members || []).some(m => (m.user_id || m.user?.id) === userId);
  }

  // Resolve a user's effective access, mirroring the backend: an explicit grant
  // (direct or via a team) replaces the membership default; the strongest of all
  // grants wins; an org admin keeps a manage floor.
  function resolveUser(userId, orgRole) {
    const direct = grantMap[`user:${userId}`] || null;
    const viaTeams = orgGroups
      .filter(grp => grantMap[`group:${grp.id}`] && groupHasMember(grp, userId))
      .map(grp => ({ name: grp.name, role: grantMap[`group:${grp.id}`] }));
    const grantRoles = [direct, ...viaTeams.map(t => t.role)].filter(Boolean);
    const base = grantRoles.length ? strongest(grantRoles) : membershipDefault(orgRole);
    const floor = orgRole === 'admin' ? 'admin' : null;
    const effective = strongest([floor, base]);
    return { direct, viaTeams, effective, default: membershipDefault(orgRole) };
  }

  // Rows for the People list: every org member, plus any individually-granted
  // user who isn't an org member (e.g. a guest on a user-owned dataset).
  $: peopleRows = (() => {
    const rows = orgMembers.map(m => {
      const userId = m.user_id || m.user?.id;
      return { userId, username: m.username || m.user?.username || userId, avatar_key: m.avatar_key, orgRole: m.role, isOwner: false, ...resolveUser(userId, m.role) };
    });
    const known = new Set(rows.map(r => r.userId));
    // The owner of a user-owned dataset has full control but is neither an org
    // member nor an explicit grantee, so they would otherwise be missing from
    // this list. Add them explicitly as a non-editable owner row.
    if (dataset?.owner_type === 'user' && dataset.owner_id && !known.has(dataset.owner_id)) {
      const ownerId = dataset.owner_id;
      const u = allUsers.find(x => x.id === ownerId);
      const ownerName = u?.username || ($user?.id === ownerId ? $user.username : ownerId);
      rows.unshift({
        userId: ownerId, username: ownerName, avatar_key: u?.avatar_key ?? ($user?.id === ownerId ? $user.avatar_key : undefined),
        orgRole: null, isOwner: true, direct: null, viaTeams: [], effective: 'admin', default: null,
      });
      known.add(ownerId);
    }
    for (const g of grants.filter(x => x.principal_type === 'user' && !known.has(x.principal_id))) {
      const u = allUsers.find(x => x.id === g.principal_id);
      rows.push({ userId: g.principal_id, username: u?.username || g.principal_id, avatar_key: u?.avatar_key, orgRole: null, isOwner: false, ...resolveUser(g.principal_id, null) });
    }
    return rows;
  })();

  $: addCandidates = allUsers.filter(u => !peopleRows.some(r => r.userId === u.id));

  async function setUserGrant(userId, role) {
    savingPrincipal = `user:${userId}`;
    accessError = '';
    try {
      if (role === '') await revokeDatasetGrant(id, 'user', userId);
      else await setDatasetGrant(id, { principal_type: 'user', principal_id: userId, role });
      await fetchAccess();
    } catch (e) { accessError = e.message || $i18nT('pages.datasetDetail.failedToUpdateAccess'); }
    finally { savingPrincipal = ''; }
  }

  async function setGroupGrant(groupId, role) {
    savingPrincipal = `group:${groupId}`;
    accessError = '';
    try {
      if (role === '') await revokeDatasetGrant(id, 'group', groupId);
      else await setDatasetGrant(id, { principal_type: 'group', principal_id: groupId, role });
      await fetchAccess();
    } catch (e) { accessError = e.message || $i18nT('pages.datasetDetail.failedToUpdateAccess'); }
    finally { savingPrincipal = ''; }
  }

  async function setOrgGrant(orgId, role) {
    savingPrincipal = `organisation:${orgId}`;
    accessError = '';
    try {
      if (role === '') await revokeDatasetGrant(id, /** @type {any} */ ('organisation'), orgId);
      else await setDatasetGrant(id, { principal_type: /** @type {any} */ ('organisation'), principal_id: orgId, role });
      await fetchAccess();
    } catch (e) { accessError = e.message || $i18nT('pages.datasetDetail.failedToUpdateAccess'); }
    finally { savingPrincipal = ''; }
  }

  // Organisations to show in the grant list: those the manager can pick, plus
  // any org that already holds a grant (so it can be seen and revoked).
  $: orgGrantRows = (() => {
    const byId = new Map(allOrgs.map(o => [o.id, { id: o.id, name: o.name }]));
    for (const g of grants.filter(x => x.principal_type === 'organisation')) {
      if (!byId.has(g.principal_id)) byId.set(g.principal_id, { id: g.principal_id, name: g.principal_id });
    }
    return [...byId.values()].sort((a, b) => a.name.localeCompare(b.name));
  })();

  async function addPerson() {
    if (!addUserId) return;
    const uid = addUserId;
    const role = addUserRole;
    showAddPersonModal = false;
    addUserId = '';
    await setUserGrant(uid, role);
  }

  // Service editing state
  let editSvcId = null;
  let editSvcName = '';
  let editSvcDesc = '';
  let savingSvc = false;

  // Service graph management
  let expandedSvcId = null;   // which service has its graph panel open
  let svcGraphs = [];         // graph IRIs currently registered for expanded service
  let svcGraphsLoading = false;
  let svcGraphsSaving = false;

  // Image / banner upload state
  let uploadingImage = false;
  let imageKey = null;
  let imageVersion = 0; // bump to bust cache
  let uploadingBanner = false;
  let bannerKey = null;
  let bannerVersion = 0;

  // Delete state (driven from the Page settings dialog's danger zone)
  let deletingDataset = false;

  // Versions — for the picker shown next to the dataset name.
  let versions = [];
  let selectedVersion = '';      // '' = Live (current); otherwise a version string
  let viewingVersion = null;     // non-null when the preview shows a version snapshot
  let versionLoading = false;
  let versionError = '';

  // Ontology conformance
  let editConformsToOntology = '';
  let editConformsToVersion = '';

  // Write permission: set by backend based on ownership/membership
  $: canWrite = (dataset?.can_write ?? false) && $isAuthenticated;
  // Manage permission: who may edit settings and assign access (admin/owner).
  $: canManage = (dataset?.can_manage ?? false) && $isAuthenticated;

  // ── Derived metadata for the "About" panel ──────────────────────────────────
  function parseJsonList(s) {
    if (!s) return [];
    try { const v = JSON.parse(s); return Array.isArray(v) ? v : []; } catch { return []; }
  }
  // VISIBILITY_LABEL is imported from ../lib/permissions.js (single source of truth).
  // Canonical dataset-level role + its label, for the header / About badge.
  $: datasetRole = normalizeGraphRole(dataset?.graph_role);
  $: datasetRoleLabel = graphRoleLabel(dataset?.graph_role);
  $: ROLE_HINT = {
    model: $i18nT('pages.datasetDetail.roleHintModel'),
    vocabulary: $i18nT('pages.datasetDetail.roleHintVocabulary'),
    shapes: $i18nT('pages.datasetDetail.roleHintShapes'),
    entailment: $i18nT('pages.datasetDetail.roleHintEntailment'),
    instances: $i18nT('pages.datasetDetail.roleHintInstances'),
    system: $i18nT('pages.datasetDetail.roleHintSystem'),
  };
  function fmtDate(s) {
    if (!s) return '';
    const d = new Date(s);
    return isNaN(d.getTime()) ? s : d.toLocaleDateString(undefined, { year: 'numeric', month: 'short', day: 'numeric' });
  }
  $: mdLicense = dataset?.license ? (findLicense(dataset.license)) : null;
  $: mdThemes = parseJsonList(dataset?.themes);
  $: mdKeywords = parseJsonList(dataset?.keywords);
  $: mdAdms = dataset?.adms_status ? findAdmsStatus(dataset.adms_status) : null;
  $: hasContact = !!(dataset?.contact_name || dataset?.contact_email || dataset?.contact_url);
  // Whether there is any rich metadata worth showing beyond the basics.
  $: hasRichMetadata = !!(dataset && (dataset.license || mdThemes.length || mdKeywords.length
    || dataset.adms_status || dataset.version_notes || dataset.spatial || dataset.landing_page
    || hasContact || dataset.conforms_to_ontology));

  // Breadcrumb: org name when dataset is org-owned
  let ownerOrgName = null;
  $: if (dataset?.owner_type === 'organisation' && dataset?.owner_id) {
    getOrganisation(dataset.owner_id)
      .then(org => { ownerOrgName = org?.name ?? String(dataset.owner_id); })
      .catch(() => { ownerOrgName = String(dataset.owner_id); });
  }

  // Copy-to-clipboard state per service
  let copiedSlug = null;
  async function copyEndpoint(slug) {
    const url = `${window.location.origin}/api/datasets/${id}/services/${slug}/sparql`;
    if (await copyToClipboard(url)) {
      copiedSlug = slug;
      setTimeout(() => { copiedSlug = null; }, 2000);
    }
  }

  // Validation
  let validationReport = null;
  let validating = false;
  let shapesGraphIri = '';
  let validationError = '';
  let validationRanAt = null;
  let validationHistory = [];
  // Set after a graph's role becomes 'shapes': the backend auto-registers it
  // in the SHACL Studio library, so we surface a pointer to it.
  let shapesRoleNotice = false;

  function fmtDateTime(s) {
    if (!s) return '';
    const d = new Date(s);
    return isNaN(d.getTime()) ? s : d.toLocaleString();
  }

  // Last persisted run + a short history, so the section shows state on load.
  async function loadValidationState() {
    if (!$isAuthenticated) return;
    try {
      const latest = await getLatestValidationRun(id);
      if (latest) {
        const run = unwrapValidationRun(latest);
        if (run.report) {
          validationReport = run.report;
          validationRanAt = run.ranAt;
        }
      }
    } catch (_) { /* non-fatal — the card still allows running validation */ }
    try {
      validationHistory = (await getValidationHistory(id, 5)) || [];
    } catch (_) {
      validationHistory = [];
    }
  }

  // Validation layer: shape graphs that effectively apply (dataset-level bindings
  // ∪ shapes inherited from the dataset's graphs).
  let effectiveShapes = [];
  let datasetBoundIds = new Set();     // shape-graph ids bound at the dataset level
  let loadingEffective = false;
  let attachTarget = null;             // { kind, id, label } — drives the shared dialog

  async function loadEffectiveShapes() {
    if (!$isAuthenticated) return;
    loadingEffective = true;
    try {
      const [eff, dsBindings] = await Promise.all([
        getDatasetEffectiveShapes(id),
        listBindingsForTarget('dataset', id),
      ]);
      effectiveShapes = eff || [];
      datasetBoundIds = new Set((dsBindings?.shape_graphs || []).map((s) => s.id));
    } catch (_e) {
      // Non-fatal: the validation card still works without the effective list.
      effectiveShapes = [];
      datasetBoundIds = new Set();
    }
    loadingEffective = false;
  }

  // Linked data preview
  let graphNodes = [];
  let graphEdges = [];
  let sampleTriples = [];
  let loadingPreview = false;
  let previewError = '';

  // Expand/collapse state for linked data preview graph
  let previewExpandedUris = new Map();
  let previewExpandedDirs = new Map();
  let previewExpansionCache = new Map();
  let previewExpandingUri = null;
  $: previewExpandedIris = new Set(previewExpandedUris.keys());
  $: previewExhaustedIris = new Set(
    [...previewExpandedDirs.entries()]
      .filter(([, dirs]) => dirs.has('in') && dirs.has('out'))
      .map(([iri]) => iri)
  );

  // Context menu for preview graph
  let previewCtxVisible = false;
  let previewCtxX = 0, previewCtxY = 0;
  let previewCtxItems = [];
  let previewCtxNodeData = null;

  async function previewExpandUri(uri, direction = 'both') {
    if (!uri || uri.startsWith('_:') || (!uri.includes('://') && !uri.startsWith('urn:'))) return;
    previewExpandingUri = uri;
    try {
      const cacheKey = `${uri}::${direction}`;
      const applyElements = (newNodes, newEdges) => {
        const existingIds = new Set(graphNodes.map(n => n.data.id));
        const existingEdgeIds = new Set(graphEdges.map(e => e.data.id));
        const nodesToAdd = newNodes.filter(n => !existingIds.has(n.data.id));
        const edgesToAdd = newEdges.filter(e => !existingEdgeIds.has(e.data.id));
        graphNodes = [...graphNodes, ...nodesToAdd];
        graphEdges = [...graphEdges, ...edgesToAdd];
        const prev = previewExpandedUris.get(uri) || { nodeIds: new Set(), edgeIds: new Set() };
        previewExpandedUris = new Map(previewExpandedUris).set(uri, {
          nodeIds: new Set([...prev.nodeIds, ...nodesToAdd.map(n => n.data.id)]),
          edgeIds: new Set([...prev.edgeIds, ...edgesToAdd.map(e => e.data.id)]),
        });
        const dirs = new Set(previewExpandedDirs.get(uri) || []);
        if (direction === 'both') { dirs.add('in'); dirs.add('out'); } else dirs.add(direction);
        previewExpandedDirs = new Map(previewExpandedDirs).set(uri, dirs);
      };
      if (previewExpansionCache.has(cacheKey)) {
        const { nodes, edges } = previewExpansionCache.get(cacheKey);
        applyElements(nodes, edges);
        return;
      }
      const outPromise = (direction === 'both' || direction === 'out')
        ? sparqlQuery(`SELECT ?p ?o WHERE { <${uri}> ?p ?o } LIMIT 80`)
        : Promise.resolve(null);
      const inPromise  = (direction === 'both' || direction === 'in')
        ? sparqlQuery(`SELECT ?s ?p WHERE { ?s ?p <${uri}> } LIMIT 30`)
        : Promise.resolve(null);
      const [outRes, inRes] = await Promise.all([outPromise, inPromise]);
      const outBindings = [], inBindings = [];
      if (outRes) for (const row of (outRes?.results?.bindings || []))
        outBindings.push({ s: { type: 'uri', value: uri }, p: row.p, o: row.o });
      if (inRes) for (const row of (inRes?.results?.bindings || []))
        inBindings.push({ s: row.s, p: row.p, o: { type: 'uri', value: uri } });
      const { nodes: newNodes, edges: newEdges } = graphResultsToElements([...outBindings, ...inBindings]);
      previewExpansionCache = new Map(previewExpansionCache).set(cacheKey, { nodes: newNodes, edges: newEdges });
      applyElements(newNodes, newEdges);
    } catch {}
    finally { previewExpandingUri = null; }
  }

  function previewCollapseUri(uri) {
    const expanded = previewExpandedUris.get(uri);
    if (!expanded) return;
    const { nodeIds, edgeIds } = expanded;
    const otherNodeIds = new Set();
    const otherEdgeIds = new Set();
    for (const [otherUri, data] of previewExpandedUris) {
      if (otherUri === uri) continue;
      for (const nid of data.nodeIds) otherNodeIds.add(nid);
      for (const eid of data.edgeIds) otherEdgeIds.add(eid);
    }
    const removeNodes = new Set([...nodeIds].filter(nid => !otherNodeIds.has(nid)));
    const removeEdges = new Set([...edgeIds].filter(eid => !otherEdgeIds.has(eid)));
    graphNodes = graphNodes.filter(n => !removeNodes.has(n.data.id));
    const keptNodeIds = new Set(graphNodes.map(n => n.data.id));
    graphEdges = graphEdges.filter(e =>
      !removeEdges.has(e.data.id) &&
      keptNodeIds.has(e.data.source) &&
      keptNodeIds.has(e.data.target)
    );
    const next = new Map(previewExpandedUris); next.delete(uri);
    previewExpandedUris = next;
    const nextDirs = new Map(previewExpandedDirs); nextDirs.delete(uri);
    previewExpandedDirs = nextDirs;
  }

  function handlePreviewNodeExpand(e) {
    if (e.detail.fullIri) previewExpandUri(e.detail.fullIri);
  }

  function handlePreviewNodeContextMenu(e) {
    const { data, x, y } = e.detail;
    previewCtxNodeData = data;
    const items = [];
    if (data.nodeType === 'uri' && data.fullIri) {
      const expandedDirs = previewExpandedDirs.get(data.fullIri) || new Set();
      if (expandedDirs.size > 0) items.push({ label: $i18nT('pages.datasetDetail.ctxCollapse'), icon: Unlink, action: 'collapse' });
      if (!expandedDirs.has('out')) items.push({ label: $i18nT('pages.datasetDetail.ctxExpandOutgoing'), icon: ChevronRight, action: 'expandOut' });
      if (!expandedDirs.has('in'))  items.push({ label: $i18nT('pages.datasetDetail.ctxExpandIncoming'), icon: ChevronLeft, action: 'expandIn' });
      if (!expandedDirs.has('in') || !expandedDirs.has('out'))
        items.push({ label: $i18nT('pages.datasetDetail.ctxExpandBoth'), icon: Plus, action: 'expandBoth' });
      items.push({ divider: true });
      items.push({ label: $i18nT('pages.datasetDetail.ctxCopyIri'), icon: Copy, action: 'copyIri' });
    }
    previewCtxItems = items;
    previewCtxX = x; previewCtxY = y; previewCtxVisible = true;
  }

  function handlePreviewCtxAction(e) {
    const action = e.detail;
    if (previewCtxNodeData) {
      const data = previewCtxNodeData;
      if      (action === 'expandOut')  previewExpandUri(data.fullIri, 'out');
      else if (action === 'expandIn')   previewExpandUri(data.fullIri, 'in');
      else if (action === 'expandBoth') previewExpandUri(data.fullIri, 'both');
      else if (action === 'collapse')   previewCollapseUri(data.fullIri);
      else if (action === 'copyIri')    void copyToClipboard(data.fullIri);
    }
  }

  // RDF data upload state
  let showRdfUpload = false;
  let uploadRdfFile = null;
  let uploadRdfGraph = '';
  let uploadRdfReplace = false;
  let uploadingRdf = false;
  let uploadRdfError = '';
  let uploadRdfSuccess = '';

  async function handleRdfUpload() {
    if (!uploadRdfFile || !uploadRdfGraph) return;
    uploadingRdf = true; uploadRdfError = ''; uploadRdfSuccess = '';
    try {
      const fmt = detectRdfFormat(uploadRdfFile.name) || 'text/turtle';
      const text = await uploadRdfFile.text();
      await uploadToGraph(uploadRdfGraph, text, fmt, uploadRdfReplace);
      uploadRdfSuccess = $i18nT('pages.datasetDetail.uploadedToGraph', { values: { file: uploadRdfFile.name, graph: uploadRdfGraph } });
      uploadRdfFile = null;
      showRdfUpload = false;
      await fetchGraphs();
    } catch (e) {
      uploadRdfError = e.message || $i18nT('pages.datasetDetail.uploadFailed');
    } finally {
      uploadingRdf = false;
    }
  }

  // Assets state
  let assets = [];
  let uploading = false;
  let uploadProgress = 0;
  let assetsError = '';
  let copiedAssetId = null;
  let previewAsset = null; // asset being previewed

  // Linked data IRI for an asset — used for copy/turtle, not for fetching file content.
  function assetIri(asset) {
    return asset.iri || `${window.location.origin}/datasets/${id}/assets/${asset.id}`;
  }

  // Programmatic download via the authenticated API endpoint.
  async function downloadAssetFile(asset) {
    try {
      const res = await fetchAssetContent(id, asset.id);
      if (!res.ok) { assetsError = $i18nT('pages.datasetDetail.downloadFailedHttp', { values: { status: res.status } }); return; }
      const blob = await res.blob();
      const url = URL.createObjectURL(blob);
      const a = document.createElement('a');
      a.href = url;
      a.download = asset.filename;
      document.body.appendChild(a);
      a.click();
      document.body.removeChild(a);
      setTimeout(() => URL.revokeObjectURL(url), 10000);
    } catch (e) {
      assetsError = e.message || $i18nT('pages.datasetDetail.downloadFailed');
    }
  }

  // Determine preview category from content_type or filename extension
  function previewKind(asset) {
    const ct = (asset.content_type || '').split(';')[0].trim().toLowerCase();
    const ext = asset.filename.split('.').pop().toLowerCase();
    if (ct.startsWith('image/') || ['jpg','jpeg','png','gif','webp','svg','bmp','ico'].includes(ext)) return 'image';
    if (ct === 'application/pdf' || ext === 'pdf') return 'pdf';
    if (ct.startsWith('audio/') || ['mp3','ogg','wav','flac','m4a','aac'].includes(ext)) return 'audio';
    if (ct.startsWith('video/') || ['mp4','webm','ogv','mov','avi'].includes(ext)) return 'video';
    if (ct === 'text/markdown' || ['md','markdown'].includes(ext)) return 'markdown';
    if (ct.startsWith('text/') || ['txt','json','yaml','yml','toml','xml','csv','tsv','html','css','js','ts','py','rs','sh','sql'].includes(ext)) return 'text';
    return null;
  }

  onMount(async () => {
    // fetchAccess depends on the loaded dataset (owner + can_manage), so run it after.
    await Promise.all([fetchDataset(), fetchGraphs(), fetchServices(), fetchAssets(), fetchVersions()]);
    await Promise.all([fetchAccess(), loadDataPreview(), loadEffectiveShapes(), loadValidationState()]);
  });

  // Probe geo capability (independent of the dataset load) to gate the viewer tile.
  async function fetchGeoStats() {
    try { geoStats = await getGeoStats(id); } catch { geoStats = null; }
  }
  fetchGeoStats();

  async function fetchDataset() {
    try {
      dataset = await getDataset(id);
      editConformsToOntology = dataset.conforms_to_ontology || '';
      editConformsToVersion = dataset.conforms_to_version || '';
      imageKey = dataset.image_key;
      bannerKey = dataset.banner_key;
    } catch (e) {
      error = e.message;
    }
  }

  async function fetchVersions() {
    try {
      versions = await listDatasetVersions(id) || [];
    } catch { versions = []; }
  }

  // Versions sorted latest-first, for the picker shown next to the name.
  function verParts(v) {
    return String(v?.version ?? '').replace(/^v/i, '').split('.').map(n => parseInt(n, 10) || 0);
  }
  $: pickerVersions = [...versions].sort((a, b) => {
    const pa = verParts(a), pb = verParts(b);
    for (let i = 0; i < Math.max(pa.length, pb.length); i++) {
      const x = pa[i] ?? 0, y = pb[i] ?? 0;
      if (x !== y) return y - x;
    }
    return 0;
  });

  // Map an N3.js term to the SPARQL-results JSON shape used by RdfTerm / graphResultsToElements.
  function n3TermToJson(t) {
    if (!t) return null;
    if (t.termType === 'BlankNode') return { type: 'bnode', value: t.value };
    if (t.termType === 'Literal') {
      const o = { type: 'literal', value: t.value };
      if (t.language) o['xml:lang'] = t.language;
      else if (t.datatype?.value && t.datatype.value !== 'http://www.w3.org/2001/XMLSchema#string') o.datatype = t.datatype.value;
      return o;
    }
    return { type: 'uri', value: t.value };
  }

  function onPickVersion(e) {
    const ver = e.detail;
    if (ver) loadVersionData(ver);
    else clearVersionView();
  }

  // Fetch a version's snapshot (TriG), parse it client-side and show it in the
  // preview + sample-triples sections, replacing the live data.
  async function loadVersionData(ver) {
    viewingVersion = ver;
    versionLoading = true;
    versionError = '';
    try {
      const res = await fetch(getDatasetVersionDataUrl(id, ver, 'trig'), {
        credentials: 'include',
        headers: { Accept: 'application/trig' },
      });
      if (!res.ok) throw new Error($i18nT('pages.datasetDetail.couldNotLoadVersion', { values: { version: ver, status: res.status } }));
      const text = await res.text();
      const quads = new N3Parser({ format: 'application/trig' }).parse(text);
      const bindings = quads.map(q => ({
        s: n3TermToJson(q.subject), p: n3TermToJson(q.predicate),
        o: n3TermToJson(q.object), g: n3TermToJson(q.graph),
      }));
      const { nodes, edges } = graphResultsToElements(bindings.slice(0, 80));
      graphNodes = nodes;
      graphEdges = edges;
      sampleTriples = bindings.slice(0, 10);
      // Reset graph-preview expansion state carried over from live data.
      previewExpandedUris = new Map();
      previewExpandedDirs = new Map();
    } catch (e) {
      versionError = e.message || $i18nT('pages.datasetDetail.failedToLoadVersion');
      graphNodes = []; graphEdges = []; sampleTriples = [];
    } finally {
      versionLoading = false;
    }
  }

  function clearVersionView() {
    viewingVersion = null;
    versionError = '';
    selectedVersion = '';
    loadDataPreview();
  }

  // When a real version is selected, explore tiles carry it via ?version= so the
  // SPARQL editor / browser / named graphs open scoped to that snapshot.
  $: versionParam = (selectedVersion && !['live', 'latest', 'current'].includes(selectedVersion)) ? selectedVersion : '';

  async function handleMetadataSave(e) {
    savingDataset = true;
    dialogError = '';
    try {
      dataset = await updateDataset(id, {
        ...e.detail,
        conforms_to_ontology: editConformsToOntology || null,
        conforms_to_version: editConformsToVersion || null,
      });
      metadataDialogOpen = false;
    } catch (err) {
      dialogError = err.message || $i18nT('pages.datasetDetail.failedToSaveMetadata');
    } finally {
      savingDataset = false;
    }
  }

  async function doUploadImage(file) {
    if (!file) return;
    uploadingImage = true;
    try {
      await uploadDatasetImage(id, file);
      imageKey = true; // mark as set
      imageVersion++;
    } catch (e) {
      dialogError = e.message;
    } finally {
      uploadingImage = false;
    }
  }

  async function doUploadBanner(file) {
    if (!file) return;
    uploadingBanner = true;
    try {
      const res = await uploadDatasetBanner(id, file);
      bannerKey = res?.banner_key || true;
      bannerVersion++;
    } catch (e) {
      dialogError = e.message;
    } finally {
      uploadingBanner = false;
    }
  }

  async function doSetBannerPreset(preset) {
    uploadingBanner = true;
    dialogError = '';
    try {
      await setDatasetBannerPreset(id, preset);
      bannerKey = `preset:${preset}`;
      bannerVersion++;
    } catch (e) {
      dialogError = e.message;
    } finally {
      uploadingBanner = false;
    }
  }

  async function doClearBanner() {
    uploadingBanner = true;
    dialogError = '';
    try {
      await clearDatasetBanner(id);
      bannerKey = null;
      bannerVersion++;
    } catch (e) {
      dialogError = e.message;
    } finally {
      uploadingBanner = false;
    }
  }

  async function handleDeleteDataset() {
    deletingDataset = true;
    dialogError = '';
    try {
      await deleteDataset(id);
      navigate(dataset?.owner_type === 'organisation'
        ? `/organisations/${dataset.owner_id}`
        : '/datasets');
    } catch (e) {
      dialogError = e.message || $i18nT('pages.datasetDetail.failedToDeleteDataset');
    } finally {
      deletingDataset = false;
    }
  }

  function openEditSvc(svc) {
    editSvcId = svc.id;
    editSvcName = svc.name;
    editSvcDesc = svc.description || '';
  }

  async function saveService() {
    savingSvc = true;
    try {
      await updateService(id, editSvcId, { name: editSvcName, description: editSvcDesc || null });
      editSvcId = null;
      await fetchServices();
    } catch (e) {
      error = e.message;
    } finally {
      savingSvc = false;
    }
  }

  async function toggleService(svc) {
    try {
      await updateService(id, svc.id, { is_active: !svc.is_active });
      await fetchServices();
    } catch (e) {
      error = e.message;
    }
  }

  let deleteServiceTarget = null;

  async function doDeleteService() {
    const svc = deleteServiceTarget;
    deleteServiceTarget = null;
    try {
      await deleteService(id, svc.id);
      await fetchServices();
    } catch (e) {
      error = e.message;
    }
  }

  async function toggleSvcGraphPanel(svc) {
    if (expandedSvcId === svc.id) {
      expandedSvcId = null;
      svcGraphs = [];
      return;
    }
    expandedSvcId = svc.id;
    svcGraphsLoading = true;
    try {
      svcGraphs = await listServiceGraphs(id, svc.id);
    } catch {
      svcGraphs = [];
    } finally {
      svcGraphsLoading = false;
    }
  }

  async function toggleGraphInService(graphIri, isCurrentlyRegistered) {
    svcGraphsSaving = true;
    try {
      if (isCurrentlyRegistered) {
        await removeServiceGraph(id, expandedSvcId, { graph_iri: graphIri });
      } else {
        await addServiceGraph(id, expandedSvcId, { graph_iri: graphIri });
      }
      svcGraphs = await listServiceGraphs(id, expandedSvcId);
    } catch (e) {
      error = e.message;
    } finally {
      svcGraphsSaving = false;
    }
  }

  async function bulkSetAllServiceGraphs(addAll) {
    svcGraphsSaving = true;
    try {
      for (const g of graphs) {
        const iri = typeof g === 'string' ? g : g.graph_iri;
        const isRegistered = svcGraphs.includes(iri);
        if (addAll && !isRegistered) {
          await addServiceGraph(id, expandedSvcId, { graph_iri: iri });
        } else if (!addAll && isRegistered) {
          await removeServiceGraph(id, expandedSvcId, { graph_iri: iri });
        }
      }
      svcGraphs = await listServiceGraphs(id, expandedSvcId);
    } catch (e) {
      error = e.message;
    } finally {
      svcGraphsSaving = false;
    }
  }

  async function fetchGraphs() {
    try {
      graphs = await listDatasetGraphs(id);
    } catch (_) { /* ignore */ }
  }

  async function loadDataPreview() {
    if (graphs.length === 0) return;
    loadingPreview = true;
    previewError = '';
    try {
      // Use the dataset-scoped SPARQL service so results are not filtered by
      // the accessible_graphs_cache (which may be stale right after upload).
      const sparql = `SELECT ?s ?p ?o ?g WHERE { GRAPH ?g { ?s ?p ?o } } LIMIT 80`;
      const firstSvcSlug = services[0]?.slug ?? null;
      const res = (firstSvcSlug
          ? await datasetSparqlQuery(id, firstSvcSlug, sparql).catch(() => null)
          : null)
        // Fall back to global SPARQL if the dataset service doesn't exist yet.
        || await sparqlQuery(`SELECT ?s ?p ?o WHERE { VALUES ?g { ${graphs.slice(0, 8).map(g => `<${typeof g === 'string' ? g : g.graph_iri}>`).join(' ')} } GRAPH ?g { ?s ?p ?o } } LIMIT 80`);
      const bindings = res?.results?.bindings || [];
      const { nodes, edges } = graphResultsToElements(bindings);
      graphNodes = nodes;
      graphEdges = edges;
      sampleTriples = bindings.slice(0, 10);
    } catch (e) {
      previewError = e.message;
    } finally {
      loadingPreview = false;
    }
  }

  async function fetchServices() {
    try {
      services = await listServices(id);
    } catch (_) { /* ignore */ }
  }

  async function addGraph() {
    if (!newGraphIri) return;
    try {
      await addDatasetGraph(id, { graph_iri: newGraphIri });
      newGraphIri = '';
      await fetchGraphs();
    } catch (e) {
      error = e.message;
    }
  }

  async function removeGraph(g) {
    const iri = graphIri(g);
    try {
      await removeDatasetGraph(id, { graph_iri: iri });
      await fetchGraphs();
    } catch (e) {
      error = e.message;
    }
  }

  async function addService() {
    if (!newServiceName || !newServiceSlug) return;
    addingService = true;
    addServiceError = '';
    try {
      const svc = await createService(id, {
        name: newServiceName,
        slug: newServiceSlug,
        description: newServiceDesc || null,
      });
      // Add the selected graphs to the new service
      for (const iri of newServiceGraphs) {
        try { await addServiceGraph(id, svc.id, { graph_iri: iri }); } catch {}
      }
      showAddServiceModal = false;
      await fetchServices();
    } catch (e) {
      addServiceError = e.message || $i18nT('pages.datasetDetail.failedToCreateService');
    } finally {
      addingService = false;
    }
  }

  async function fetchAssets() {
    try {
      assets = await listAssets(id);
    } catch (_) { /* ignore if user lacks access */ }
  }

  async function handleFileUpload(event) {
    const file = event.target.files?.[0];
    if (!file) return;
    uploading = true;
    uploadProgress = 0;
    assetsError = '';
    try {
      const asset = await uploadAsset(id, file, (p) => { uploadProgress = p; });
      assets = [...assets, asset];
      assetsError = '';
    } catch (e) {
      const msg = e.message || '';
      if (msg.toLowerCase().includes('service unavailable') || e.status === 503) {
        assetsError = $i18nT('pages.datasetDetail.fileStorageNotConfigured');
      } else if (msg.toLowerCase().includes('forbidden') || e.status === 403) {
        assetsError = $i18nT('pages.datasetDetail.noWriteAccess');
      } else {
        assetsError = msg || $i18nT('pages.datasetDetail.uploadFailedDot');
      }
    } finally {
      uploading = false;
      uploadProgress = 0;
      event.target.value = '';
    }
  }

  let deleteAssetId = null;

  async function doDeleteAsset() {
    try {
      await deleteAsset(id, deleteAssetId);
      assets = assets.filter(a => a.id !== deleteAssetId);
      assetsError = '';
      deleteAssetId = null;
    } catch (e) {
      const msg = e.message || '';
      if (msg.toLowerCase().includes('forbidden') || e.status === 403) {
        assetsError = $i18nT('pages.datasetDetail.noDeleteAssetPermission');
      } else {
        assetsError = msg || $i18nT('pages.datasetDetail.failedToDeleteAsset');
      }
      deleteAssetId = null;
    }
  }

  async function copyAssetIri(asset) {
    if (await copyToClipboard(assetIri(asset))) {
      copiedAssetId = asset.id;
      setTimeout(() => { copiedAssetId = null; }, 2000);
    }
  }

  function copyAssetTurtle(asset) {
    const iri = assetIri(asset);
    const title = asset.filename.replace(/\\/g, '\\\\').replace(/"/g, '\\"');
    const turtle = `<${iri}> a <http://www.w3.org/ns/dcat#Distribution> ;\n    <http://purl.org/dc/terms/title> "${title}" ;\n    <http://www.w3.org/ns/dcat#mediaType> "${asset.content_type}" ;\n    <http://www.w3.org/ns/dcat#downloadURL> <${iri}> .`;
    void copyToClipboard(turtle);
  }

  async function toggleAssetVisibility(asset) {
    const makingPublic = !asset.public;
    // Pre-flight: dataset must be public before an asset can be made public
    if (makingPublic && dataset?.visibility !== 'public') {
      const vis = dataset?.visibility === 'private' ? 'private' : 'members-only';
      assetsError = $i18nT('pages.datasetDetail.cannotMakeAssetPublic', { values: { visibility: vis } });
      return;
    }
    try {
      await updateAssetVisibility(id, asset.id, makingPublic);
      assets = assets.map(a => a.id === asset.id ? { ...a, public: makingPublic } : a);
      assetsError = '';
    } catch (e) {
      // Map cryptic HTTP errors to friendly messages
      const msg = e.message || '';
      if (msg.toLowerCase().includes('method not allowed') || e.status === 405) {
        assetsError = $i18nT('pages.datasetDetail.actionNotAllowed');
      } else if (msg.toLowerCase().includes('forbidden') || e.status === 403) {
        assetsError = $i18nT('pages.datasetDetail.noVisibilityPermission');
      } else if (msg.toLowerCase().includes('unauthorized') || e.status === 401) {
        assetsError = $i18nT('pages.datasetDetail.mustBeLoggedInVisibility');
      } else {
        assetsError = msg || $i18nT('pages.datasetDetail.failedToUpdateVisibility');
      }
    }
  }

  function formatBytes(bytes) {
    if (bytes < 1024) return bytes + ' B';
    if (bytes < 1048576) return (bytes / 1024).toFixed(1) + ' KB';
    return (bytes / 1048576).toFixed(1) + ' MB';
  }

  async function runValidation() {
    validating = true;
    validationError = '';
    try {
      // The endpoint returns an envelope { report, run_id, ran_at } — unwrap it.
      const res = await validateDataset(id, {
        shapes_graph: shapesGraphIri || null,
      });
      const run = unwrapValidationRun(res);
      validationReport = run.report;
      validationRanAt = run.ranAt;
      // The run may have resolved shapes that weren't listed yet (e.g. freshly
      // auto-registered imported shapes) — refresh the panel and the history.
      await Promise.all([
        loadEffectiveShapes(),
        getValidationHistory(id, 5).then((h) => { validationHistory = h || []; }).catch(() => {}),
      ]);
    } catch (e) {
      validationError = validationErrorMessage(e, $i18nT('pages.datasetDetail.validationFailed'));
    } finally {
      validating = false;
    }
  }
</script>

<div class="detail-stack">
<PageHeader breadcrumbs={dataset?.owner_type === 'organisation'
  ? [{ label: $i18nT('pages.organisations.title'), href: '/organisations' }, { label: ownerOrgName ?? '…', href: '/organisations/' + dataset.owner_id }, { label: dataset?.name || id }]
  : [{ label: $i18nT('pages.datasets.title'), href: '/datasets' }, { label: dataset?.name || id }]
} />

<div class="card">
  {#if dataset}
    <div class="ds-cover">
      <BannerBackdrop bannerKey={bannerKey} imageUrl="{getDatasetBannerUrl(id)}?v={bannerVersion}" seed={id} />
      <div class="ds-hero glass">
        <div class="ds-hero-main">
          {#if imageKey}
            <img
              src="{getDatasetImageUrl(id)}?v={imageVersion}"
              alt={$i18nT('pages.datasetDetail.coverAlt')}
              class="ds-image"
              on:error={e => { /** @type {HTMLElement} */ (e.currentTarget).style.display='none'; }}
            />
          {/if}
          <div class="ds-hero-text">
            <div class="ds-title-row">
              <h2 class="ds-title">{dataset.name}</h2>
              {#if pickerVersions.length}
                <Select
                  class="version-picker{viewingVersion ? ' viewing' : ''}"
                  size="sm"
                  bind:value={selectedVersion}
                  on:change={onPickVersion}
                  title={$i18nT('pages.datasetDetail.versionPickerTitle')}
                  options={[
                    { value: '', label: $i18nT('pages.datasetDetail.liveCurrent') },
                    ...pickerVersions.map(v => ({ value: v.version, label: `v${v.version}${v.status && v.status !== 'published' ? ` · ${v.status}` : ''}` })),
                  ]}
                />
              {/if}
            </div>
            <p class="ds-hero-meta">
              <span class="vis vis-{dataset.visibility}">{dataset.visibility}</span>
              {#if datasetRole}
                <span class="graph-role-badge role-{datasetRole}" title={ROLE_HINT[datasetRole] || ''}><Tag size={11} /> {datasetRoleLabel}</span>
              {/if}
            </p>
            {#if dataset.description}<p class="ds-hero-desc">{dataset.description}</p>{/if}
          </div>
        </div>
        <div class="ds-hero-actions">
          {#if canWrite}
            <button class="btn btn-sm btn-ghost" on:click={() => { dialogError = ''; metadataDialogOpen = true; }}>
              <Edit2 size={13} /> {$i18nT('pages.datasetDetail.editPage')}
            </button>
          {/if}
          <Link to="/datasets/{id}/sparql" class="btn btn-sm">
            <Terminal size={13} /> {$i18nT('pages.datasetDetail.openSparql')}
          </Link>
        </div>
      </div>
    </div>
  {:else if error}
    <p class="error">{error}</p>
  {:else}
    <p>{$i18nT('system.loading')}</p>
  {/if}
</div>

<!-- About / metadata -->
{#if dataset}
<div class="card about-card">
  <div class="section-head">
    <div class="section-head-left">
      <Info size={15} />
      <h3>{$i18nT('pages.datasetDetail.aboutThisDataset')}</h3>
    </div>
  </div>

  {#if dataset.description}
    <p class="about-desc">{dataset.description}</p>
  {/if}

  <dl class="meta-grid">
    <div class="meta-item">
      <dt>{$i18nT('pages.datasets.visibility')}</dt>
      <dd><span class="vis vis-{dataset.visibility}">{VISIBILITY_LABEL[dataset.visibility] || dataset.visibility}</span></dd>
    </div>

    {#if datasetRole}
      <div class="meta-item">
        <dt>{$i18nT('pages.datasetDetail.role')}</dt>
        <dd>
          <span class="graph-role-badge role-{datasetRole}"><Tag size={11} /> {datasetRoleLabel}</span>
          <span class="md-sub">{ROLE_HINT[datasetRole] || ''}</span>
        </dd>
      </div>
    {/if}

    {#if dataset.license}
      <div class="meta-item">
        <dt>{$i18nT('pages.datasetDetail.license')}</dt>
        <dd>
          {#if mdLicense}
            <span class="md-pill">{mdLicense.label}</span>
            <span class="md-sub">{LICENSE_CATEGORY_LABEL[mdLicense.category]}</span>
            {#if mdLicense.url}<a class="md-ext" href={mdLicense.url} target="_blank" rel="noopener"><LinkIcon size={11} /></a>{/if}
          {:else}
            <a href={safeExternalUrl(dataset.license)} target="_blank" rel="noopener" class="md-link">{dataset.license}</a>
          {/if}
        </dd>
      </div>
    {/if}

    {#if mdAdms || dataset.adms_status}
      <div class="meta-item">
        <dt>{$i18nT('pages.datasetDetail.status')}</dt>
        <dd>
          <span class="md-status">{mdAdms ? mdAdms.label : dataset.adms_status}</span>
          {#if mdAdms}<span class="md-sub">{mdAdms.summary}</span>{/if}
        </dd>
      </div>
    {/if}

    {#if dataset.version_notes}
      <div class="meta-item">
        <dt>{$i18nT('pages.datasetDetail.versionNotes')}</dt>
        <dd>{dataset.version_notes}</dd>
      </div>
    {/if}

    {#if mdThemes.length}
      <div class="meta-item meta-wide">
        <dt>{$i18nT('pages.datasetDetail.themes')}</dt>
        <dd class="chips-row">
          {#each mdThemes as iri}
            {@const th = findTheme(iri)}
            <span class="md-chip" title={th?.summary || iri}><Tag size={10} /> {th ? th.label : iri}</span>
          {/each}
        </dd>
      </div>
    {/if}

    {#if mdKeywords.length}
      <div class="meta-item meta-wide">
        <dt>{$i18nT('pages.datasetDetail.keywords')}</dt>
        <dd class="chips-row">
          {#each mdKeywords as kw}<span class="md-chip">{kw}</span>{/each}
        </dd>
      </div>
    {/if}

    {#if dataset.spatial}
      <div class="meta-item">
        <dt>{$i18nT('pages.datasetDetail.spatialCoverage')}</dt>
        <dd><a href={safeExternalUrl(dataset.spatial)} target="_blank" rel="noopener" class="md-link">{dataset.spatial}</a></dd>
      </div>
    {/if}

    {#if dataset.landing_page}
      <div class="meta-item">
        <dt>{$i18nT('pages.datasetDetail.landingPage')}</dt>
        <dd><a href={safeExternalUrl(dataset.landing_page)} target="_blank" rel="noopener" class="md-link"><Globe size={11} /> {dataset.landing_page}</a></dd>
      </div>
    {/if}

    {#if dataset.conforms_to_ontology}
      <div class="meta-item">
        <dt>{$i18nT('pages.datasetDetail.conformsToModel')}</dt>
        <dd><a href="/models/{dataset.conforms_to_ontology}" class="md-link">{dataset.conforms_to_ontology}{#if dataset.conforms_to_version} · v{dataset.conforms_to_version}{/if}</a></dd>
      </div>
    {/if}

    {#if hasContact}
      <div class="meta-item meta-wide">
        <dt>{$i18nT('pages.datasetDetail.contactPoint')}</dt>
        <dd class="contact-dd">
          {#if dataset.contact_name}<span class="contact-name">{dataset.contact_name}</span>{/if}
          {#if dataset.contact_email}<a href="mailto:{dataset.contact_email}" class="md-link">{dataset.contact_email}</a>{/if}
          {#if dataset.contact_url}<a href={safeExternalUrl(dataset.contact_url)} target="_blank" rel="noopener" class="md-link">{dataset.contact_url}</a>{/if}
        </dd>
      </div>
    {/if}

    {#if dataset.created_at}
      <div class="meta-item">
        <dt>{$i18nT('pages.datasetDetail.created')}</dt>
        <dd>{fmtDate(dataset.created_at)}</dd>
      </div>
    {/if}
    {#if dataset.updated_at}
      <div class="meta-item">
        <dt>{$i18nT('pages.datasetDetail.lastUpdated')}</dt>
        <dd>{fmtDate(dataset.updated_at)}</dd>
      </div>
    {/if}
  </dl>

  {#if !hasRichMetadata}
    <p class="about-empty">
      {$i18nT('pages.datasetDetail.noDescriptiveMetadata')}
      {#if canWrite}{$i18nT('pages.datasetDetail.noDescriptiveMetadataHint1')}<strong>{$i18nT('pages.datasetDetail.editPage')}</strong>{$i18nT('pages.datasetDetail.noDescriptiveMetadataHint2')}{/if}
    </p>
  {/if}
</div>
{/if}

<!-- Content kind warning: flags ontology/model data inside a dataset -->
{#if graphs.length > 0}
  {#key graphs.map(g => graphIri(g)).join('|')}
    <ContentKindWarning
      graphs={graphs.map(g => graphIri(g))}
      expected="dataset"
      contextName={dataset?.name}
      datasetId={id}
      declaredRole={dataset?.graph_role ?? null}
      onresolved={(e) => {
        dataset = { ...dataset, graph_role: e.role };
        if (e.role === 'shapes') onShapesRoleAssigned();
      }}
    />
  {/key}
{/if}

{#if shapesRoleNotice}
  <div class="card shapes-studio-note">
    <ShieldCheck size={15} />
    <span>{$i18nT('pages.datasetDetail.shapesAutoRegisteredNotice')}</span>
    <Link to="/shacl/shapes" class="btn btn-sm btn-ghost shapes-studio-link">{$i18nT('pages.datasetDetail.openShaclStudio')}</Link>
  </div>
{/if}

<!-- Explore & Visualize -->
<div class="card explore-card">
  <div class="explore-head">
    <Activity size={15} />
    <h3>{$i18nT('pages.datasetDetail.explore')}</h3>
  </div>
  <div class="explore-actions">
    <Link
      to={`/browse?dataset=${id}${versionParam ? `&version=${versionParam}` : ''}`}
      class="action-tile"
    >
      <Rows3 size={22} />
      <strong>{$i18nT('pages.datasetDetail.browseTriples')}</strong>
      <span>{$i18nT('pages.datasetDetail.browseTriplesDesc')}</span>
    </Link>
    <Link
      to={`/browse?view=graph&dataset=${id}${versionParam ? `&version=${versionParam}` : ''}`}
      class="action-tile"
    >
      <Network size={22} />
      <strong>{$i18nT('pages.datasetDetail.graphExplorer')}</strong>
      <span>{$i18nT('pages.datasetDetail.graphExplorerDesc')}</span>
    </Link>
    <Link to={`/datasets/${id}/sparql${versionParam ? `?version=${versionParam}` : ''}`} class="action-tile">
      <Terminal size={22} />
      <strong>{$i18nT('pages.datasetDetail.sparql')}</strong>
      <span>{$i18nT('pages.datasetDetail.sparqlDesc')}</span>
    </Link>
    {#if geoStats && (geoStats.has_coordinates || geoStats.has_3d)}
      <!-- Gated by data capability: 3D viewer when there's 3D data, otherwise a
           plain map for coordinate-only features. -->
      <Link to="/datasets/{id}/viewer" class="action-tile">
        {#if geoStats.has_3d}
          <Boxes size={22} />
          <strong>{$i18nT('pages.datasetDetail.viewer3d')}</strong>
          <span>{$i18nT('pages.datasetDetail.viewer3dDesc')}</span>
        {:else}
          <MapPin size={22} />
          <strong>{$i18nT('pages.datasetDetail.viewerMap')}</strong>
          <span>{$i18nT('pages.datasetDetail.viewerMapDesc')}</span>
        {/if}
      </Link>
    {/if}
    <Link to="/datasets/{id}/api-services" class="action-tile">
      <Bookmark size={22} />
      <strong>{$i18nT('pages.datasetDetail.apiServices')}</strong>
      <span>{$i18nT('pages.datasetDetail.apiServicesDesc')}</span>
    </Link>
    <Link to={`/graphs?dataset=${id}${versionParam ? `&version=${versionParam}` : ''}`} class="action-tile">
      <LayoutGrid size={22} />
      <strong>{$i18nT('pages.datasetDetail.namedGraphs')}</strong>
      <span>{$i18nT('pages.datasetDetail.namedGraphsDesc')}</span>
    </Link>
    <Link to="/validation?dataset={id}" class="action-tile">
      <ShieldCheck size={22} />
      <strong>{$i18nT('pages.datasetDetail.validate')}</strong>
      <span>{$i18nT('pages.datasetDetail.validateDesc')}</span>
    </Link>
  </div>
</div>

{#if viewingVersion}
  <div class="card version-view-banner">
    <span class="vvb-text">
      <History size={15} />
      {$i18nT('pages.datasetDetail.showingSnapshotPre')}<strong>v{viewingVersion}</strong>{$i18nT('pages.datasetDetail.showingSnapshotPost')}
    </span>
    <button class="btn btn-sm btn-ghost" on:click={clearVersionView}>{$i18nT('pages.datasetDetail.backToLive')}</button>
  </div>
{/if}

{#if loadingPreview || versionLoading || graphNodes.length > 0 || previewError || versionError}
  <div class="card">
    <div class="explore-head">
      <Network size={15} />
      <h3>{viewingVersion ? $i18nT('pages.datasetDetail.linkedDataGraphVersion', { values: { version: viewingVersion } }) : $i18nT('pages.datasetDetail.linkedDataGraph')}</h3>
    </div>
    {#if loadingPreview || versionLoading}
      <div class="preview-loading"><Loader2 size={18} class="animate-spin" /> {$i18nT('system.loading')}</div>
    {:else if previewError || versionError}
      <p class="error">{previewError || versionError}</p>
    {:else}
      <GraphCanvas
        nodes={graphNodes}
        edges={graphEdges}
        height="400px"
        expandedNodes={previewExpandedIris}
        expandingNode={previewExpandingUri}
        exhaustedNodes={previewExhaustedIris}
        on:nodeExpand={handlePreviewNodeExpand}
        on:nodeOpen={(e) => e.detail.fullIri && navigate(`/resource?iri=${encodeURIComponent(e.detail.fullIri)}`)}
        on:nodeContextMenu={handlePreviewNodeContextMenu}
      />
      <ContextMenu
        visible={previewCtxVisible}
        x={previewCtxX}
        y={previewCtxY}
        items={previewCtxItems}
        on:action={handlePreviewCtxAction}
        on:close={() => previewCtxVisible = false}
      />
    {/if}
  </div>
{/if}

{#if sampleTriples.length > 0}
  <div class="card">
    <div class="explore-head">
      <Rows3 size={15} />
      <h3>{$i18nT('pages.datasetDetail.sampleTriples')}</h3>
    </div>
    <div class="table-scroll">
      <table>
        <thead>
          <tr>
            <th>{$i18nT('pages.tripleBrowser.subject')}</th>
            <th>{$i18nT('pages.tripleBrowser.predicate')}</th>
            <th>{$i18nT('pages.tripleBrowser.object')}</th>
          </tr>
        </thead>
        <tbody>
          {#each sampleTriples as row}
            <tr>
              <td><RdfTerm term={row.s} /></td>
              <td><RdfTerm term={row.p} navigable={false} /></td>
              <td><RdfTerm term={row.o} /></td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  </div>
{/if}

<!-- Graphs -->
<div class="card">
  <div class="graphs-head">
    <h3>{$i18nT('pages.datasetDetail.graphs')}</h3>
    {#if canWrite}
    <button class="btn btn-sm btn-ghost" on:click={() => { showRdfUpload = !showRdfUpload; uploadRdfGraph = graphs.length ? graphIri(graphs[0]) : ''; uploadRdfError = ''; uploadRdfSuccess = ''; }}>
      <Upload size={13} /> {$i18nT('pages.datasetDetail.uploadRdf')}
    </button>
    {/if}
  </div>

  {#if uploadRdfSuccess}
    <div class="upload-success">{uploadRdfSuccess}</div>
  {/if}

  {#if canWrite && showRdfUpload}
    <div class="rdf-upload-form">
      <div class="upload-row">
        <label class="file-pick-btn btn btn-sm" class:btn-loading={uploadingRdf}>
          <Upload size={13} />
          {uploadRdfFile ? uploadRdfFile.name : $i18nT('pages.datasetDetail.chooseRdfFile')}
          <input type="file" accept=".ttl,.n3,.nt,.nq,.trig,.jsonld,.rdf,.owl,.xml" style="display:none"
            on:change={(e) => { uploadRdfFile = e.currentTarget.files?.[0] || null; if (!uploadRdfGraph && graphs.length) uploadRdfGraph = graphIri(graphs[0]); }} />
        </label>
        {#if graphs.length > 0}
          <Select bind:value={uploadRdfGraph} class="graph-select" size="sm"
            options={graphs.map(g => ({ value: graphIri(g), label: graphIri(g) }))} />
        {:else}
          <input bind:value={uploadRdfGraph} placeholder={$i18nT('pages.datasetDetail.targetGraphIri')} class="graph-iri-input" />
        {/if}
        <label class="toggle">
          <input type="checkbox" bind:checked={uploadRdfReplace} />
          <span class="toggle-track"><span class="toggle-thumb"></span></span>
          <span class="toggle-text">{$i18nT('pages.datasetDetail.replace')}</span>
        </label>
        <button class="btn btn-sm" on:click={handleRdfUpload} disabled={uploadingRdf || !uploadRdfFile || !uploadRdfGraph}>
          {#if uploadingRdf}<Loader2 size={13} class="animate-spin" />{:else}<Upload size={13} />{/if}
          {$i18nT('pages.datasetDetail.upload')}
        </button>
        <button class="btn btn-sm btn-ghost" on:click={() => showRdfUpload = false}><XIcon size={13} /></button>
      </div>
      {#if uploadRdfError}<p class="upload-error">{uploadRdfError}</p>{/if}
    </div>
  {/if}

  {#if canWrite}
  <div class="inline-form">
    <input placeholder={$i18nT('pages.datasetDetail.graphIri')} bind:value={newGraphIri} />
    <button class="btn btn-sm" on:click={addGraph}><Plus size={13} /> {$i18nT('system.add')}</button>
  </div>
  {/if}
  <ul class="graph-list">
    {#each graphs as g}
      {@const iri = graphIri(g)}
      {@const role = normalizeGraphRole(graphRole(g))}
      {@const isPrivate = graphPrivate(g)}
      <li class="graph-item">
        <div class="graph-item-main">
          <code class="graph-iri">{iri}</code>
          {#if role}
            <span class="graph-role-badge role-{role}" title={ROLE_HINT[role] || ''}><Tag size={11} /> {graphRoleLabel(role)}</span>
          {/if}
          {#if isPrivate}
            <span class="graph-private-badge" title={$i18nT('pages.datasetDetail.privateBadgeTitle')}><Lock size={11} /> {$i18nT('pages.datasetDetail.private')}</span>
          {/if}
        </div>
        <div class="graph-item-actions">
          {#if canWrite}
            {#if editingGraphRoleIri === iri}
              <Select
                size="sm"
                disabled={updatingGraphRole}
                value={role ?? ''}
                options={GRAPH_ROLE_OPTIONS}
                on:change={(e) => setGraphRole(g, e.detail)}
              />
              <button class="btn btn-sm btn-ghost" on:click={() => editingGraphRoleIri = null}><XIcon size={12} /></button>
            {:else}
              <button class="btn btn-sm btn-ghost" title={$i18nT('pages.datasetDetail.setGraphRole')} on:click={() => editingGraphRoleIri = iri}>
                <Tag size={12} /> {$i18nT('pages.datasetDetail.role')}
              </button>
            {/if}
            <button
              class="btn btn-sm btn-ghost"
              title={$i18nT('pages.datasetDetail.attachShapesGraphTitle')}
              on:click={() => attachTarget = { kind: 'graph', id: iri, label: iri }}
            >
              <ShieldCheck size={12} /> {$i18nT('pages.datasetDetail.shapes')}
            </button>
            <button
              class="btn btn-sm btn-ghost"
              class:btn-active={isPrivate}
              title={isPrivate ? $i18nT('pages.datasetDetail.makeGraphVisibleTitle') : $i18nT('pages.datasetDetail.makeGraphPrivateTitle')}
              disabled={updatingGraphPrivacy === iri}
              on:click={() => toggleGraphPrivacy(g)}
            >
              {#if isPrivate}<Lock size={12} /> {$i18nT('pages.datasetDetail.private')}{:else}<Globe size={12} /> {$i18nT('pages.datasetDetail.public')}{/if}
            </button>
            <button class="btn btn-sm btn-danger" on:click={() => removeGraph(g)}><XIcon size={13} /></button>
          {/if}
        </div>
      </li>
    {/each}
    {#if graphs.length === 0}
      <li class="empty">{$i18nT('pages.datasetDetail.noGraphs')}</li>
    {/if}
  </ul>
</div>

<!-- Version snapshots + branches -->
<div class="card" id="dataset-versions">
  <DatasetVersions {id} {canWrite} {graphs} />
</div>

<!-- Commit history -->
<CommitHistory kind="dataset" {id} />

<!-- Assets -->
<div class="card">
  <div class="section-head">
    <div class="section-head-left">
      <FileText size={15} />
      <h3>{$i18nT('pages.datasetDetail.assets')}</h3>
    </div>
    {#if canWrite}
    <label class="btn btn-sm" class:btn-loading={uploading}>
      {#if uploading}<Loader2 size={13} class="animate-spin" />{:else}<Upload size={13} />{/if}
      {uploading ? $i18nT('pages.datasetDetail.uploading') : $i18nT('pages.datasetDetail.uploadFile')}
      <input type="file" style="display:none" on:change={handleFileUpload} disabled={uploading} />
    </label>
    {/if}
  </div>

  {#if uploading && uploadProgress > 0}
    <div class="progress-bar">
      <div class="progress-fill" style="width: {uploadProgress * 100}%"></div>
    </div>
  {/if}

  {#if assetsError}
    <div class="assets-error">
      <span>{assetsError}</span>
      <button class="btn btn-xs btn-ghost" on:click={() => assetsError = ''}><XIcon size={12} /></button>
    </div>
  {/if}

  {#if assets.length > 0}
    <div class="asset-cards">
      {#each assets as asset}
        <div class="asset-card">
          <div class="asset-card-icon">
            <FileText size={20} />
          </div>
          <div class="asset-card-body">
            <div class="asset-card-title">
              {asset.title || asset.filename}
              {#if asset.title && asset.title !== asset.filename}
                <span class="asset-filename-sub">{asset.filename}</span>
              {/if}
            </div>
            {#if asset.description}
              <p class="asset-card-desc">{asset.description}</p>
            {/if}
            <div class="asset-card-meta">
              <code class="media-type">{asset.content_type}</code>
              <span class="asset-size">{formatBytes(asset.size_bytes)}</span>
              <span class="asset-created">{$i18nT('pages.datasetDetail.uploadedDate', { values: { date: new Date(asset.created_at).toLocaleDateString() } })}</span>
              {#if asset.updated_at}
                <span class="asset-created">{$i18nT('pages.datasetDetail.updatedDate', { values: { date: new Date(asset.updated_at).toLocaleDateString() } })}</span>
              {/if}
            </div>
          </div>
          <div class="asset-card-actions">
            {#if canWrite}
            <button
              class="btn btn-xs visibility-btn"
              class:vis-public={asset.public}
              class:vis-private={!asset.public}
              on:click={() => toggleAssetVisibility(asset)}
              title={asset.public ? $i18nT('pages.datasetDetail.assetPublicToggleTitle') : $i18nT('pages.datasetDetail.assetPrivateToggleTitle')}
            >
              {#if asset.public}<Globe size={11} /> {$i18nT('pages.datasetDetail.public')}{:else}<Lock size={11} /> {$i18nT('pages.datasetDetail.private')}{/if}
            </button>
            {:else}
            <span class="vis-badge {asset.public ? 'vis-public' : 'vis-private'}">
              {#if asset.public}<Globe size={11} /> {$i18nT('pages.datasetDetail.public')}{:else}<Lock size={11} /> {$i18nT('pages.datasetDetail.private')}{/if}
            </span>
            {/if}
            {#if previewKind(asset)}
              <button class="btn btn-xs btn-ghost" on:click={() => previewAsset = asset} title={$i18nT('pages.datasetDetail.preview')}><Eye size={12} /></button>
            {/if}
            <button class="btn btn-xs btn-ghost" on:click={() => downloadAssetFile(asset)} title={$i18nT('pages.datasetDetail.download')}><Download size={12} /></button>
            <button class="btn btn-xs btn-ghost" on:click={() => copyAssetIri(asset)} title={$i18nT('pages.datasetDetail.copyLinkedDataIri')}>
              {#if copiedAssetId === asset.id}<CheckCheck size={12} />{:else}<LinkIcon size={12} />{/if}
            </button>
            <button class="btn btn-xs btn-ghost" on:click={() => copyAssetTurtle(asset)} title={$i18nT('pages.datasetDetail.copyTurtleDescription')}><Clipboard size={12} /></button>
            {#if canWrite}
              <button class="btn btn-xs btn-ghost" on:click={() => openAssetEdit(asset)} title={$i18nT('pages.datasetDetail.editMetadata')}><Pencil size={12} /></button>
              <button class="btn btn-xs btn-ghost btn-danger" on:click={() => deleteAssetId = asset.id} title={$i18nT('pages.datasetDetail.deleteAsset')}><Trash2 size={12} /></button>
            {/if}
          </div>
        </div>
      {/each}
    </div>
  {:else if !assetsError}
    <p class="empty">{$i18nT('pages.datasetDetail.noAssets')}</p>
  {/if}
</div>

<!-- Asset preview modal -->
{#if previewAsset}
  <AssetPreview asset={previewAsset} datasetId={id} on:close={() => previewAsset = null} />
{/if}

<!-- Access management (users + teams, role-based) -->
{#if canManage}
<div class="card">
  <div class="section-head">
    <div class="section-head-left">
      <Users size={15} />
      <h3>{$i18nT('pages.datasetDetail.access')}</h3>
    </div>
  </div>
  <p class="hint-text">
    {$i18nT('pages.datasetDetail.accessHintIntro')} <strong>{$i18nT('pages.datasetDetail.roleViewer')}</strong> {$i18nT('pages.datasetDetail.accessHintViewer')}
    <strong>{$i18nT('pages.datasetDetail.roleEditor')}</strong> {$i18nT('pages.datasetDetail.accessHintEditor')} <strong>{$i18nT('pages.datasetDetail.roleAdmin')}</strong> {$i18nT('pages.datasetDetail.accessHintAdmin')}
    {$i18nT('pages.datasetDetail.accessHintOverride')}
  </p>
  {#if accessError}<p class="error">{accessError}</p>{/if}
  {#if accessLoading}
    <p class="hint-text"><Loader2 size={13} class="animate-spin" /> {$i18nT('pages.datasetDetail.loadingAccess')}</p>
  {:else}
    <!-- People -->
    <div class="access-subhead-row">
      <h4 class="access-subhead">{$i18nT('pages.datasetDetail.people')}</h4>
      <button class="btn btn-xs" on:click={openAddPerson}><UserPlus size={12} /> {$i18nT('pages.datasetDetail.addPerson')}</button>
    </div>
    <ul class="access-list">
      {#each peopleRows as p (p.userId)}
        <li class="access-row">
          <span class="access-principal">
            <Avatar kind="user" id={p.userId} name={p.username} hasImage={!!p.avatar_key} size={26} />
            <span class="access-principal-text">
              <span class="access-name">{p.username}</span>
              <span class="access-sources">
                {#if p.effective}
                  <span class="eff-badge eff-{p.effective}">{p.effective}</span>
                {:else}
                  <span class="eff-badge eff-none">{$i18nT('pages.datasetDetail.noAccess')}</span>
                {/if}
                {#if p.isOwner}
                  <span class="src-chip src-owner">{$i18nT('pages.datasetDetail.ownerLower')}</span>
                {:else if p.direct}
                  <span class="src-chip src-direct">{$i18nT('pages.datasetDetail.directRole', { values: { role: p.direct } })}</span>
                {:else if p.orgRole}
                  <span class="src-chip src-inherit">{$i18nT('pages.datasetDetail.orgRole', { values: { role: p.orgRole } })}{p.default ? ` → ${p.default}` : ` → ${$i18nT('pages.datasetDetail.none')}`}</span>
                {/if}
                {#each p.viaTeams as vt}
                  <span class="src-chip src-team" title={$i18nT('pages.datasetDetail.grantedViaTeam')}><Users size={10} /> {vt.name}: {vt.role}</span>
                {/each}
                {#if p.direct && p.viaTeams.some(vt => ROLE_RANK[vt.role] > ROLE_RANK[p.direct])}
                  <span class="src-note">{$i18nT('pages.datasetDetail.teamGrantOutranks')}</span>
                {/if}
              </span>
            </span>
          </span>
          {#if p.isOwner}
            <span class="owner-pill" title={$i18nT('pages.datasetDetail.ownerFullAccessTitle')}>{$i18nT('pages.datasetDetail.owner')}</span>
          {:else}
            <Select
              class="grant-select"
              size="sm"
              disabled={savingPrincipal === `user:${p.userId}`}
              value={p.direct || ''}
              on:change={(e) => setUserGrant(p.userId, e.detail)}
              options={[
                { value: '', label: p.default ? $i18nT('pages.datasetDetail.defaultWithRole', { values: { role: p.default } }) : $i18nT('pages.datasetDetail.defaultNoAccess') },
                { value: 'viewer', label: $i18nT('pages.datasetDetail.roleViewer') },
                { value: 'editor', label: $i18nT('pages.datasetDetail.roleEditor') },
                { value: 'admin', label: $i18nT('pages.datasetDetail.roleAdmin') },
              ]} />
          {/if}
        </li>
      {/each}
      {#if peopleRows.length === 0}
        <li class="access-empty">{$i18nT('pages.datasetDetail.noPeopleAccess')}</li>
      {/if}
    </ul>

    <!-- Teams -->
    {#if orgGroups.length > 0}
      <h4 class="access-subhead">{$i18nT('pages.datasetDetail.teams')}</h4>
      <ul class="access-list">
        {#each orgGroups as g (g.id)}
          <li class="access-row">
            <span class="access-principal">
              <Avatar kind="group" id={g.id} name={g.name} size={26} />
              <span class="access-principal-text">
                <span class="access-name">{g.name}</span>
                <span class="access-sources">
                  <span class="muted">{$i18nT('pages.datasetDetail.memberCount', { values: { count: (g.members || []).length } })}</span>
                  {#if grantMap[`group:${g.id}`]}
                    <span class="src-note">{$i18nT('pages.datasetDetail.appliesToAllMembers')}</span>
                  {/if}
                </span>
              </span>
            </span>
            <Select
              class="grant-select"
              size="sm"
              disabled={savingPrincipal === `group:${g.id}`}
              value={grantMap[`group:${g.id}`] || ''}
              on:change={(e) => setGroupGrant(g.id, e.detail)}
              options={[
                { value: '', label: $i18nT('pages.datasetDetail.noAccessOption') },
                ...RESOURCE_ROLES.map(r => ({ value: r.value, label: r.label })),
              ]} />
          </li>
        {/each}
      </ul>
    {/if}

    <!-- Organisations -->
    {#if orgGrantRows.length > 0}
      <h4 class="access-subhead">{$i18nT('pages.organisations.title')}</h4>
      <ul class="access-list">
        {#each orgGrantRows as o (o.id)}
          <li class="access-row">
            <span class="access-principal">
              <Avatar kind="organisation" id={o.id} name={o.name} size={26} />
              <span class="access-principal-text">
                <span class="access-name">{o.name}</span>
                <span class="access-sources">
                  {#if grantMap[`organisation:${o.id}`]}
                    <span class="src-note">{$i18nT('pages.datasetDetail.appliesToAllMembers')}</span>
                  {/if}
                </span>
              </span>
            </span>
            <Select
              class="grant-select"
              size="sm"
              disabled={savingPrincipal === `organisation:${o.id}`}
              value={grantMap[`organisation:${o.id}`] || ''}
              on:change={(e) => setOrgGrant(o.id, e.detail)}
              options={[
                { value: '', label: $i18nT('pages.datasetDetail.noAccessOption') },
                ...RESOURCE_ROLES.map(r => ({ value: r.value, label: r.label })),
              ]} />
          </li>
        {/each}
      </ul>
    {/if}
  {/if}
</div>
{/if}

<!-- SPARQL Services -->
<div class="card">
  <div class="section-head">
    <div class="section-head-left">
      <Database size={15} />
      <h3>{$i18nT('pages.datasetDetail.sparqlServices')}</h3>
    </div>
    {#if canWrite}
      <button class="btn btn-sm" on:click={openAddServiceModal}><Plus size={13} /> {$i18nT('pages.datasetDetail.newService')}</button>
    {/if}
  </div>

  {#if services.length > 0}
  <div class="svc-list">
    {#each services as svc}
      <div class="svc-card" class:svc-card-inactive={!svc.is_active}>
        <!-- Service header row -->
        <div class="svc-card-header">
          <div class="svc-card-meta">
            {#if editSvcId === svc.id}
              <input bind:value={editSvcName} class="inline-edit-input svc-name-input" placeholder={$i18nT('pages.datasetDetail.serviceName')} />
            {:else}
              <span class="svc-name">{svc.name}</span>
            {/if}
            <code class="svc-slug">{svc.slug}</code>
            <span class="svc-badge" class:svc-badge-active={svc.is_active} class:svc-badge-inactive={!svc.is_active}>
              {svc.is_active ? $i18nT('pages.datasetDetail.active') : $i18nT('pages.datasetDetail.inactive')}
            </span>
          </div>
          <div class="svc-card-actions">
            {#if editSvcId === svc.id}
              <button class="btn btn-xs" on:click={saveService} disabled={savingSvc}>
                {#if savingSvc}<Loader2 size={12} class="animate-spin" />{:else}<Check size={12} />{/if} {$i18nT('system.save')}
              </button>
              <button class="btn btn-xs btn-ghost" on:click={() => editSvcId = null}><XIcon size={12} /></button>
            {:else}
              {#if svc.is_active}
                <div class="svc-endpoint-row">
                  <code class="endpoint-url">{window.location.origin}/api/datasets/{id}/services/{svc.slug}/sparql</code>
                  <button class="btn btn-xs btn-ghost copy-btn" on:click={() => copyEndpoint(svc.slug)} title={$i18nT('pages.datasetDetail.copyEndpointUrl')}>
                    {#if copiedSlug === svc.slug}<CheckCheck size={12} />{:else}<Copy size={12} />{/if}
                  </button>
                </div>
              {/if}
              {#if canWrite}
                <button class="btn btn-xs btn-ghost" on:click={() => openEditSvc(svc)} title={$i18nT('system.edit')}><Edit2 size={12} /></button>
                <button class="btn btn-xs btn-ghost" on:click={() => toggleService(svc)} title={svc.is_active ? $i18nT('pages.datasetDetail.deactivate') : $i18nT('pages.datasetDetail.activate')}>
                  <Power size={12} class={svc.is_active ? 'power-on' : 'power-off'} />
                </button>
                <button class="btn btn-xs btn-ghost btn-danger" on:click={() => deleteServiceTarget = svc} title={$i18nT('system.delete')}><Trash2 size={12} /></button>
              {/if}
            {/if}
          </div>
        </div>

        {#if editSvcId === svc.id}
          <input bind:value={editSvcDesc} placeholder={$i18nT('pages.datasetDetail.descriptionOptional')} class="inline-edit-input" style="margin: 0.4rem 0; width:100%" />
        {:else if svc.description}
          <p class="svc-desc">{svc.description}</p>
        {/if}

        <!-- Graph subset panel — always visible for write users -->
        {#if canWrite}
          <div class="svc-graph-section">
            <button class="svc-graph-toggle" on:click={() => toggleSvcGraphPanel(svc)}>
              <Database size={12} />
              <span>{$i18nT('pages.datasetDetail.graphSubset')}</span>
              {#if expandedSvcId === svc.id && !svcGraphsLoading}
                <span class="svc-graph-count-hint">{$i18nT('pages.datasetDetail.graphSubsetCount', { values: { selected: svcGraphs.length, total: graphs.length } })}</span>
              {/if}
              <span class="svc-graph-chevron" class:open={expandedSvcId === svc.id}>›</span>
            </button>
            {#if expandedSvcId === svc.id}
              <div class="svc-graph-panel-inner">
                <div class="svc-graph-panel-header">
                  <span class="svc-graph-panel-label">{$i18nT('pages.datasetDetail.selectGraphsExposed')}</span>
                  <div class="svc-graph-bulk">
                    <button class="btn btn-xs btn-ghost" on:click={() => bulkSetAllServiceGraphs(true)} disabled={svcGraphsSaving}>{$i18nT('pages.datasetDetail.all')}</button>
                    <button class="btn btn-xs btn-ghost" on:click={() => bulkSetAllServiceGraphs(false)} disabled={svcGraphsSaving}>{$i18nT('pages.datasetDetail.none')}</button>
                  </div>
                </div>
                {#if svcGraphsLoading}
                  <div class="svc-graph-loading"><Loader2 size={13} class="animate-spin" /> {$i18nT('system.loading')}</div>
                {:else if graphs.length === 0}
                  <p class="svc-graph-empty">{$i18nT('pages.datasetDetail.noNamedGraphsYet')}</p>
                {:else}
                  <div class="svc-graph-list">
                    {#each graphs as g}
                      {@const iri = typeof g === 'string' ? g : g.graph_iri}
                      {@const isRegistered = svcGraphs.includes(iri)}
                      <label class="svc-graph-item" class:checked={isRegistered}>
                        <input type="checkbox" checked={isRegistered} disabled={svcGraphsSaving}
                          on:change={() => toggleGraphInService(iri, isRegistered)} />
                        <code class="svc-graph-iri">{iri}</code>
                        {#if isRegistered}<span class="svc-graph-tick"><Check size={11} /></span>{/if}
                      </label>
                    {/each}
                  </div>
                {/if}
              </div>
            {/if}
          </div>
        {/if}
      </div>
    {/each}
  </div>
  {:else}
    <p class="empty">{$i18nT('pages.datasetDetail.noServices')}</p>
  {/if}
</div>

<!-- SHACL Validation -->
<div class="card">
  <div class="section-head">
    <div class="section-head-left">
      <ShieldCheck size={15} />
      <h3>{$i18nT('pages.datasetDetail.validation')}</h3>
    </div>
    {#if $isAuthenticated}
      <div class="flex items-center gap-2">
        <button class="btn btn-sm btn-ghost" title={$i18nT('pages.datasetDetail.attachShapesDatasetTitle')} on:click={() => attachTarget = { kind: 'dataset', id, label: dataset?.name || id }}>
          <ShieldCheck size={13} /> {$i18nT('pages.datasetDetail.attachShapes')}
        </button>
        <button class="btn btn-sm btn-ghost" on:click={() => showValidationModal = true}>
          {#if validating}<Loader2 size={13} class="animate-spin" />{:else}<ShieldCheck size={13} />{/if}
          {$i18nT('pages.datasetDetail.runValidation')}
        </button>
      </div>
    {/if}
  </div>

  {#if $isAuthenticated}
    <div class="effective-shapes">
      <div class="eff-head">
        {$i18nT('pages.datasetDetail.effectiveShapes')}
        <span class="eff-hint">{$i18nT('pages.datasetDetail.effectiveShapesHint')}</span>
      </div>
      {#if loadingEffective}
        <span class="hint-text"><Loader2 size={12} class="animate-spin" /> {$i18nT('system.loading')}</span>
      {:else if effectiveShapes.length === 0}
        <span class="hint-text">{$i18nT('pages.datasetDetail.noShapesAttachedPre')}<strong>{$i18nT('pages.datasetDetail.attachShapes')}</strong>{$i18nT('pages.datasetDetail.noShapesAttachedPost')}</span>
      {:else}
        <ul class="eff-list">
          {#each effectiveShapes as s (s.id)}
            <li class="eff-item">
              <ShieldCheck size={12} class="text-[var(--brand-500)]" />
              <Link to={`/shacl/shape-graphs/${s.id}`} class="eff-name">{s.name}</Link>
              {#if datasetBoundIds.has(s.id)}
                <span class="eff-badge eff-dataset" title={$i18nT('pages.datasetDetail.boundDirectlyTitle')}>{$i18nT('pages.datasetDetail.datasetBadge')}</span>
              {:else}
                <span class="eff-badge eff-inherited" title={$i18nT('pages.datasetDetail.inheritedFromGraphTitle')}>{$i18nT('pages.datasetDetail.inheritedFromGraph')}</span>
              {/if}
              {#if s.status}<span class="eff-badge eff-status-{s.status}">{s.status}</span>{/if}
            </li>
          {/each}
        </ul>
      {/if}
    </div>
  {/if}

  {#if validationError}
    <p class="error validation-error">{validationError}</p>
  {/if}

  {#if validationReport}
    <div class="report" class:conforms={validationReport.conforms}>
      <p><strong>{#if validationReport.conforms}<Check size={14} /> {$i18nT('pages.datasetDetail.conforms')}{:else}<XIcon size={14} /> {$i18nT('pages.datasetDetail.doesNotConform')}{/if}</strong>
        — {validationReport.results_count} {$i18nT('pages.datasetDetail.results')}
        {#if validationRanAt}<span class="run-meta" title={validationRanAt}> · {fmtDateTime(validationRanAt)}</span>{/if}</p>
      {#if validationReport.results.length > 0}
        <table>
          <thead>
            <tr><th>{$i18nT('pages.datasetDetail.severity')}</th><th>{$i18nT('pages.datasetDetail.focusNode')}</th><th>{$i18nT('pages.datasetDetail.path')}</th><th>{$i18nT('pages.datasetDetail.message')}</th></tr>
          </thead>
          <tbody>
            {#each validationReport.results as r}
              <tr>
                <td><span class="sev sev-{r.severity}">{r.severity}</span></td>
                <td><code>{r.focus_node}</code></td>
                <td>{r.path || '—'}</td>
                <td>{r.message}</td>
              </tr>
            {/each}
          </tbody>
        </table>
      {/if}
    </div>
  {:else if !validationError}
    <p class="hint-text">{$i18nT('pages.datasetDetail.runValidationHint')}</p>
  {/if}

  {#if validationHistory.length > 0}
    <div class="val-history">
      <div class="eff-head">{$i18nT('pages.datasetDetail.recentValidationRuns')}</div>
      <ul class="val-history-list">
        {#each validationHistory as run (run.id)}
          <li class="val-history-item">
            {#if run.conforms}
              <span class="vh-pill vh-ok"><Check size={11} /> {$i18nT('pages.datasetDetail.conforms')}</span>
            {:else}
              <span class="vh-pill vh-fail"><XIcon size={11} /> {run.results_count} {$i18nT('pages.datasetDetail.results')}</span>
            {/if}
            <span class="vh-time" title={run.run_timestamp}>{fmtDateTime(run.run_timestamp)}</span>
          </li>
        {/each}
      </ul>
    </div>
  {/if}
</div>
</div><!-- /detail-stack -->

{#if attachTarget}
  <AttachShapesDialog
    targetKind={attachTarget.kind}
    targetId={attachTarget.id}
    targetLabel={attachTarget.label}
    on:changed={loadEffectiveShapes}
    on:close={() => attachTarget = null}
  />
{/if}

{#if deleteAssetId !== null}
  <ConfirmModal
    title={$i18nT('pages.datasetDetail.deleteAssetConfirmTitle')}
    message={$i18nT('pages.datasetDetail.cannotBeUndone')}
    confirmLabel={$i18nT('pages.datasetDetail.deleteAssetConfirm')}
    on:confirm={doDeleteAsset}
    on:cancel={() => deleteAssetId = null}
  />
{/if}

{#if deleteServiceTarget !== null}
  <ConfirmModal
    title={$i18nT('pages.datasetDetail.deleteServiceConfirmTitle', { values: { name: deleteServiceTarget.name } })}
    message={$i18nT('pages.datasetDetail.cannotBeUndone')}
    confirmLabel={$i18nT('pages.datasetDetail.deleteServiceConfirm')}
    on:confirm={doDeleteService}
    on:cancel={() => deleteServiceTarget = null}
  />
{/if}

<!-- Add Service modal -->
{#if showAddServiceModal}
  <div class="modal-backdrop" on:click={() => showAddServiceModal = false} role="presentation" on:keydown={(e) => e.key === 'Escape' && (showAddServiceModal = false)}>
    <div class="modal-box modal-lg" on:click|stopPropagation on:keydown|stopPropagation role="dialog" aria-modal="true" aria-label={$i18nT('pages.datasetDetail.addSparqlServiceAria')} tabindex="-1">
      <div class="modal-header">
        <h3><Database size={16} /> {$i18nT('pages.datasetDetail.newSparqlService')}</h3>
        <button class="btn btn-xs btn-ghost" on:click={() => showAddServiceModal = false}><XIcon size={14} /></button>
      </div>
      {#if addServiceError}<p class="error">{addServiceError}</p>{/if}
      <div class="modal-body">
        <div class="form-group">
          <label for="svc-name">{$i18nT('pages.datasetDetail.serviceName')}</label>
          <input id="svc-name" bind:value={newServiceName} placeholder={$i18nT('pages.datasetDetail.serviceNamePlaceholder')} required />
        </div>
        <div class="form-group">
          <label for="svc-slug">{$i18nT('pages.datasetDetail.urlSlug')} <span class="field-hint">{$i18nT('pages.datasetDetail.urlSlugHint')}</span></label>
          <div class="slug-row">
            <code class="slug-prefix">/api/datasets/{id}/services/</code>
            <input id="svc-slug" bind:value={newServiceSlug}
              on:input={() => newServiceSlugEdited = true}
              placeholder={$i18nT('pages.datasetDetail.slugPlaceholder')} pattern="[a-z0-9\-]+" required />
            <code class="slug-suffix">/sparql</code>
          </div>
        </div>
        <div class="form-group">
          <label for="svc-desc">{$i18nT('pages.datasets.description')} <span class="field-hint">{$i18nT('pages.datasetDetail.optionalParen')}</span></label>
          <input id="svc-desc" bind:value={newServiceDesc} placeholder={$i18nT('pages.datasetDetail.serviceDescPlaceholder')} />
        </div>
        <div class="form-group">
          <span class="group-caption">{$i18nT('pages.datasetDetail.graphSubset')} <span class="field-hint">{$i18nT('pages.datasetDetail.graphSubsetCaptionHint')}</span></span>
          {#if graphs.length === 0}
            <p class="field-empty-hint">{$i18nT('pages.datasetDetail.noNamedGraphsAddAfter')}</p>
          {:else}
            <div class="svc-graph-picker">
              <div class="svc-graph-picker-head">
                <button class="btn btn-xs btn-ghost" type="button" on:click={() => { newServiceGraphs = new Set(graphs.map(g => typeof g === 'string' ? g : g.graph_iri)); }}>{$i18nT('pages.datasetDetail.all')}</button>
                <button class="btn btn-xs btn-ghost" type="button" on:click={() => newServiceGraphs = new Set()}>{$i18nT('pages.datasetDetail.none')}</button>
              </div>
              {#each graphs as g}
                {@const iri = typeof g === 'string' ? g : g.graph_iri}
                <label class="svc-graph-item" class:checked={newServiceGraphs.has(iri)}>
                  <input type="checkbox" checked={newServiceGraphs.has(iri)}
                    on:change={() => {
                      if (newServiceGraphs.has(iri)) newServiceGraphs.delete(iri);
                      else newServiceGraphs.add(iri);
                      newServiceGraphs = new Set(newServiceGraphs);
                    }} />
                  <code class="svc-graph-iri">{iri}</code>
                </label>
              {/each}
              <p class="svc-graph-count">{$i18nT('pages.datasetDetail.graphsSelectedCount', { values: { selected: newServiceGraphs.size, total: graphs.length } })}</p>
            </div>
          {/if}
        </div>
      </div>
      <div class="modal-footer">
        <button class="btn btn-ghost" on:click={() => showAddServiceModal = false}>{$i18nT('system.cancel')}</button>
        <button class="btn" on:click={addService} disabled={addingService || !newServiceName || !newServiceSlug}>
          {#if addingService}<Loader2 size={14} class="animate-spin" /> {$i18nT('pages.datasetDetail.creating')}{:else}<Plus size={14} /> {$i18nT('pages.datasetDetail.createService')}{/if}
        </button>
      </div>
    </div>
  </div>
{/if}

<!-- SHACL Validation modal -->
{#if showValidationModal}
  <div class="modal-backdrop" on:click={() => showValidationModal = false} role="presentation" on:keydown={(e) => e.key === 'Escape' && (showValidationModal = false)}>
    <div class="modal-box" on:click|stopPropagation on:keydown|stopPropagation role="dialog" aria-modal="true" aria-label={$i18nT('pages.datasetDetail.validation')} tabindex="-1">
      <div class="modal-header">
        <h3><ShieldCheck size={16} /> {$i18nT('pages.datasetDetail.validation')}</h3>
        <button class="btn btn-xs btn-ghost" on:click={() => showValidationModal = false}><XIcon size={14} /></button>
      </div>
      <div class="modal-body">
        <div class="form-group">
          <label for="shapes-iri">{$i18nT('pages.datasetDetail.shapesGraphIri')}</label>
          <input id="shapes-iri" bind:value={shapesGraphIri} placeholder="https://example.org/shapes" />
          <p class="field-hint-text">{$i18nT('pages.datasetDetail.shapesGraphBlankHint')}</p>
        </div>
        {#if validationError}
          <p class="error validation-error">{validationError}</p>
        {/if}
        {#if validationReport}
          <div class="report" class:conforms={validationReport.conforms}>
            <p><strong>{#if validationReport.conforms}<Check size={14} /> {$i18nT('pages.datasetDetail.conforms')}{:else}<XIcon size={14} /> {$i18nT('pages.datasetDetail.doesNotConform')}{/if}</strong>
              \u2014 {validationReport.results_count} {$i18nT('pages.datasetDetail.results')}</p>
            {#if validationReport.results.length > 0}
              <table>
                <thead><tr><th>{$i18nT('pages.datasetDetail.severity')}</th><th>{$i18nT('pages.datasetDetail.focusNode')}</th><th>{$i18nT('pages.datasetDetail.path')}</th><th>{$i18nT('pages.datasetDetail.message')}</th></tr></thead>
                <tbody>
                  {#each validationReport.results as r}
                    <tr>
                      <td><span class="sev sev-{r.severity}">{r.severity}</span></td>
                      <td><code>{r.focus_node}</code></td>
                      <td>{r.path || '\u2014'}</td>
                      <td>{r.message}</td>
                    </tr>
                  {/each}
                </tbody>
              </table>
            {/if}
          </div>
        {/if}
      </div>
      <div class="modal-footer">
        <button class="btn btn-ghost" on:click={() => showValidationModal = false}>{$i18nT('system.close')}</button>
        <button class="btn" on:click={runValidation} disabled={validating}>
          {#if validating}<Loader2 size={14} class="animate-spin" /> {$i18nT('pages.datasetDetail.validating')}{:else}<ShieldCheck size={14} /> {$i18nT('pages.datasetDetail.runValidation')}{/if}
        </button>
      </div>
    </div>
  </div>
{/if}

<!-- Dataset metadata edit dialog -->
<DatasetMetadataDialog
  open={metadataDialogOpen}
  {dataset}
  saving={savingDataset}
  error={dialogError}
  hasImage={!!imageKey}
  imageUrl={`${getDatasetImageUrl(id)}?v=${imageVersion}`}
  uploadingImage={uploadingImage}
  bannerKey={bannerKey}
  bannerUrl={`${getDatasetBannerUrl(id)}?v=${bannerVersion}`}
  uploadingBanner={uploadingBanner}
  deleting={deletingDataset}
  on:close={() => { if (!savingDataset && !deletingDataset) metadataDialogOpen = false; }}
  on:save={handleMetadataSave}
  on:uploadImage={(e) => doUploadImage(e.detail.file)}
  on:uploadBanner={(e) => doUploadBanner(e.detail.file)}
  on:selectBannerPreset={(e) => doSetBannerPreset(e.detail.preset)}
  on:clearBanner={() => doClearBanner()}
  on:delete={handleDeleteDataset}
/>

<!-- Grant Access modal -->
<!-- Add person to dataset access modal -->
{#if showAddPersonModal}
  <div class="modal-backdrop" on:click={() => showAddPersonModal = false} role="presentation" on:keydown={(e) => e.key === 'Escape' && (showAddPersonModal = false)}>
    <div class="modal-box" on:click|stopPropagation on:keydown|stopPropagation role="dialog" aria-modal="true" aria-label={$i18nT('pages.datasetDetail.addPersonAria')} tabindex="-1">
      <div class="modal-header">
        <h3><UserPlus size={16} /> {$i18nT('pages.datasetDetail.addPersonTitle')}</h3>
        <button class="btn btn-xs btn-ghost" on:click={() => showAddPersonModal = false}><XIcon size={14} /></button>
      </div>
      <div class="modal-body">
        {#if accessError}<p class="error">{accessError}</p>{/if}
        <div class="form-group">
          <label for="add-person-user">{$i18nT('pages.datasetDetail.person')}</label>
          <Select id="add-person-user" bind:value={addUserId}
            options={[
              { value: '', label: $i18nT('pages.datasetDetail.selectPerson') },
              ...addCandidates.map(u => ({ value: u.id, label: u.username })),
            ]} />
          {#if addCandidates.length === 0}
            <p class="field-hint-text">{$i18nT('pages.datasetDetail.everyoneHasAccess')}</p>
          {/if}
        </div>
        <div class="form-group">
          <label for="add-person-role">{$i18nT('pages.datasetDetail.role')}</label>
          <Select id="add-person-role" bind:value={addUserRole}
            options={[
              { value: 'viewer', label: $i18nT('pages.datasetDetail.roleViewerOption') },
              { value: 'editor', label: $i18nT('pages.datasetDetail.roleEditorOption') },
              { value: 'admin', label: $i18nT('pages.datasetDetail.roleAdminOption') },
            ]} />
        </div>
      </div>
      <div class="modal-footer">
        <button class="btn btn-ghost" on:click={() => showAddPersonModal = false}>{$i18nT('system.cancel')}</button>
        <button class="btn" on:click={addPerson} disabled={!addUserId || savingPrincipal === `user:${addUserId}`}>
          {#if savingPrincipal === `user:${addUserId}`}<Loader2 size={14} class="animate-spin" /> {$i18nT('pages.datasetDetail.adding')}{:else}<UserPlus size={14} /> {$i18nT('system.add')}{/if}
        </button>
      </div>
    </div>
  </div>
{/if}

<!-- Asset edit metadata modal -->
{#if editingAsset}
  <div class="modal-backdrop" on:click={() => editingAsset = null} role="presentation" on:keydown={(e) => e.key === 'Escape' && (editingAsset = null)}>
    <div class="modal-box" on:click|stopPropagation on:keydown|stopPropagation role="dialog" aria-modal="true" aria-label={$i18nT('pages.datasetDetail.editAssetMetadataAria')} tabindex="-1">
      <div class="modal-header">
        <h3><Pencil size={16} /> {$i18nT('pages.datasetDetail.editAssetMetadata')}</h3>
        <button class="btn btn-xs btn-ghost" on:click={() => editingAsset = null}><XIcon size={14} /></button>
      </div>
      <div class="modal-body">
        <p class="asset-edit-filename"><FileText size={13} /> {editingAsset.filename} \u00b7 {formatBytes(editingAsset.size_bytes)}</p>
        {#if assetEditError}<p class="error">{assetEditError}</p>{/if}
        <div class="form-group">
          <label for="asset-title">{$i18nT('pages.datasetDetail.displayTitle')} <span class="field-hint">{$i18nT('pages.datasetDetail.displayTitleHint')}</span></label>
          <input id="asset-title" bind:value={assetEditTitle} placeholder={editingAsset.filename} />
        </div>
        <div class="form-group">
          <label for="asset-desc">{$i18nT('pages.datasets.description')}</label>
          <textarea id="asset-desc" bind:value={assetEditDesc} placeholder={$i18nT('pages.datasetDetail.assetDescPlaceholder')} rows="3"></textarea>
        </div>
        <div class="form-group metadata-readonly-group">
          <p class="meta-readonly-label">{$i18nT('pages.datasetDetail.linkedDataMetadataAuto')}</p>
          <div class="meta-readonly-rows">
            <div class="meta-readonly-row"><span class="meta-key">dct:created</span><code class="meta-val">{editingAsset.created_at}</code></div>
            {#if editingAsset.updated_at}<div class="meta-readonly-row"><span class="meta-key">dct:modified</span><code class="meta-val">{editingAsset.updated_at}</code></div>{/if}
            <div class="meta-readonly-row"><span class="meta-key">dcat:mediaType</span><code class="meta-val">{editingAsset.content_type}</code></div>
            <div class="meta-readonly-row"><span class="meta-key">dcat:byteSize</span><code class="meta-val">{editingAsset.size_bytes}</code></div>
            <div class="meta-readonly-row"><span class="meta-key">dcat:downloadURL</span><code class="meta-val">{editingAsset.iri || assetIri(editingAsset)}</code></div>
          </div>
          <p class="meta-readonly-hint"><Info size={12} /> {$i18nT('pages.datasetDetail.metaReadonlyHint')}</p>
        </div>
      </div>
      <div class="modal-footer">
        <button class="btn btn-ghost" on:click={() => editingAsset = null}>{$i18nT('system.cancel')}</button>
        <button class="btn" on:click={saveAssetMetadata} disabled={assetEditSaving}>
          {#if assetEditSaving}<Loader2 size={14} class="animate-spin" /> {$i18nT('pages.datasetDetail.saving')}{:else}<Check size={14} /> {$i18nT('pages.datasetDetail.saveMetadata')}{/if}
        </button>
      </div>
    </div>
  </div>
{/if}

<style>
  .detail-stack {
    display: flex;
    flex-direction: column;
    gap: 1.5rem;
  }

  .graphs-head { display: flex; align-items: center; justify-content: space-between; margin-bottom: 0.5rem; }
  .graphs-head h3 { margin: 0; }

  /* User access list */
  .access-subhead {
    font-size: 0.78rem; text-transform: uppercase; letter-spacing: 0.05em;
    color: var(--ink-500, #64748b); margin: 1rem 0 0.4rem; font-weight: 700;
  }
  .access-subhead-row {
    display: flex; align-items: center; justify-content: space-between;
    gap: 0.5rem; margin: 1rem 0 0.5rem;
  }
  .access-subhead-row .access-subhead { margin: 0; }
  .access-list { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 0.35rem; }
  .access-row {
    display: flex; align-items: center; justify-content: space-between; gap: 0.75rem;
    padding: 0.5rem 0.65rem;
    border: 1px solid var(--line-soft, #e5e7eb);
    border-radius: 8px;
    background: var(--bg-subtle, #f8fafc);
    flex-wrap: wrap;
  }
  .access-principal { display: inline-flex; align-items: center; gap: 0.55rem; min-width: 0; flex: 1 1 220px; }
  .access-principal :global(.avatar) { flex-shrink: 0; }
  .access-row .grant-select { flex-shrink: 0; min-width: 120px; }
  .access-principal-text { display: flex; flex-direction: column; gap: 0.2rem; min-width: 0; }
  .access-name { font-size: 0.9rem; font-weight: 600; color: var(--ink-800, #1e293b); }
  .access-sources { display: flex; flex-wrap: wrap; align-items: center; gap: 0.3rem; }
  .access-empty, .access-row .muted { color: var(--ink-400, #94a3b8); }
  .access-empty { font-style: italic; font-size: 0.85rem; padding: 0.3rem 0; }
  .access-row .muted { font-size: 0.76rem; }

  .eff-badge {
    font-size: 0.7rem; font-weight: 700; text-transform: uppercase; letter-spacing: 0.03em;
    padding: 0.1rem 0.4rem; border-radius: 6px;
  }
  .eff-viewer { background: #f1f5f9; color: #475569; }
  .eff-editor { background: #dbeafe; color: #1e40af; }
  .eff-admin  { background: #fef3c7; color: #92400e; }
  .eff-none   { background: #fee2e2; color: #991b1b; }

  .src-chip {
    font-size: 0.68rem; padding: 0.08rem 0.38rem; border-radius: 10px;
    display: inline-flex; align-items: center; gap: 0.2rem;
    border: 1px solid transparent;
  }
  .src-direct  { background: #eef2ff; color: #4338ca; border-color: #e0e7ff; }
  .src-inherit { background: #f8fafc; color: #64748b; border-color: #e2e8f0; }
  .src-team    { background: #f5f3ff; color: #6d28d9; border-color: #ede9fe; }
  .src-owner   { background: #fef3c7; color: #92400e; border-color: #fde68a; }
  .src-note    { font-size: 0.66rem; font-style: italic; color: #b45309; }

  .owner-pill {
    flex-shrink: 0;
    font-size: 0.75rem; font-weight: 600; color: #92400e;
    background: #fef3c7; border: 1px solid #fde68a; border-radius: 6px;
    padding: 0.25rem 0.6rem;
  }

  .grant-select {
    font-size: 0.78rem; padding: 0.25rem 0.4rem;
    border: 1px solid var(--line-soft, #e5e7eb); border-radius: 6px;
    background: white; cursor: pointer;
  }
  .grant-select:disabled { opacity: 0.5; cursor: progress; }

  .rdf-upload-form { background: var(--bg-subtle, #f8f9fa); border-radius: 8px; padding: 0.75rem; margin-bottom: 0.75rem; }
  .upload-row { display: flex; align-items: center; gap: 0.5rem; flex-wrap: wrap; }
  .graph-select { flex: 1; min-width: 160px; font-size: 0.85rem; padding: 0.3rem 0.5rem; border: 1px solid var(--line-soft, #e0e0e0); border-radius: 6px; }
  .graph-iri-input { flex: 1; min-width: 160px; font-size: 0.85rem; padding: 0.3rem 0.5rem; }
  .upload-error { color: #d94a4a; font-size: 0.82rem; margin: 0.4rem 0 0; }
  .upload-success { background: #d4edda; color: #155724; font-size: 0.85rem; padding: 0.4rem 0.75rem; border-radius: 6px; margin-bottom: 0.5rem; }

  .toggle { position: relative; display: inline-flex; align-items: center; gap: 0.4rem; cursor: pointer; user-select: none; }
  .toggle input { position: absolute; opacity: 0; width: 0; height: 0; }
  .toggle-track { width: 36px; height: 20px; background: #ccc; border-radius: 10px; transition: background 0.2s; position: relative; flex-shrink: 0; }
  .toggle input:checked + .toggle-track { background: var(--brand-600, #4a90d9); }
  .toggle-thumb { position: absolute; top: 3px; left: 3px; width: 14px; height: 14px; background: #fff; border-radius: 50%; transition: transform 0.2s; box-shadow: 0 1px 3px rgba(0,0,0,0.2); }
  .toggle input:checked + .toggle-track .toggle-thumb { transform: translateX(16px); }
  .toggle-text { font-size: 0.82rem; color: var(--ink-600, #555); }

  .breadcrumb {
    display: none;
  }

  .ds-title {
    font-size: 1.6rem;
    font-weight: 700;
    margin: 0 0 0.35rem 0;
    color: var(--ink-900);
    line-height: 1.2;
  }
  .ds-title-row {
    display: flex;
    align-items: center;
    gap: 0.6rem;
    flex-wrap: wrap;
  }
  .ds-title-row .ds-title { margin-bottom: 0; }
  .version-picker {
    font-size: 0.82rem;
    font-weight: 600;
    color: var(--ink-700, #334155);
    padding: 0.2rem 0.5rem;
    border: 1px solid var(--line-soft, #e2e8f0);
    border-radius: 999px;
    background: var(--bg-subtle, #f8fafc);
    cursor: pointer;
  }
  .version-picker.viewing {
    color: #92400e;
    background: #fef3c7;
    border-color: #fde68a;
  }
  .version-view-banner {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 1rem;
    background: #fffbeb;
    border: 1px solid #fde68a;
  }
  .vvb-text {
    display: inline-flex;
    align-items: center;
    gap: 0.5rem;
    font-size: 0.88rem;
    color: #92400e;
  }
  /* Dataset hero: animated/preset/uploaded banner behind a liquid-glass header,
     mirroring the organisation cover so the two page types feel consistent. */
  .ds-cover {
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
  }
  .ds-hero {
    position: relative;
    z-index: 1;
    width: min(760px, 100%);
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
  .ds-hero-main {
    display: flex;
    align-items: flex-start;
    gap: 0.85rem;
    flex: 1;
    min-width: 0;
  }
  .ds-hero-text { min-width: 0; }
  .ds-hero .ds-title { color: #fff; }
  .ds-hero .ds-image { border-color: rgba(255, 255, 255, 0.25); }
  .ds-hero-meta {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 0.45rem;
    margin: 0.1rem 0 0;
  }
  .ds-hero-desc {
    margin: 0.4rem 0 0;
    color: rgba(255, 255, 255, 0.84);
    font-size: 0.9rem;
    line-height: 1.5;
    max-width: 64ch;
  }
  .ds-hero-actions {
    display: flex;
    gap: 0.4rem;
    flex-shrink: 0;
    align-items: flex-end;
    flex-wrap: wrap;
  }
  .ds-hero :global(.btn-ghost) { color: rgba(255, 255, 255, 0.92); }
  .ds-hero :global(.btn-ghost:hover) { background: rgba(255, 255, 255, 0.14); }
  :global(.ds-hero-actions a.btn), :global(.ds-hero-actions .btn) {
    display: inline-flex;
    align-items: center;
    gap: 0.3rem;
    white-space: nowrap;
  }

  h2 { margin-top: 0; }
  h3 { margin-top: 0; }
  .meta { color: #666; font-size: 0.9rem; }
  .vis { padding: 0.15rem 0.5rem; border-radius: 3px; font-size: 0.8rem; }
  .vis-public { background: #d4edda; color: #155724; }
  .vis-members { background: #fff3cd; color: #856404; }
  .vis-private { background: #f8d7da; color: #721c24; }

  /* About / metadata card */
  .about-desc {
    margin: 0 0 1rem;
    font-size: 0.95rem;
    line-height: 1.55;
    color: var(--ink-700);
  }
  .meta-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(240px, 1fr));
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
    display: flex; flex-wrap: wrap; align-items: baseline; gap: 0.35rem;
    min-width: 0; word-break: break-word;
  }
  .md-pill {
    background: var(--bg-subtle, #eef2f7); border: 1px solid var(--line-soft, #e2e8f0);
    border-radius: 6px; padding: 0.1rem 0.45rem; font-weight: 600; font-size: 0.84rem;
  }
  .md-status {
    background: #e0f2fe; color: #075985; border-radius: 999px;
    padding: 0.12rem 0.6rem; font-weight: 600; font-size: 0.82rem;
  }
  .md-sub { font-size: 0.78rem; color: var(--ink-400); }
  .md-link { color: var(--brand-600, #2563eb); text-decoration: none; display: inline-flex; align-items: center; gap: 0.25rem; word-break: break-all; }
  .md-link:hover { text-decoration: underline; }
  .md-ext { color: var(--brand-600, #2563eb); display: inline-flex; align-items: center; }
  .chips-row { gap: 0.3rem; }
  .md-chip {
    display: inline-flex; align-items: center; gap: 0.25rem;
    background: var(--bg-subtle, #eef2f7); border: 1px solid var(--line-soft, #e2e8f0);
    border-radius: 999px; padding: 0.12rem 0.5rem; font-size: 0.78rem; color: var(--ink-700);
  }
  .contact-dd { flex-direction: column; align-items: flex-start; gap: 0.2rem; }
  .contact-name { font-weight: 600; }
  .about-empty {
    margin: 0.5rem 0 0; font-size: 0.86rem; color: var(--ink-500);
    line-height: 1.5;
  }

  .ds-image {
    width: 56px;
    height: 56px;
    object-fit: cover;
    border-radius: 12px;
    border: 1px solid var(--line-soft);
    flex-shrink: 0;
  }
  .conformance-row {
    display: flex;
    flex-direction: column;
    gap: 0.3rem;
  }
  .edit-label {
    font-size: 0.82rem;
    font-weight: 500;
    color: var(--ink-600, #555);
    display: flex;
    align-items: center;
    gap: 0.3rem;
  }
  .conformance-badge {
    display: flex;
    align-items: center;
    gap: 0.35rem;
    font-size: 0.82rem;
    color: var(--ink-500, #777);
    margin-top: 0.2rem;
  }
  .onto-ver { font-size: 0.75rem; color: var(--ink-400, #9ca3af); }
  .svc-inactive { opacity: 0.55; }
  .svc-badge {
    display: inline-block;
    padding: 0.15rem 0.5rem;
    border-radius: 20px;
    font-size: 0.74rem;
    font-weight: 600;
  }
  .svc-badge-active { background: #d4edda; color: #155724; }
  .svc-badge-inactive { background: #f5f5f5; color: #616161; }
  .svc-actions {
    display: flex;
    gap: 0.25rem;
    white-space: nowrap;
  }
  .inline-edit-input { width: 100%; font-size: 0.88rem; }
  :global(.power-on) { color: #2d7d46; }
  :global(.power-off) { color: #999; }
  .muted { color: var(--ink-400); }
  .btn-active { background: var(--brand-100, #e3f0fc); color: var(--brand-700, #1d5ea8); }

  .svc-graph-panel { padding: 0 !important; background: var(--bg-subtle, #f8f9fa); }
  .svc-graph-panel-inner { padding: 0.75rem 1rem; }
  .svc-graph-panel-header { display: flex; align-items: center; justify-content: space-between; margin-bottom: 0.5rem; }
  .svc-graph-list { display: flex; flex-direction: column; gap: 0.15rem; max-height: 240px; overflow-y: auto; }
  .svc-graph-item { display: flex; align-items: center; gap: 0.5rem; cursor: pointer; padding: 0.25rem 0.4rem; border-radius: 4px; }
  .svc-graph-item:hover { background: var(--bg-hover, #eef2f6); }

  .inline-form {
    display: flex;
    gap: 0.5rem;
    margin-bottom: 0.75rem;
  }
  .inline-form input { width: auto; flex: 1; }

  .graph-list {
    list-style: none;
    padding: 0;
  }
  .graph-list li {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 0.35rem 0;
    border-bottom: 1px solid #eee;
  }
  .graph-item { flex-wrap: wrap; gap: 0.4rem; }
  .graph-item-main { display: flex; align-items: center; gap: 0.5rem; flex: 1; min-width: 0; }
  .graph-iri { font-size: 0.8rem; word-break: break-all; }
  .graph-item-actions { display: flex; align-items: center; gap: 0.35rem; flex-shrink: 0; }
  .graph-role-badge {
    display: inline-flex; align-items: center; gap: 0.2rem;
    padding: 0.1rem 0.45rem; border-radius: 999px;
    font-size: 0.7rem; font-weight: 600; white-space: nowrap;
  }
  .role-model      { background: #dcfce7; color: #15803d; }
  .role-vocabulary { background: #fdf4ff; color: #7e22ce; }
  .role-shapes     { background: #fef9c3; color: #854d0e; }
  .role-entailment { background: #ede9fe; color: #5b21b6; }
  .role-instances  { background: #dbeafe; color: #1e40af; }
  .role-system     { background: #f1f5f9; color: #475569; }
  .graph-private-badge {
    display: inline-flex; align-items: center; gap: 0.2rem;
    padding: 0.1rem 0.45rem; border-radius: 999px;
    font-size: 0.7rem; font-weight: 600; white-space: nowrap;
    background: #fee2e2; color: #b91c1c;
  }
  .role-select {
    padding: 0.2rem 0.4rem; border: 1px solid var(--line-soft, #d1d5db);
    border-radius: 6px; font-size: 0.78rem; cursor: pointer;
  }
  .empty { color: #888; font-style: italic; }
  .endpoint-cell { display: flex; align-items: center; gap: 0.4rem; max-width: 480px; }
  .endpoint-url { font-family: 'SF Mono', 'Fira Code', monospace; font-size: 0.78rem; color: #444; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; flex: 1; min-width: 0; }
  .copy-btn { padding: 0.15rem 0.35rem; flex-shrink: 0; }
  .btn-xs { font-size: 0.75rem; padding: 0.15rem 0.4rem; }

  .report { padding: 0.75rem; border-radius: 6px; margin-top: 0.5rem; }
  .report.conforms { background: #d4edda; }
  .report:not(.conforms) { background: #f8d7da; }
  .run-meta { font-weight: 400; font-size: 0.78rem; opacity: 0.75; }
  .validation-error { margin: 0.5rem 0 0; }

  .val-history { margin-top: 0.75rem; }
  .val-history-list { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 0.25rem; }
  .val-history-item { display: flex; align-items: center; gap: 0.5rem; font-size: 0.78rem; }
  .vh-pill { display: inline-flex; align-items: center; gap: 3px; font-size: 0.68rem; padding: 1px 7px; border-radius: 999px; font-weight: 600; white-space: nowrap; }
  .vh-ok { background: #dcfce7; color: #15803d; }
  .vh-fail { background: #fee2e2; color: #b91c1c; }
  .vh-time { color: var(--ink-400, #9ca3af); }
  :global(:is([data-theme="dark"], .dark)) .vh-ok { background: rgba(16,185,129,0.18); color: #6ee7b7; }
  :global(:is([data-theme="dark"], .dark)) .vh-fail { background: rgba(239,68,68,0.18); color: #fca5a5; }

  .shapes-studio-note { display: flex; align-items: center; gap: 0.5rem; font-size: 0.85rem; }
  .shapes-studio-note span { flex: 1; min-width: 0; }
  :global(.shapes-studio-link) { flex-shrink: 0; }

  .effective-shapes { margin: 0.25rem 0 0.75rem; padding: 0.6rem 0.75rem; border: 1px solid var(--line-soft, #e5e7eb); border-radius: 8px; background: var(--bg-soft, #f8fafc); }
  .eff-head { font-size: 0.8rem; font-weight: 600; display: flex; flex-wrap: wrap; align-items: baseline; gap: 0.4rem; margin-bottom: 0.4rem; }
  .eff-hint { font-weight: 400; font-size: 0.7rem; color: var(--ink-400, #9ca3af); }
  .eff-list { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 0.3rem; }
  .eff-item { display: flex; align-items: center; gap: 0.4rem; font-size: 0.8rem; }
  :global(.eff-name) { font-weight: 600; color: var(--brand-600, #4f46e5); text-decoration: none; }
  :global(.eff-name:hover) { text-decoration: underline; }
  .eff-badge { font-size: 0.62rem; padding: 0.05rem 0.35rem; border-radius: 4px; text-transform: capitalize; }
  .eff-dataset { background: #e0e7ff; color: #3730a3; }
  .eff-inherited { background: #dcfce7; color: #166534; }
  .eff-status-draft { background: #f3f4f6; color: #6b7280; }
  .eff-status-staged { background: #fef3c7; color: #92400e; }
  .eff-status-published { background: #dcfce7; color: #166534; }
  .eff-status-deprecated { background: #fee2e2; color: #991b1b; }

  .sev { padding: 0.1rem 0.4rem; border-radius: 3px; font-size: 0.75rem; }
  .sev-violation { background: #f8d7da; color: #721c24; }
  .sev-warning { background: #fff3cd; color: #856404; }
  .sev-info { background: #d1ecf1; color: #0c5460; }

  .explore-card { padding-bottom: 0.25rem; }

  .linked-onto-head { display: flex; align-items: center; gap: 0.4rem; margin-bottom: 0.5rem; }
  .muted-tip { font-size: 0.78rem; color: #94a3b8; }
  .linked-row { display: flex; align-items: center; gap: 0.5rem; flex-wrap: wrap; padding: 0.4rem 0; border-top: 1px solid #f1f5f9; }
  .linked-row:first-of-type { border-top: none; }
  .linked-badge { font-size: 0.68rem; font-weight: 700; text-transform: uppercase; letter-spacing: 0.04em; padding: 1px 7px; border-radius: 999px; }
  .linked-badge.conforms { background: #dcfce7; color: #15803d; }
  .linked-badge.detected { background: #fef3c7; color: #92400e; }
  :global(.linked-row .onto-title) { font-weight: 600; color: #1565c0; text-decoration: none; }
  :global(.linked-row .onto-title:hover) { text-decoration: underline; }
  .onto-ns { font-family: monospace; font-size: 0.72rem; color: #6a5acd; }
  .link-btn { margin-left: auto; }

  .explore-head {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    margin-bottom: 0.9rem;
  }
  .explore-head h3 { margin: 0; }

  .explore-actions {
    display: grid;
    grid-template-columns: repeat(5, minmax(0, 1fr));
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
  :global(.action-tile strong) {
    font-size: 0.9rem;
    color: var(--ink-900);
  }
  :global(.action-tile span) {
    font-size: 0.78rem;
    color: var(--ink-500);
    line-height: 1.4;
  }
  :global(.action-tile svg) {
    color: var(--brand-500);
    margin-bottom: 0.2rem;
  }

  .assets-head {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 0.75rem;
  }

  .progress-bar {
    height: 4px;
    background: var(--line-soft, #eee);
    border-radius: 2px;
    margin-bottom: 0.75rem;
    overflow: hidden;
  }
  .progress-fill {
    height: 100%;
    background: var(--brand-500, #4f46e5);
    transition: width 0.1s ease;
  }

  .asset-name {
    display: inline-flex;
    align-items: center;
    gap: 0.3rem;
    font-size: 0.88rem;
  }
  .media-type {
    font-size: 0.75rem;
    color: var(--ink-500, #666);
  }
  .asset-size {
    font-size: 0.82rem;
    color: var(--ink-500, #666);
    white-space: nowrap;
  }
  .asset-actions {
    display: flex;
    gap: 0.2rem;
    align-items: center;
    white-space: nowrap;
  }

  .assets-error {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.5rem;
    background: #fef2f2;
    border: 1px solid #fecaca;
    color: #991b1b;
    border-radius: 6px;
    padding: 0.5rem 0.75rem;
    font-size: 0.875rem;
    margin-bottom: 0.75rem;
  }

  .visibility-btn {
    display: inline-flex;
    align-items: center;
    gap: 0.25rem;
    font-size: 0.73rem;
    font-weight: 600;
    border-radius: 20px;
    padding: 0.15rem 0.5rem;
    border: 1px solid transparent;
    cursor: pointer;
  }
  .vis-public {
    background: #d4edda;
    color: #155724;
    border-color: #c3e6cb;
  }
  .vis-private {
    background: #f5f5f5;
    color: #555;
    border-color: #ddd;
  }
  .vis-public:hover { background: #c3e6cb; }
  .vis-private:hover { background: #e8e8e8; }

  /* Preview modal */
  .modal-backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0,0,0,0.55);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 1000;
    padding: 1.5rem;
  }
  .preview-modal {
    background: var(--surface, #fff);
    border-radius: 12px;
    box-shadow: 0 20px 60px rgba(0,0,0,0.25);
    display: flex;
    flex-direction: column;
    width: min(92vw, 960px);
    max-height: min(90vh, 800px);
    overflow: hidden;
  }
  .preview-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 0.85rem 1rem;
    border-bottom: 1px solid var(--line-soft, #eee);
    flex-shrink: 0;
  }
  .preview-title {
    display: inline-flex;
    align-items: center;
    gap: 0.4rem;
    font-weight: 600;
    font-size: 0.92rem;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .preview-header-actions {
    display: flex;
    gap: 0.3rem;
    flex-shrink: 0;
  }
  .preview-body {
    flex: 1;
    overflow: auto;
    display: flex;
    flex-direction: column;
    min-height: 0;
  }
  .preview-image {
    max-width: 100%;
    max-height: 100%;
    object-fit: contain;
    margin: auto;
    display: block;
    padding: 1rem;
  }
  .preview-embed {
    width: 100%;
    flex: 1;
    border: none;
    min-height: 560px;
  }
  .preview-audio-wrap {
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 3rem 2rem;
  }
  .preview-audio { width: 100%; max-width: 480px; }
  .preview-video {
    width: 100%;
    max-height: 60vh;
    background: #000;
  }
  .preview-text {
    font-family: 'SF Mono', 'Fira Code', monospace;
    font-size: 0.82rem;
    line-height: 1.6;
    padding: 1rem 1.25rem;
    margin: 0;
    overflow: auto;
    white-space: pre-wrap;
    word-break: break-word;
    flex: 1;
  }

  .table-scroll {
    overflow: auto;
    max-height: 22rem;
  }

  .preview-loading {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 2rem;
    color: var(--ink-500);
    font-size: 0.9rem;
  }

  .preview-error {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 1rem;
    padding: 2.5rem;
    color: #991b1b;
  }

  .preview-text--md {
    white-space: pre-wrap;
    word-break: break-word;
  }

  @media (max-width: 900px) {
    .explore-actions {
      grid-template-columns: repeat(3, minmax(0, 1fr));
    }
  }
  @media (max-width: 600px) {
    .explore-actions {
      grid-template-columns: repeat(2, minmax(0, 1fr));
    }
  }

  /* ── Section head (shared across cards) ──────────────────────────────────── */
  .section-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 1rem;
  }
  .section-head-left {
    display: flex;
    align-items: center;
    gap: 0.45rem;
  }
  .section-head-left h3 { margin: 0; }
  .section-head-left :global(svg) { color: var(--ink-500); }

  /* ── SPARQL service cards ─────────────────────────────────────────────────── */
  .svc-list { display: flex; flex-direction: column; gap: 0.75rem; }

  .svc-card {
    border: 1px solid var(--line-soft, #e5e7eb);
    border-radius: 12px;
    padding: 0.85rem 1rem;
    background: var(--bg-subtle, #fafafa);
    transition: border-color 0.15s;
  }
  .svc-card:hover { border-color: var(--brand-200, #a5c8f2); }
  .svc-card-inactive { opacity: 0.6; }

  .svc-card-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.75rem;
    flex-wrap: wrap;
  }
  .svc-card-meta {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    flex-wrap: wrap;
    flex: 1;
    min-width: 0;
  }
  .svc-name { font-weight: 600; font-size: 0.92rem; }
  .svc-slug {
    font-size: 0.77rem;
    color: var(--ink-500);
    background: var(--bg-accent, #f0f4f8);
    padding: 0.1rem 0.4rem;
    border-radius: 4px;
  }
  .svc-name-input { font-weight: 600; max-width: 240px; }

  .svc-card-actions {
    display: flex;
    align-items: center;
    gap: 0.3rem;
    flex-shrink: 0;
  }
  .svc-endpoint-row {
    display: flex;
    align-items: center;
    gap: 0.25rem;
    max-width: 380px;
    overflow: hidden;
  }
  .svc-endpoint-row .endpoint-url {
    font-size: 0.73rem;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .svc-desc { font-size: 0.85rem; color: var(--ink-500); margin: 0.3rem 0 0; }

  /* ── Service graph section (inside service card) ────────────────────────── */
  .svc-graph-section { margin-top: 0.65rem; border-top: 1px solid var(--line-soft, #eee); padding-top: 0.5rem; }

  .svc-graph-toggle {
    display: flex;
    align-items: center;
    gap: 0.35rem;
    background: none;
    border: none;
    cursor: pointer;
    font-size: 0.82rem;
    color: var(--ink-500);
    padding: 0.2rem 0.3rem;
    border-radius: 4px;
    width: 100%;
    text-align: left;
  }
  .svc-graph-toggle:hover { background: var(--bg-hover, #eef2f6); color: var(--ink-800); }

  .svc-graph-count-hint {
    font-size: 0.78rem;
    color: var(--ink-400);
    margin-left: auto;
  }
  .svc-graph-chevron {
    margin-left: auto;
    font-size: 1.1rem;
    line-height: 1;
    transition: transform 0.15s;
    color: var(--ink-400);
  }
  .svc-graph-chevron.open { transform: rotate(90deg); }

  .svc-graph-panel-inner { padding: 0.5rem 0; }
  .svc-graph-panel-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 0.4rem;
  }
  .svc-graph-panel-label { font-size: 0.8rem; color: var(--ink-500); }
  .svc-graph-bulk { display: flex; gap: 0.25rem; }
  .svc-graph-loading { display: flex; align-items: center; gap: 0.4rem; font-size: 0.82rem; color: var(--ink-400); padding: 0.3rem 0; }
  .svc-graph-empty { font-size: 0.82rem; color: var(--ink-400); margin: 0; }

  .svc-graph-list { display: flex; flex-direction: column; gap: 0.1rem; max-height: 220px; overflow-y: auto; }
  .svc-graph-item {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    cursor: pointer;
    padding: 0.25rem 0.4rem;
    border-radius: 6px;
    font-size: 0.82rem;
  }
  .svc-graph-item:hover { background: var(--bg-hover, #eef2f6); }
  .svc-graph-item.checked { background: #f0fdf4; }
  .svc-graph-iri { font-size: 0.75rem; color: var(--ink-700); word-break: break-all; white-space: normal; flex: 1; }
  .svc-graph-tick { color: #16a34a; flex-shrink: 0; }

  /* ── Add Service modal specifics ─────────────────────────────────────────── */
  .slug-row {
    display: flex;
    align-items: center;
    gap: 0.2rem;
    flex-wrap: wrap;
  }
  .slug-prefix, .slug-suffix { font-size: 0.78rem; color: var(--ink-400); background: var(--bg-subtle); padding: 0.3rem 0.4rem; border-radius: 4px; white-space: nowrap; }
  .slug-row input { flex: 1; min-width: 120px; }
  .field-hint { font-size: 0.78rem; color: var(--ink-400); font-weight: 400; }
  .field-hint-text { font-size: 0.8rem; color: var(--ink-400); margin: 0.25rem 0 0; }
  .field-empty-hint { font-size: 0.82rem; color: var(--ink-400); font-style: italic; }
  .svc-graph-picker { border: 1px solid var(--line-soft); border-radius: 8px; overflow: hidden; }
  .svc-graph-picker-head { display: flex; gap: 0.25rem; padding: 0.4rem 0.5rem; background: var(--bg-subtle); border-bottom: 1px solid var(--line-soft); }
  .svc-graph-count { font-size: 0.78rem; color: var(--ink-400); padding: 0.35rem 0.5rem; margin: 0; background: var(--bg-subtle); border-top: 1px solid var(--line-soft); }

  /* ── Modals (shared base) ────────────────────────────────────────────────── */
  .modal-backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0,0,0,0.4);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 200;
    padding: 1.5rem;
  }
  .modal-box {
    background: white;
    border-radius: 1rem;
    width: min(520px, calc(100vw - 2rem));
    max-height: min(90vh, 800px);
    display: flex;
    flex-direction: column;
    box-shadow: 0 20px 60px rgba(0,0,0,0.18);
    overflow: hidden;
  }
  .modal-lg { width: min(680px, calc(100vw - 2rem)); }
  .modal-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 1rem 1.25rem 0.75rem;
    border-bottom: 1px solid var(--line-soft);
    flex-shrink: 0;
  }
  .modal-header h3 { margin: 0; display: flex; align-items: center; gap: 0.4rem; font-size: 1rem; }
  .modal-body { padding: 1rem 1.25rem; overflow-y: auto; flex: 1; }
  .modal-footer {
    display: flex;
    justify-content: flex-end;
    gap: 0.5rem;
    padding: 0.75rem 1.25rem;
    border-top: 1px solid var(--line-soft);
    flex-shrink: 0;
  }
  .form-group { display: flex; flex-direction: column; gap: 0.3rem; margin-bottom: 0.85rem; }
  .form-group label,
  .form-group .group-caption { font-size: 0.85rem; font-weight: 600; color: var(--ink-700); }
  .form-group .group-caption { display: block; }
  .form-group textarea { width: 100%; min-height: 72px; resize: vertical; }

  /* ── Asset metadata modal ─────────────────────────────────────────────────── */
  .asset-edit-filename {
    display: flex;
    align-items: center;
    gap: 0.35rem;
    font-size: 0.85rem;
    color: var(--ink-500);
    background: var(--bg-subtle);
    padding: 0.4rem 0.6rem;
    border-radius: 6px;
    margin-bottom: 1rem;
  }
  .metadata-readonly-group { margin-top: 0.5rem; }
  .meta-readonly-label { font-size: 0.78rem; font-weight: 600; color: var(--ink-400); text-transform: uppercase; letter-spacing: 0.05em; margin: 0 0 0.4rem; }
  .meta-readonly-rows { display: flex; flex-direction: column; gap: 0.2rem; border: 1px solid var(--line-soft); border-radius: 6px; overflow: hidden; }
  .meta-readonly-row { display: flex; align-items: baseline; gap: 0.5rem; padding: 0.3rem 0.6rem; font-size: 0.8rem; background: var(--bg-subtle); border-bottom: 1px solid var(--line-soft, #eee); }
  .meta-readonly-row:last-child { border-bottom: none; }
  .meta-key { font-family: monospace; font-size: 0.78rem; color: var(--brand-600, #4a90d9); min-width: 120px; flex-shrink: 0; }
  .meta-val { font-size: 0.77rem; color: var(--ink-600); word-break: break-all; }
  .meta-readonly-hint { display: flex; align-items: center; gap: 0.3rem; font-size: 0.77rem; color: var(--ink-400); margin: 0.4rem 0 0; }

  /* ── Asset cards ─────────────────────────────────────────────────────────── */
  .asset-cards { display: flex; flex-direction: column; gap: 0.5rem; }
  .asset-card {
    display: flex;
    align-items: flex-start;
    gap: 0.75rem;
    padding: 0.75rem 0.85rem;
    border: 1px solid var(--line-soft, #e5e7eb);
    border-radius: 10px;
    background: var(--bg-subtle, #fafafa);
    transition: border-color 0.15s;
  }
  .asset-card:hover { border-color: var(--brand-200, #a5c8f2); }
  .asset-card-icon { color: var(--ink-300); flex-shrink: 0; margin-top: 0.1rem; }
  .asset-card-body { flex: 1; min-width: 0; }
  .asset-card-title { font-weight: 600; font-size: 0.88rem; display: flex; align-items: baseline; gap: 0.4rem; flex-wrap: wrap; }
  .asset-filename-sub { font-weight: 400; font-size: 0.78rem; color: var(--ink-400); font-family: monospace; }
  .asset-card-desc { font-size: 0.82rem; color: var(--ink-500); margin: 0.2rem 0 0; }
  .asset-card-meta {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    flex-wrap: wrap;
    margin-top: 0.3rem;
    font-size: 0.78rem;
    color: var(--ink-400);
  }
  .asset-created { font-size: 0.77rem; color: var(--ink-400); }
  .asset-card-actions { display: flex; align-items: center; gap: 0.2rem; flex-shrink: 0; flex-wrap: wrap; }

  /* ── Validation hint ─────────────────────────────────────────────────────── */
  .hint-text { font-size: 0.85rem; color: var(--ink-400); margin: 0; }

  /* ─── Dark theme overrides ───────────────────────────────────────────────── */
  /* var(--bg-subtle/-hover) surfaces flip via theme.css; these rules re-map the
     remaining hardcoded badges, banners, modals and role colour-sets. */
  :global(:is([data-theme="dark"], .dark)) .eff-viewer { background: rgba(255,255,255,0.06); color: var(--ink-600); }
  :global(:is([data-theme="dark"], .dark)) .eff-editor,
  :global(:is([data-theme="dark"], .dark)) .role-instances { background: rgba(59,130,246,0.2); color: #93c5fd; }
  :global(:is([data-theme="dark"], .dark)) .eff-admin { background: rgba(245,158,11,0.18); color: #fcd34d; }
  :global(:is([data-theme="dark"], .dark)) .eff-none { background: rgba(220,38,38,0.18); color: #fca5a5; }
  :global(:is([data-theme="dark"], .dark)) .src-direct { background: rgba(99,102,241,0.18); color: #c7d2fe; border-color: rgba(99,102,241,0.3); }
  :global(:is([data-theme="dark"], .dark)) .src-inherit { background: rgba(255,255,255,0.04); color: var(--ink-500); border-color: var(--line-soft); }
  :global(:is([data-theme="dark"], .dark)) .src-team { background: rgba(124,58,237,0.18); color: #c4b5fd; border-color: rgba(124,58,237,0.3); }
  :global(:is([data-theme="dark"], .dark)) .src-owner,
  :global(:is([data-theme="dark"], .dark)) .owner-pill,
  :global(:is([data-theme="dark"], .dark)) .version-picker.viewing { background: rgba(245,158,11,0.18); color: #fcd34d; border-color: rgba(245,158,11,0.4); }
  :global(:is([data-theme="dark"], .dark)) .src-note,
  :global(:is([data-theme="dark"], .dark)) .vvb-text { color: #fcd34d; }
  :global(:is([data-theme="dark"], .dark)) .version-view-banner { background: rgba(245,158,11,0.12); border-color: rgba(245,158,11,0.4); }
  :global(:is([data-theme="dark"], .dark)) .meta { color: var(--ink-500); }
  :global(:is([data-theme="dark"], .dark)) .md-status { background: rgba(59,130,246,0.18); color: #93c5fd; }

  :global(:is([data-theme="dark"], .dark)) .vis-public,
  :global(:is([data-theme="dark"], .dark)) .svc-badge-active,
  :global(:is([data-theme="dark"], .dark)) .upload-success,
  :global(:is([data-theme="dark"], .dark)) .role-model,
  :global(:is([data-theme="dark"], .dark)) .linked-badge.conforms { background: rgba(16,185,129,0.18); color: #6ee7b7; border-color: rgba(16,185,129,0.35); }
  :global(:is([data-theme="dark"], .dark)) .vis-public:hover { background: rgba(16,185,129,0.26); }
  :global(:is([data-theme="dark"], .dark)) .vis-members { background: rgba(245,158,11,0.18); color: #fcd34d; }
  :global(:is([data-theme="dark"], .dark)) .vis-private { background: rgba(255,255,255,0.06); color: var(--ink-600); border-color: var(--line-strong); }
  :global(:is([data-theme="dark"], .dark)) .vis-private:hover { background: rgba(255,255,255,0.1); }
  :global(:is([data-theme="dark"], .dark)) .svc-badge-inactive,
  :global(:is([data-theme="dark"], .dark)) .role-system { background: rgba(255,255,255,0.06); color: var(--ink-500); }
  :global(:is([data-theme="dark"], .dark)) .power-on,
  :global(:is([data-theme="dark"], .dark)) .svc-graph-tick { color: #4ade80; }
  :global(:is([data-theme="dark"], .dark)) .power-off,
  :global(:is([data-theme="dark"], .dark)) .empty { color: var(--ink-500); }
  :global(:is([data-theme="dark"], .dark)) .endpoint-url { color: var(--ink-700); }

  :global(:is([data-theme="dark"], .dark)) .graph-list li { border-bottom-color: var(--line-soft); }
  :global(:is([data-theme="dark"], .dark)) .role-vocabulary { background: rgba(168,85,247,0.18); color: #d8b4fe; }
  :global(:is([data-theme="dark"], .dark)) .role-shapes { background: rgba(234,179,8,0.2); color: #fde047; }
  :global(:is([data-theme="dark"], .dark)) .role-entailment { background: rgba(139,92,246,0.2); color: #c4b5fd; }
  :global(:is([data-theme="dark"], .dark)) .graph-private-badge { background: rgba(220,38,38,0.18); color: #fca5a5; }

  :global(:is([data-theme="dark"], .dark)) .report.conforms { background: rgba(16,185,129,0.16); }
  :global(:is([data-theme="dark"], .dark)) .report:not(.conforms) { background: rgba(220,38,38,0.16); }
  :global(:is([data-theme="dark"], .dark)) .sev-violation { background: rgba(220,38,38,0.18); color: #fca5a5; }
  :global(:is([data-theme="dark"], .dark)) .sev-warning { background: rgba(245,158,11,0.18); color: #fcd34d; }
  :global(:is([data-theme="dark"], .dark)) .sev-info { background: rgba(6,182,212,0.16); color: #67e8f9; }

  :global(:is([data-theme="dark"], .dark)) .linked-row { border-top-color: var(--line-soft); }
  :global(:is([data-theme="dark"], .dark)) .linked-badge.detected { background: rgba(245,158,11,0.18); color: #fcd34d; }
  :global(:is([data-theme="dark"], .dark) .linked-row .onto-title) { color: #60a5fa; }
  :global(:is([data-theme="dark"], .dark)) .onto-ns { color: #a5b4fc; }

  :global(:is([data-theme="dark"], .dark)) .action-tile { background: linear-gradient(160deg, rgba(255,255,255,0.05), rgba(255,255,255,0.02)); }
  :global(:is([data-theme="dark"], .dark)) .assets-error { background: rgba(220,38,38,0.12); border-color: rgba(220,38,38,0.35); color: #fca5a5; }
  :global(:is([data-theme="dark"], .dark)) .upload-error,
  :global(:is([data-theme="dark"], .dark)) .preview-error { color: #fca5a5; }
  :global(:is([data-theme="dark"], .dark)) .toggle-track { background: rgba(255,255,255,0.2); }

  :global(:is([data-theme="dark"], .dark)) .modal-box,
  :global(:is([data-theme="dark"], .dark)) .preview-modal { background: var(--bg-strong); }
  :global(:is([data-theme="dark"], .dark)) .svc-graph-item.checked { background: rgba(16,185,129,0.12); }
</style>
