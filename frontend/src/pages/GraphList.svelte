<script lang="ts">
  import { onMount } from 'svelte';
  import { browseGraphs, deleteGraph, exportGraph, uploadToGraph, getDataset, getOrganisation, listDatasetGraphs, listDatasets, createDataset, addDatasetGraph, listOrganisations, listDatasetVersions, getDatasetVersion, updateGraphRole } from '../lib/api.js';
  import { navigate, Link } from '../lib/router/index.js';
  import { formatNumber, shortenIRI, downloadFile, normalizeGraphRole, graphRoleLabel } from '../lib/rdf-utils.js';
  import { probeContentKind } from '../lib/content-kind.js';
  import { t } from 'svelte-i18n';
  import { Plus, Search, Download, Trash2, Loader2, Network, Share2, X, Info, ChevronDown, Tag, Database, History, Sparkles, Check, ShieldCheck } from 'lucide-svelte';
  import ConfirmModal from '../components/ConfirmModal.svelte';
  import AttachShapesDialog from '../components/AttachShapesDialog.svelte';
  import PageHeader from '../components/PageHeader.svelte';
  import BulkActionBar from '../components/BulkActionBar.svelte';
  import Select from '../components/Select.svelte';
  import { isAuthenticated } from '../lib/stores.js';
  import { user as userStore } from '../lib/stores.js';
  import { autofocus } from '../lib/actions/autofocus.js';

  let backDatasetId = null;
  let backOrgId = null;
  let backContextName = null;
  let currentUser = null;
  userStore.subscribe(v => currentUser = v);

  // Version scoping (dataset context only): when pinned, list the version's
  // snapshot graphs instead of the live ones.
  let version = null;          // null/'' = live
  let versionsList = [];       // available versions for backDatasetId
  const LIVE_SENTINELS = ['', 'live', 'latest', 'current'];
  $: versionPinned = !!version && !LIVE_SENTINELS.includes(version);
  function verParts(v) {
    return String(v?.version ?? '').replace(/^v/i, '').split('.').map(n => parseInt(n, 10) || 0);
  }
  $: sortedVersionsList = versionsList.slice().sort((a, b) => {
    const pa = verParts(a), pb = verParts(b);
    for (let i = 0; i < Math.max(pa.length, pb.length); i++) {
      const x = pa[i] ?? 0, y = pb[i] ?? 0;
      if (x !== y) return y - x;
    }
    return 0;
  });
  function onPickVersion(v) {
    version = v || null;
    clearSelection();
    loadAll();
  }
  // Browse URL for a graph, carrying dataset + version so the browser can
  // authorise and scope a version snapshot graph.
  function browseGraphUrl(iri) {
    let u = `/browse?graph=${encodeURIComponent(iri)}`;
    if (backDatasetId) u += `&dataset=${backDatasetId}`;
    if (versionPinned) u += `&version=${encodeURIComponent(version)}`;
    return u;
  }

  // Normalised graph records: { iri, name, count, declaredRole }
  let graphs = [];
  let error = '';
  let loading = true;

  // ── Role auto-detection ─────────────────────────────────────────────────────
  // Declared roles (from the dataset registration) are the source of truth and
  // are attached in loadAll(). For graphs with no declared role we fall back to
  // an IRI-pattern guess and, finally, to live content probing (auto-detect).
  let detectedRoles: Map<string, string> = new Map(); // iri → canonical role
  let detecting: Set<string> = new Set();              // iris currently probing
  let detectRan = false;                               // auto-detect pass done?
  let applyingRole: string | null = null;              // iri being persisted
  let applyError = '';
  const AUTO_DETECT_LIMIT = 40;  // auto-probe only when the unknown set is small
  const DETECT_CONCURRENCY = 4;
  let confirmDelete = null;
  let attachGraphIri = null;           // graph IRI whose shapes dialog is open
  let exporting = null;
  let filterText = '';
  let showInfo = false;

  let showCreateForm = false;
  let newGraphIri = '';
  let creating = false;
  let createError = '';

  // ── Multi-select ──────────────────────────────────────────────────────────
  let selected: Set<string> = new Set();
  let confirmBulkDelete = false;
  let bulkDeleting = false;

  function toggleSelect(iri: string) {
    if (selected.has(iri)) { selected.delete(iri); } else { selected.add(iri); }
    selected = selected;
  }
  function toggleSelectAll() {
    const selectableIris = filtered.filter(g => g.iri).map(g => g.iri);
    if (selected.size === selectableIris.length && selectableIris.length > 0) {
      selected.clear();
    } else {
      selectableIris.forEach(iri => selected.add(iri));
    }
    selected = selected;
  }
  function clearSelection() { selected.clear(); selected = selected; }

  async function bulkDelete() {
    bulkDeleting = true;
    const toDelete = [...selected];
    const errors: string[] = [];
    for (const iri of toDelete) {
      try { await deleteGraph(iri); } catch (_) { errors.push(iri); }
    }
    confirmBulkDelete = false;
    bulkDeleting = false;
    clearSelection();
    if (errors.length) error = `${$t('pages.graphList.failedToDelete')} ${errors.join(', ')}`;
    await loadAll();
  }

  // ── Create dataset from selected graphs ───────────────────────────────────
  let showCreateDatasetModal = false;
  let cdName = '';
  let cdVisibility = 'private';
  let cdOwnerType = 'user';
  let cdOwnerOrgId = '';
  let cdLoading = false;
  let cdError = '';
  let organisations = [];

  async function openCreateDatasetModal() {
    cdError = '';
    cdName = '';
    cdVisibility = 'private';
    cdOwnerType = 'user';
    cdOwnerOrgId = '';
    if (!organisations.length) {
      try { organisations = await listOrganisations(); } catch {}
    }
    showCreateDatasetModal = true;
  }

  async function createDatasetFromSelected() {
    if (!cdName.trim()) return;
    cdLoading = true;
    cdError = '';
    try {
      const ds = await createDataset({
        name: cdName.trim(),
        description: null,
        visibility: cdVisibility,
        owner_type: cdOwnerType,
        owner_id: cdOwnerType === 'organisation' ? cdOwnerOrgId : currentUser?.id,
        conforms_to_ontology: null,
        conforms_to_version: null,
        graph_role: null,
      });
      // Link each selected graph to the new dataset
      const graphIris = [...selected];
      for (const iri of graphIris) {
        try { await addDatasetGraph(ds.id, { graph_iri: iri }); } catch {}
      }
      showCreateDatasetModal = false;
      clearSelection();
      navigate(`/datasets/${ds.id}`);
    } catch (e) {
      cdError = e.message || String(e);
    }
    cdLoading = false;
  }

  // ── Derived select-all state ──────────────────────────────────────────────
  $: selectableFiltered = filtered.filter(g => g.iri);
  $: allFilteredSelected = selectableFiltered.length > 0 && selectableFiltered.every(g => selected.has(g.iri));
  $: someFilteredSelected = !allFilteredSelected && selectableFiltered.some(g => selected.has(g.iri));

  // Header checkbox indeterminate binding
  let headerCheckbox: HTMLInputElement | null = null;
  $: if (headerCheckbox) {
    headerCheckbox.indeterminate = someFilteredSelected;
    headerCheckbox.checked = allFilteredSelected;
  }

  function normalise(entry) {
    if (entry === null || entry === undefined) return null;
    if (typeof entry === 'string') return { iri: entry, name: shortenIRI(entry), count: null, declaredRole: null };
    // Backend shape: { iri, name, count } (iri may be null for default graph)
    return {
      iri: entry.iri ?? entry.graph ?? null,
      name: entry.name ?? (entry.iri ? shortenIRI(entry.iri) : $t('pages.graphList.defaultGraph')),
      count: entry.count ?? null,
      declaredRole: null,
    };
  }

  // Stamp declared roles (from dataset graph registrations) onto the normalised
  // list. `roleByIri` maps graph IRI → canonical role string.
  function applyDeclaredRoles(list, roleByIri: Map<string, string>) {
    if (!roleByIri.size) return list;
    return list.map(g => g && g.iri && roleByIri.has(g.iri)
      ? { ...g, declaredRole: roleByIri.get(g.iri) }
      : g);
  }

  // Build an IRI → canonical role map from dataset graph entries.
  function rolesFromEntries(entries): Map<string, string> {
    const m = new Map<string, string>();
    for (const e of (Array.isArray(entries) ? entries : (entries?.graphs || []))) {
      if (typeof e === 'string') continue;
      const role = normalizeGraphRole(e.graph_role);
      if (e.graph_iri && role) m.set(e.graph_iri, role);
    }
    return m;
  }

  onMount(async () => {
    const params = new URLSearchParams(window.location.search);
    backDatasetId = params.get('dataset') || null;
    backOrgId = params.get('org') || null;
    version = params.get('version') || null;
    if (backDatasetId) {
      getDataset(backDatasetId).then(d => { backContextName = d?.name ?? backDatasetId; }).catch(() => { backContextName = backDatasetId; });
      listDatasetVersions(backDatasetId).then(vs => { versionsList = vs || []; }).catch(() => {});
    } else if (backOrgId) {
      getOrganisation(backOrgId).then(o => { backContextName = o?.name ?? backOrgId; }).catch(() => { backContextName = backOrgId; });
    }
    await loadAll();
  });

  async function loadAll() {
    loading = true;
    error = '';
    // A fresh listing invalidates any prior content-probe results.
    detectedRoles = new Map();
    detecting = new Set();
    detectRan = false;
    applyError = '';
    try {
      // Compute pinned-ness directly from `version` rather than the reactive
      // `versionPinned`, since loadAll() is called synchronously right after
      // `version` is set (before reactive statements flush).
      const pinned = !!version && !LIVE_SENTINELS.includes(version);
      if (backDatasetId && pinned) {
        // Version snapshot: list the version's snapshot graphs. Their IRIs are
        // not part of the live accessible set, so counts may be unavailable —
        // fall back to matching any counts browseGraphs happens to return.
        const [ver, allGraphs, iris] = await Promise.all([
          getDatasetVersion(backDatasetId, version),
          browseGraphs().catch(() => []),
          listDatasetGraphs(backDatasetId).catch(() => []),
        ]);
        const byIri = new Map((allGraphs || []).map(normalise).filter(Boolean).map(g => [g.iri, g]));
        const snap = ver?.snapshot_graphs || [];
        graphs = snap.map(iri => byIri.get(iri) || { iri, name: shortenIRI(iri), count: null, declaredRole: null });
        graphs = applyDeclaredRoles(graphs, rolesFromEntries(iris));
      } else if (backDatasetId) {
        const [iris, allGraphs] = await Promise.all([
          listDatasetGraphs(backDatasetId),
          browseGraphs(),
        ]);
        const irisAny = iris as unknown as { graphs?: any[] };
        const irisList = Array.isArray(iris) ? iris : (irisAny.graphs || []);
        const wanted = new Set(irisList.map((g: any) => (typeof g === 'string' ? g : g.graph_iri)));
        graphs = (allGraphs || []).map(normalise).filter(g => g && wanted.has(g.iri));
        graphs = applyDeclaredRoles(graphs, rolesFromEntries(iris));
      } else if (backOrgId) {
        const [allGraphs, allDs] = await Promise.all([browseGraphs(), listDatasets()]);
        const orgDs = (allDs || []).filter(d => d.owner_type === 'organisation' && String(d.owner_id) === String(backOrgId));
        const orgGraphSets = await Promise.all(orgDs.map(d => listDatasetGraphs(d.id).catch(() => [])));
        const orgIriSet = new Set(orgGraphSets.flat().map(g => (typeof g === 'string' ? g : g.graph_iri)));
        const roleByIri = rolesFromEntries(orgGraphSets.flat());
        graphs = (allGraphs || []).map(normalise).filter(g => g && orgIriSet.has(g.iri));
        graphs = applyDeclaredRoles(graphs, roleByIri);
      } else {
        const g = await browseGraphs();
        graphs = (g || []).map(normalise).filter(Boolean);
      }
    } catch (e) {
      error = e.message || String(e);
    } finally {
      loading = false;
    }
    // Auto-detect roles for the unknown remainder, but only when that set is
    // small enough to probe without hammering the SPARQL endpoint.
    maybeAutoDetect();
  }

  let deleteLoading = false;

  async function doDelete(iri) {
    deleteLoading = true;
    try {
      await deleteGraph(iri);
      confirmDelete = null;
      await loadAll();
    } catch (e) {
      error = e.message;
      confirmDelete = null;
    }
    deleteLoading = false;
  }

  async function doExport(iri) {
    exporting = iri;
    try {
      const turtle = await exportGraph(iri);
      const filename = (iri?.split('/').pop()?.split('#').pop()) || 'graph';
      downloadFile(turtle, `${filename}.ttl`, 'text/turtle');
    } catch (e) {
      error = e.message;
    } finally {
      exporting = null;
    }
  }

  async function createGraph() {
    if (!newGraphIri.trim()) return;
    creating = true;
    createError = '';
    try {
      await uploadToGraph(newGraphIri.trim(), '', 'text/turtle', false);
      showCreateForm = false;
      newGraphIri = '';
      await loadAll();
    } catch (e) {
      createError = e.message;
    } finally {
      creating = false;
    }
  }

  $: filtered = filterText.trim()
    ? graphs.filter(g => (g.iri || '').toLowerCase().includes(filterText.toLowerCase()) || (g.name || '').toLowerCase().includes(filterText.toLowerCase()))
    : graphs;

  // Footer totals over the currently shown graphs. Counts can be null (e.g.
  // version snapshot graphs), so the triple sum only covers graphs we have a
  // number for; `countedGraphs` lets us flag a partial total.
  $: totalTriples = filtered.reduce((sum, g) => sum + (typeof g.count === 'number' ? g.count : 0), 0);
  $: countedGraphs = filtered.filter(g => typeof g.count === 'number').length;

  const GRAPH_ROLE_PATTERNS: Array<{ pattern: RegExp; role: string }> = [
    { pattern: /urn:entailment:|\/entailment\/|#entailment|\/inferred?\//i, role: 'entailment' },
    // Only the `urn:system:` URN convention denotes a genuine internal system
    // graph (commit-log, registries). A loose `/system/` path segment in a user
    // graph IRI is not a system graph — matching it produced false "System
    // suggested" badges that disagreed with both the content probe (which has no
    // 'system' verdict) and the dataset's declared role.
    { pattern: /^urn:system:/i,                                            role: 'system' },
    { pattern: /\/shapes?\/|#shapes?|urn:shacl:|\/shacl\//i,               role: 'shapes' },
    { pattern: /\/skos|#skos|\/vocab|#vocab|\/concept|\/thesaur/i,          role: 'vocabulary' },
    { pattern: /\/ontolog|#ontolog|urn:ontolog|\/schema\/|\/model\/|\/owl(\/|#)/i, role: 'model' },
  ];

  // Best-guess role from the graph IRI alone (no content read).
  function inferGraphRole(iri: string | null): string | null {
    if (!iri) return null;
    for (const p of GRAPH_ROLE_PATTERNS) {
      if (p.pattern.test(iri)) return p.role;
    }
    return null;
  }

  // Resolve the role to show for a graph, in priority order:
  //   declared (set on the dataset) → live content probe → IRI-pattern guess.
  // `source` drives the badge styling and whether an "apply" action is offered.
  function resolveRole(g, detected = detectedRoles): { role: string; label: string; cls: string; source: 'declared' | 'detected' | 'inferred' } | null {
    const declared = normalizeGraphRole(g?.declaredRole);
    if (declared) return { role: declared, label: graphRoleLabel(declared)!, cls: `role-${declared}`, source: 'declared' };
    const det = g?.iri ? detected.get(g.iri) : null;
    if (det) return { role: det, label: graphRoleLabel(det)!, cls: `role-${det}`, source: 'detected' };
    const inferred = inferGraphRole(g?.iri);
    if (inferred) return { role: inferred, label: graphRoleLabel(inferred)!, cls: `role-${inferred}`, source: 'inferred' };
    return null;
  }

  // Graphs with no role we can resolve yet — candidates for content probing.
  function computeUndetected(list = graphs, detected = detectedRoles) {
    return list.filter(g =>
      g && g.iri &&
      !normalizeGraphRole(g.declaredRole) &&
      !inferGraphRole(g.iri) &&
      !detected.has(g.iri),
    );
  }
  $: undetectedGraphs = computeUndetected(graphs, detectedRoles);

  function maybeAutoDetect() {
    if (detectRan) return;
    const targets = computeUndetected();
    if (targets.length === 0 || targets.length > AUTO_DETECT_LIMIT) return;
    detectRan = true;
    detectRoles(targets.map(g => g.iri));
  }

  // Probe graph contents (limited concurrency) and record the detected role.
  async function detectRoles(iris: string[]) {
    const queue = iris.filter(Boolean);
    const worker = async () => {
      while (queue.length) {
        const iri = queue.shift()!;
        if (detectedRoles.has(iri) || detecting.has(iri)) continue;
        detecting.add(iri); detecting = detecting;
        try {
          const probe = await probeContentKind([iri]);
          const role = normalizeGraphRole(probe?.verdict);
          if (role) { detectedRoles.set(iri, role); detectedRoles = detectedRoles; }
        } catch { /* leave undetected */ }
        finally { detecting.delete(iri); detecting = detecting; }
      }
    };
    await Promise.all(Array.from({ length: Math.min(DETECT_CONCURRENCY, queue.length) }, worker));
  }

  // Manual trigger for large lists where auto-detect is skipped.
  function detectRemaining() {
    detectRan = true;
    detectRoles(computeUndetected().map(g => g.iri));
  }

  // Persist a suggested (detected/inferred) role onto the dataset graph.
  // Only meaningful in dataset scope, where there is one owning dataset.
  async function applySuggestedRole(iri: string, role: string) {
    if (!backDatasetId || !iri || !role) return;
    applyingRole = iri;
    applyError = '';
    try {
      await updateGraphRole(backDatasetId, iri, role);
      graphs = graphs.map(g => g && g.iri === iri ? { ...g, declaredRole: role } : g);
    } catch (e) {
      applyError = e.message || $t('pages.graphList.failedToSetRole');
    } finally {
      applyingRole = null;
    }
  }
</script>

<div class="graphs-page space-y-4">
  <PageHeader
    icon={Network}
    title={$t('pages.graphList.namedGraphs')}
    count="{graphs.length} {graphs.length === 1 ? $t('pages.graphList.graph') : $t('pages.graphList.graphs')}"
    breadcrumbs={backOrgId
      ? [{ label: $t('pages.graphList.organisations'), href: '/organisations' }, { label: backContextName ?? '…', href: '/organisations/' + backOrgId }, { label: $t('pages.graphList.namedGraphs') }]
      : backDatasetId
        ? [{ label: $t('pages.graphList.datasets'), href: '/datasets' }, { label: backContextName ?? '…', href: '/datasets/' + backDatasetId }, { label: $t('pages.graphList.namedGraphs') }]
        : [{ label: $t('pages.graphList.datasets'), href: '/datasets' }, { label: $t('pages.graphList.namedGraphs') }]}
  >
    <div slot="actions">
      {#if versionsList.length > 0}
        <Select size="sm" class="gl-version-select {versionPinned ? 'gl-version-pinned' : ''}"
          value={version || ''} on:change={e => onPickVersion(e.detail)}
          options={[{ value: '', label: $t('pages.graphList.liveCurrent') }, ...sortedVersionsList.map(v => ({ value: v.version, label: `v${v.version}${v.status && v.status !== 'published' ? ` · ${v.status}` : ''}` }))]}
          title={$t('pages.graphList.versionSelectTitle')} />
      {/if}
      <button class="info-btn" on:click={() => showInfo = !showInfo} aria-expanded={showInfo}>
        <Info size={14} />
        {$t('pages.graphList.about')}
        <ChevronDown size={13} class="transition-transform {showInfo ? 'rotate-180' : ''}" />
      </button>
      {#if $isAuthenticated}
      <button class="btn btn-sm" on:click={() => { showCreateForm = true; createError = ''; newGraphIri = ''; }}>
        <Plus size={14} /> {$t('pages.graphList.newGraph')}
      </button>
      {/if}
    </div>
  </PageHeader>

  {#if showInfo}
    <div class="info-panel">
      <p><strong>{$t('pages.graphList.namedGraphs')}</strong> {$t('pages.graphList.infoIntro')}</p>
      <ul>
        <li><strong>{$t('pages.graphList.infoIriIdentifier')}</strong> — {$t('pages.graphList.infoIriIdentifierDesc')}</li>
        <li><strong>{$t('pages.graphList.infoTripleStorage')}</strong> — {$t('pages.graphList.infoTripleStorageDesc')}</li>
        <li><strong>{$t('pages.graphList.infoSparqlQueries')}</strong> — {$t('pages.graphList.infoSparqlQueriesPre')} <code>GRAPH &lt;iri&gt;</code> {$t('pages.graphList.infoSparqlQueriesPost')}</li>
        <li><strong>{$t('pages.graphList.infoGraphStoreProtocol')}</strong> — {$t('pages.graphList.infoGraphStoreProtocolPre')} <code>/sparql?graph=&lt;iri&gt;</code> {$t('pages.graphList.infoGraphStoreProtocolPost')}</li>
        <li><strong>{$t('pages.graphList.infoOwnership')}</strong> — {$t('pages.graphList.infoOwnershipDesc')}</li>
      </ul>
      <Link to="/docs/named-graphs" class="info-docs-link">{$t('pages.graphList.viewFullDocs')}</Link>
    </div>
  {/if}

  {#if versionPinned}
    <div class="version-banner">
      <History size={14} />
      {$t('pages.graphList.versionBannerPre')} <strong>v{version}</strong> {$t('pages.graphList.versionBannerPost')}
      <button class="version-banner-clear" on:click={() => onPickVersion('')}>{$t('pages.graphList.backToLive')}</button>
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
        id="graph-search"
        type="text"
        placeholder={$t('pages.graphList.filterPlaceholder')}
        bind:value={filterText}
      />
      {#if filterText}
        <button class="filter-clear" on:click={() => filterText = ''} aria-label={$t('system.clear')}><X size={12} /></button>
      {/if}
    </div>
    {#if undetectedGraphs.length > 0 && detecting.size === 0}
      <button class="detect-btn" on:click={detectRemaining} title={$t('pages.graphList.detectRolesTitle')}>
        <Sparkles size={13} /> {$t('pages.graphList.detectRoles')}{undetectedGraphs.length > AUTO_DETECT_LIMIT ? ` (${undetectedGraphs.length})` : ''}
      </button>
    {:else if detecting.size > 0}
      <span class="detect-status"><Loader2 size={13} class="spin" /> {$t('pages.graphList.detectingRoles')} ({detecting.size})</span>
    {/if}
    <span class="filter-count">{$t('pages.graphList.filterCount', { values: { shown: filtered.length, total: graphs.length } })}</span>
  </div>

  {#if applyError}
    <p class="error">{applyError}</p>
  {/if}

  <div class="card overflow-x-auto">
    <table>
      <thead>
        <tr>
          {#if $isAuthenticated}
          <th class="th-check">
            <input
              type="checkbox"
              bind:this={headerCheckbox}
              on:change={toggleSelectAll}
              aria-label={$t('pages.graphList.selectAllGraphs')}
              class="row-check"
            />
          </th>
          {/if}
          <th>{$t('pages.graphList.graphIri')}</th>
          <th class="col-role">{$t('pages.graphList.role')}</th>
          <th class="col-count">{$t('pages.graphList.triplesHeader')}</th>
          <th class="td-actions"></th>
        </tr>
      </thead>
      <tbody>
        {#if loading}
          <tr><td colspan={$isAuthenticated ? 5 : 4} class="text-center py-8"><Loader2 class="inline spin mr-2" size={16} /> {$t('pages.graphList.loadingGraphs')}</td></tr>
        {:else if graphs.length === 0}
          <tr><td colspan={$isAuthenticated ? 5 : 4} class="text-center py-8 text-[var(--ink-500)]">{$t('pages.graphList.noGraphs')}</td></tr>
        {:else if filtered.length === 0}
          <tr><td colspan={$isAuthenticated ? 5 : 4} class="text-center py-8 text-[var(--ink-500)]">{$t('pages.graphList.noMatch', { values: { query: filterText } })}</td></tr>
        {:else}
          {#each filtered as g (g.iri ?? 'default')}
            {@const roleInfo = resolveRole(g, detectedRoles)}
            {@const isSelected = g.iri ? selected.has(g.iri) : false}
            <tr
              class="g-row"
              class:row-selected={isSelected}
              on:click={(e) => {
                if ((e.target as HTMLElement).closest('button') || (e.target as HTMLElement).closest('input[type="checkbox"]')) return;
                if (g.iri) navigate(browseGraphUrl(g.iri));
              }}
            >
              {#if $isAuthenticated}
              <td class="td-check" on:click|stopPropagation>
                {#if g.iri}
                  <input
                    type="checkbox"
                    checked={isSelected}
                    on:change={() => toggleSelect(g.iri)}
                    aria-label={$t('pages.graphList.selectGraph', { values: { iri: g.iri } })}
                    class="row-check"
                  />
                {/if}
              </td>
              {/if}
              <td>
                <div class="flex items-center gap-2">
                  <Network size={14} class="text-[var(--brand-500)]" />
                  <span class="font-medium">{g.iri ? shortenIRI(g.iri) : $t('pages.graphList.defaultGraph')}</span>
                </div>
                <div class="text-xs text-[var(--ink-400)] font-mono mt-1">{g.iri || '(unnamed)'}</div>
              </td>
              <td class="col-role">
                {#if roleInfo}
                  {#if roleInfo.source === 'declared'}
                    <span class="role-badge {roleInfo.cls}" title={$t('pages.graphList.roleDeclaredTitle')}><Tag size={10} />{roleInfo.label}</span>
                  {:else}
                    <span class="role-badge role-suggested {roleInfo.cls}" title={roleInfo.source === 'detected' ? $t('pages.graphList.roleDetectedTitle') : $t('pages.graphList.roleInferredTitle')}>
                      <Sparkles size={10} />{roleInfo.label}<span class="role-suggested-tag">{$t('pages.graphList.suggested')}</span>
                    </span>
                    {#if backDatasetId && $isAuthenticated}
                      <button
                        class="role-apply-btn"
                        title={$t('pages.graphList.saveRoleTitle')}
                        disabled={applyingRole === g.iri}
                        on:click={() => applySuggestedRole(g.iri, roleInfo.role)}
                      >
                        {#if applyingRole === g.iri}<Loader2 size={11} class="spin" />{:else}<Check size={11} />{/if}
                        {$t('pages.graphList.set')}
                      </button>
                    {/if}
                  {/if}
                {:else if detecting.has(g.iri)}
                  <span class="role-detecting"><Loader2 size={10} class="spin" /> {$t('pages.graphList.detecting')}</span>
                {:else}
                  <span class="text-[var(--ink-300)] text-xs">—</span>
                {/if}
              </td>
              <td class="col-count">
                <span class="font-medium">{g.count !== null ? formatNumber(g.count) : '—'}</span>
              </td>
              <td class="td-actions">
                {#if g.iri}
                  <button class="tbl-btn" on:click|stopPropagation={() => navigate(browseGraphUrl(g.iri))} title={$t('pages.graphList.browse')}>
                    <Search size={14} />
                  </button>
                  <button class="tbl-btn" on:click|stopPropagation={() => navigate(`/browse?view=graph&subject=${encodeURIComponent(g.iri)}`)} title={$t('pages.graphList.visualize')}>
                    <Share2 size={14} />
                  </button>
                  <button class="tbl-btn" on:click|stopPropagation={() => doExport(g.iri)} disabled={exporting === g.iri} title={$t('system.export')}>
                    {#if exporting === g.iri}<Loader2 size={14} class="spin" />{:else}<Download size={14} />{/if}
                  </button>
                  {#if $isAuthenticated}
                  <button class="tbl-btn" on:click|stopPropagation={() => attachGraphIri = g.iri} title={$t('pages.graphList.attachShapes')}>
                    <ShieldCheck size={14} />
                  </button>
                  <button class="tbl-btn danger" on:click|stopPropagation={() => confirmDelete = g.iri} title={$t('system.delete')}>
                    <Trash2 size={14} />
                  </button>
                  {/if}
                {/if}
              </td>
            </tr>
          {/each}
        {/if}
      </tbody>
      {#if !loading && filtered.length > 0}
        <tfoot>
          <tr class="totals-row">
            {#if $isAuthenticated}<td class="td-check"></td>{/if}
            <td colspan="2">
              <span class="totals-label">{$t('pages.graphList.total')}</span>
              <span class="totals-graphs">{formatNumber(filtered.length)} {filtered.length === 1 ? $t('pages.graphList.graph') : $t('pages.graphList.graphs')}</span>
            </td>
            <td class="col-count">
              <span class="totals-triples">{formatNumber(totalTriples)}</span>
              {#if countedGraphs < filtered.length}
                <span class="totals-partial" title={$t('pages.graphList.partialTitle', { values: { count: filtered.length - countedGraphs } })}>*</span>
              {/if}
            </td>
            <td class="td-actions"></td>
          </tr>
        </tfoot>
      {/if}
    </table>
  </div>
</div>

<!-- Bulk action bar -->
{#if $isAuthenticated}
  <BulkActionBar
    count={selected.size}
    total={selectableFiltered.length}
    itemLabel={$t('pages.graphList.graph')}
    on:clearSelection={clearSelection}
    on:selectAll={() => { selectableFiltered.forEach(g => selected.add(g.iri)); selected = selected; }}
  >
    <button class="bulk-action-btn" on:click={openCreateDatasetModal} title={$t('pages.graphList.newDatasetFromSelectionTitle')}>
      <Database size={13} /> {$t('pages.graphList.newDatasetFromSelection')}
    </button>
    <button class="bulk-action-btn danger" on:click={() => confirmBulkDelete = true} disabled={bulkDeleting}>
      <Trash2 size={13} /> {$t('pages.graphList.deleteNGraphs', { values: { count: selected.size } })}
    </button>
  </BulkActionBar>
{/if}

{#if confirmDelete}
  <ConfirmModal
    title={$t('pages.graphList.deleteGraph')}
    message="{$t('pages.graphList.deleteWarning')} {$t('pages.graphList.cannotUndo')}"
    confirmLabel={$t('pages.graphList.deletePermanently')}
    loading={deleteLoading}
    on:confirm={() => doDelete(confirmDelete)}
    on:cancel={() => confirmDelete = null}
  >
    <code>{confirmDelete}</code>
  </ConfirmModal>
{/if}

{#if attachGraphIri}
  <AttachShapesDialog
    targetKind="graph"
    targetId={attachGraphIri}
    targetLabel={shortenIRI(attachGraphIri)}
    on:close={() => attachGraphIri = null}
  />
{/if}

<!-- Bulk delete confirm modal -->
{#if confirmBulkDelete}
  <ConfirmModal
    title={$t('pages.graphList.bulkDeleteTitle', { values: { count: selected.size } })}
    message={$t('pages.graphList.bulkDeleteMessage', { values: { count: selected.size } })}
    confirmLabel={$t('pages.graphList.deleteNGraphs', { values: { count: selected.size } })}
    loading={bulkDeleting}
    on:confirm={bulkDelete}
    on:cancel={() => confirmBulkDelete = false}
  />
{/if}

<!-- Create dataset from selected graphs modal -->
{#if showCreateDatasetModal}
  <div
    class="cd-backdrop"
    on:click={() => { if (!cdLoading) showCreateDatasetModal = false; }}
    role="presentation"
  >
    <div
      class="cd-box"
      on:click|stopPropagation
      on:keydown|stopPropagation
      role="dialog"
      aria-modal="true"
      aria-label={$t('pages.graphList.createDatasetFromGraphs')}
      tabindex="-1"
    >
      <h3 class="cd-title">{$t('pages.graphList.newDatasetFromN', { values: { count: selected.size } })}</h3>
      <p class="cd-hint">{$t('pages.graphList.newDatasetHint')}</p>

      <form on:submit|preventDefault={createDatasetFromSelected} class="cd-form">
        <div class="cd-field">
          <label for="cd-name">{$t('pages.graphList.datasetName')} <span class="req">*</span></label>
          <input id="cd-name" type="text" bind:value={cdName} required placeholder={$t('pages.graphList.datasetNamePlaceholder')} />
        </div>
        <div class="cd-field">
          <label for="cd-vis">{$t('pages.graphList.visibility')}</label>
          <Select id="cd-vis" bind:value={cdVisibility} options={[
            { value: 'private', label: $t('pages.graphList.private') },
            { value: 'members', label: $t('pages.graphList.members') },
            { value: 'public', label: $t('pages.graphList.public') },
          ]} />
        </div>
        {#if organisations.length > 0}
          <div class="cd-field">
            <span class="cd-field-label">{$t('pages.graphList.owner')}</span>
            <div class="owner-opts">
              <label class="owner-opt" class:owner-opt-sel={cdOwnerType === 'user'}>
                <input type="radio" bind:group={cdOwnerType} value="user" /> {$t('pages.graphList.personal')}
              </label>
              {#each organisations as org}
                <label
                  class="owner-opt"
                  class:owner-opt-sel={cdOwnerType === 'organisation' && cdOwnerOrgId === org.id}
                >
                  <input
                    type="radio"
                    bind:group={cdOwnerType}
                    value="organisation"
                    on:change={() => cdOwnerOrgId = org.id}
                  /> {org.name}
                </label>
              {/each}
            </div>
          </div>
        {/if}
        <div class="cd-selected-graphs">
          <span class="cd-graphs-label">{$t('pages.graphList.graphsToLink')}</span>
          <ul>
            {#each [...selected] as iri}
              <li><code class="iri-pill">{shortenIRI(iri)}</code></li>
            {/each}
          </ul>
        </div>
        {#if cdError}<p class="error">{cdError}</p>{/if}
        <div class="cd-actions">
          <button type="button" class="btn btn-ghost" on:click={() => showCreateDatasetModal = false} disabled={cdLoading}>{$t('system.cancel')}</button>
          <button type="submit" class="btn" disabled={cdLoading || !cdName.trim()}>
            {#if cdLoading}<Loader2 size={14} class="spin" /> {$t('pages.graphList.creating')}{:else}{$t('pages.graphList.createDataset')}{/if}
          </button>
        </div>
      </form>
    </div>
  </div>
{/if}

<!-- Create named graph modal -->
{#if showCreateForm}
  <div
    class="ng-backdrop"
    on:click={() => { if (!creating) showCreateForm = false; }}
    role="presentation"
  >
    <div
      class="ng-box"
      on:click|stopPropagation
      on:keydown|stopPropagation
      role="dialog"
      aria-modal="true"
      aria-label={$t('pages.graphList.createNamedGraph')}
      tabindex="-1"
    >
      <div class="ng-header">
        <div class="ng-icon-wrap"><Network size={22} /></div>
        <div>
          <h3 class="ng-title">{$t('pages.graphList.newNamedGraph')}</h3>
          <p class="ng-subtitle">{$t('pages.graphList.newNamedGraphSubtitle')}</p>
        </div>
        <button class="ng-close" on:click={() => showCreateForm = false} aria-label={$t('system.close')}><X size={16} /></button>
      </div>

      <form class="ng-form" on:submit|preventDefault={createGraph}>
        <div class="ng-field">
          <label for="ng-iri">{$t('pages.graphList.graphIri')} <span class="ng-req">*</span></label>
          <input
            id="ng-iri"
            type="url"
            bind:value={newGraphIri}
            placeholder="https://example.org/graphs/my-graph"
            required
            use:autofocus
          />
          <span class="ng-hint">
            {$t('pages.graphList.ngHintPre')}
            <code>https://example.org/graphs/name</code> {$t('pages.graphList.ngHintMid')} <code>urn:graph:name</code>.
          </span>
        </div>

        <div class="ng-conventions">
          <p class="ng-conv-title">{$t('pages.graphList.commonIriPatterns')}</p>
          <ul>
            <li><code>/ontolog</code>, <code>/vocab/</code> — {$t('pages.graphList.patternModelVocab')}</li>
            <li><code>/shapes/</code>, <code>/shacl/</code> — {$t('pages.graphList.patternShapes')}</li>
            <li><code>/data/</code>, <code>/instances/</code> — {$t('pages.graphList.patternInstances')}</li>
            <li><code>/entailment/</code> — {$t('pages.graphList.patternEntailment')}</li>
          </ul>
        </div>

        {#if createError}<p class="ng-error">{createError}</p>{/if}

        <div class="ng-actions">
          <button type="button" class="btn btn-ghost" on:click={() => showCreateForm = false} disabled={creating}>
            {$t('system.cancel')}
          </button>
          <button type="submit" class="btn" disabled={creating || !newGraphIri.trim()}>
            {#if creating}<Loader2 size={14} class="spin" /> {$t('pages.graphList.creating')}{:else}<Plus size={14} /> {$t('pages.graphList.createGraph')}{/if}
          </button>
        </div>
      </form>
    </div>
  </div>
{/if}

<style>
  h3 { margin-top: 0; margin-bottom: 0.75rem; }

  .info-panel {
    background: #f0f9ff;
    border: 1px solid #bae6fd;
    border-radius: 10px;
    padding: 1rem;
    font-size: 0.85rem;
    color: var(--ink-700, #374151);
    line-height: 1.55;
  }
  .gl-version-select {
    font-size: 0.8rem; padding: 0.3rem 0.5rem; border-radius: 6px;
    border: 1px solid var(--line-soft, #d0d7de); background: #f6f8fa; cursor: pointer;
    margin-right: 0.4rem;
  }
  .gl-version-select.gl-version-pinned { color: #92400e; background: #fef3c7; border-color: #fde68a; font-weight: 600; }
  .version-banner {
    display: flex; align-items: center; gap: 0.5rem;
    background: #fffbeb; border: 1px solid #fde68a; color: #92400e;
    border-radius: 8px; padding: 0.5rem 0.75rem; font-size: 0.85rem;
  }
  .version-banner strong { font-weight: 700; }
  .version-banner-clear {
    margin-left: auto; border: 1px solid #fcd34d; background: #fff7ed; color: #92400e;
    border-radius: 6px; padding: 0.2rem 0.6rem; font-size: 0.78rem; cursor: pointer;
  }
  .version-banner-clear:hover { background: #fef3c7; }

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
  .create-row { display: flex; gap: 0.5rem; align-items: flex-start; flex-wrap: wrap; }
  .iri-input { flex: 1; min-width: 200px; }

  .filter-row {
    display: flex;
    align-items: center;
    gap: 1rem;
    margin-bottom: 1rem;
  }
  .filter-input {
    display: flex; align-items: center; gap: 0.4rem;
    padding: 0.4rem 0.7rem;
    border: 1px solid var(--line-soft);
    border-radius: 8px;
    background: white;
    color: #64748b;
    flex: 1;
  }
  .filter-input input { flex: 1; border: none; outline: none; background: transparent; font-size: 0.85rem; color: #1e293b; }
  .filter-clear { display: grid; place-items: center; width: 18px; height: 18px; border-radius: 50%; border: none; background: #e2e8f0; color: #64748b; cursor: pointer; }
  .filter-clear:hover { background: #cbd5e1; }
  .filter-count { font-size: 0.8rem; color: var(--ink-500); white-space: nowrap; }

  table {
    width: 100%;
    border-collapse: collapse;
    font-size: 0.9rem;
  }
  table thead {
    background: var(--bg-soft, #f8fafc);
    border-bottom: 1px solid var(--line-soft);
  }
  table th {
    padding: 0.75rem;
    text-align: left;
    font-weight: 600;
    color: var(--ink-600);
    font-size: 0.8rem;
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }
  table tbody tr {
    border-bottom: 1px solid var(--line-soft);
    transition: background 0.12s;
  }
  table tbody tr:hover {
    background: var(--bg-soft, #f8fafc);
  }
  table td {
    padding: 0.75rem;
    color: var(--ink-700);
    vertical-align: middle;
  }

  .g-row { cursor: pointer; }

  table tfoot td {
    padding: 0.6rem 0.75rem;
    border-top: 2px solid var(--line-soft);
    background: var(--bg-soft, #f8fafc);
    font-size: 0.85rem;
  }
  .totals-label {
    font-weight: 600; color: var(--ink-600);
    text-transform: uppercase; letter-spacing: 0.05em; font-size: 0.75rem;
    margin-right: 0.6rem;
  }
  .totals-graphs { color: var(--ink-700); font-weight: 600; }
  .totals-triples { font-weight: 700; color: var(--ink-800); }
  .totals-partial { color: var(--ink-400); margin-left: 0.15rem; }

  .error { color: #dc2626; background: #fef2f2; border: 1px solid #fecaca; padding: 0.6rem 0.8rem; border-radius: 6px; font-size: 0.85rem; }

  :global(.btn-danger) { color: #dc2626; border-color: #fecaca; background: white; }
  :global(.btn-danger:hover) { background: #fef2f2; border-color: #fca5a5; }

  :global(.spin) { animation: spin 0.9s linear infinite; }
  @keyframes spin { to { transform: rotate(360deg); } }

  /* Role badges */
  .role-badge {
    display: inline-flex;
    align-items: center;
    gap: 0.25rem;
    padding: 0.15rem 0.45rem;
    border-radius: 4px;
    font-size: 0.75rem;
    font-weight: 600;
  }
  .role-model      { background: #dcfce7; color: #15803d; }
  .role-vocabulary { background: #fdf4ff; color: #7e22ce; }
  .role-shapes     { background: #fef9c3; color: #854d0e; }
  .role-entailment { background: #ede9fe; color: #5b21b6; }
  .role-instances  { background: #dbeafe; color: #1e40af; }
  .role-system     { background: #f1f5f9; color: #475569; }

  /* Suggested (detected/inferred but not yet saved) roles read as muted,
     dashed-outline variants so they don't look like a confirmed assignment. */
  .role-suggested {
    background: transparent;
    box-shadow: inset 0 0 0 1px currentColor;
    opacity: 0.85;
  }
  .role-suggested-tag {
    font-weight: 500;
    font-size: 0.62rem;
    text-transform: uppercase;
    letter-spacing: 0.03em;
    opacity: 0.7;
  }
  .role-apply-btn {
    display: inline-flex; align-items: center; gap: 0.2rem;
    margin-left: 0.3rem; padding: 0.1rem 0.4rem;
    border: 1px solid var(--line-soft, #d1d5db); border-radius: 5px;
    background: white; color: var(--ink-600, #475569);
    font-size: 0.7rem; font-weight: 600; cursor: pointer;
  }
  .role-apply-btn:hover:not(:disabled) { background: var(--bg-soft, #f8fafc); border-color: var(--brand-400, #2dd4bf); }
  .role-apply-btn:disabled { opacity: 0.6; cursor: default; }
  .role-detecting { display: inline-flex; align-items: center; gap: 0.25rem; font-size: 0.72rem; color: var(--ink-400, #94a3b8); }

  .detect-btn {
    display: inline-flex; align-items: center; gap: 0.3rem;
    padding: 0.4rem 0.7rem; border: 1px solid var(--line-soft, #d1d5db);
    border-radius: 8px; background: white; color: var(--ink-600, #475569);
    font-size: 0.8rem; font-weight: 600; cursor: pointer; white-space: nowrap;
  }
  .detect-btn:hover { background: var(--bg-soft, #f8fafc); border-color: var(--brand-400, #2dd4bf); }
  .detect-status { display: inline-flex; align-items: center; gap: 0.3rem; font-size: 0.8rem; color: var(--ink-500, #64748b); white-space: nowrap; }

  @media (max-width: 720px) {
    .filter-row { flex-wrap: wrap; }
    .filter-input { width: 100%; }
  }

  /* Responsive table: hide Role & Triples columns on very small screens */
  @media (max-width: 560px) {
    .col-role, .col-count { display: none; }
  }

  /* Actions cell — always shrink to icon buttons, never expand */
  .td-actions {
    white-space: nowrap;
    text-align: right;
    width: 1%;        /* shrink to content */
    padding-left: 0.25rem;
  }

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
  .row-selected {
    background: #f0fdfa !important;
  }

  /* Create-dataset modal */
  .cd-backdrop {
    position: fixed; inset: 0;
    background: rgba(0, 0, 0, 0.35);
    display: flex; align-items: center; justify-content: center;
    z-index: 50;
  }
  .cd-box {
    background: white; border-radius: 14px; padding: 1.5rem;
    width: min(500px, calc(100vw - 2rem));
    box-shadow: 0 20px 60px rgba(0, 0, 0, 0.18);
  }
  .cd-title { margin: 0 0 0.25rem; font-size: 1.05rem; font-weight: 600; }
  .cd-hint  { margin: 0 0 1rem; font-size: 0.82rem; color: var(--ink-500); }
  .cd-form  { display: flex; flex-direction: column; gap: 0.85rem; }
  .cd-field { display: flex; flex-direction: column; gap: 0.25rem; font-size: 0.875rem; font-weight: 500; }
  .cd-field input {
    padding: 0.45rem 0.7rem;
    border: 1px solid var(--line-soft, #d1d5db);
    border-radius: 8px;
    font-size: 0.875rem;
  }
  .cd-field input:focus { outline: none; border-color: var(--brand-400); }
  .req { color: #dc2626; }
  .owner-opts { display: flex; flex-wrap: wrap; gap: 0.4rem; }
  .owner-opt {
    display: flex; align-items: center; gap: 0.35rem;
    padding: 0.3rem 0.65rem; border: 1px solid var(--line-soft);
    border-radius: 6px; cursor: pointer; font-size: 0.82rem;
    font-weight: normal; background: white;
  }
  .owner-opt input[type="radio"] { display: none; }
  .owner-opt-sel { border-color: var(--brand-500); background: #f0fdfa; color: var(--brand-700); }
  .cd-selected-graphs {
    background: #f8fafc; border: 1px solid var(--line-soft);
    border-radius: 8px; padding: 0.65rem;
    max-height: 130px; overflow-y: auto;
  }
  .cd-graphs-label { font-size: 0.78rem; font-weight: 600; color: var(--ink-500); display: block; margin-bottom: 0.35rem; }
  .cd-selected-graphs ul { margin: 0; padding: 0; list-style: none; display: flex; flex-wrap: wrap; gap: 0.3rem; }
  .iri-pill {
    display: inline-block; padding: 0.1rem 0.45rem;
    background: #e0f2fe; color: #0369a1;
    border-radius: 999px; font-size: 0.75rem;
  }
  .cd-actions { display: flex; gap: 0.5rem; justify-content: flex-end; padding-top: 0.25rem; }

  /* Named graph creation modal */
  .ng-backdrop {
    position: fixed; inset: 0;
    background: rgba(0, 0, 0, 0.35);
    display: flex; align-items: center; justify-content: center;
    z-index: 50;
  }
  .ng-box {
    background: white; border-radius: 14px;
    width: min(520px, calc(100vw - 2rem));
    box-shadow: 0 20px 60px rgba(0, 0, 0, 0.18);
    overflow: hidden;
  }
  .ng-header {
    display: flex; align-items: flex-start; gap: 1rem;
    padding: 1.5rem 1.5rem 1.25rem;
    border-bottom: 1px solid var(--line-soft, #e5e7eb);
    position: relative;
  }
  .ng-icon-wrap {
    display: grid; place-items: center;
    width: 44px; height: 44px;
    background: var(--brand-50, #f0fdfa);
    border: 1px solid var(--brand-200, #99f6e4);
    border-radius: 10px;
    color: var(--brand-600, #0d9488);
    flex-shrink: 0;
  }
  .ng-title { margin: 0 0 0.2rem; font-size: 1.05rem; font-weight: 600; color: var(--ink-800); }
  .ng-subtitle { margin: 0; font-size: 0.82rem; color: var(--ink-500); }
  .ng-close {
    position: absolute; top: 1rem; right: 1rem;
    display: grid; place-items: center;
    width: 28px; height: 28px;
    border: none; background: none; cursor: pointer;
    border-radius: 6px; color: var(--ink-400);
  }
  .ng-close:hover { background: var(--bg-soft); color: var(--ink-700); }
  .ng-form {
    padding: 1.25rem 1.5rem 1.5rem;
    display: flex; flex-direction: column; gap: 1rem;
  }
  .ng-field { display: flex; flex-direction: column; gap: 0.3rem; }
  .ng-field label { font-size: 0.875rem; font-weight: 500; color: var(--ink-700); }
  .ng-req { color: #dc2626; }
  .ng-field input {
    padding: 0.5rem 0.75rem;
    border: 1px solid var(--line-soft, #d1d5db);
    border-radius: 8px;
    font-size: 0.875rem;
    font-family: 'JetBrains Mono', 'Fira Code', monospace;
  }
  .ng-field input:focus { outline: none; border-color: var(--brand-400); box-shadow: 0 0 0 3px var(--brand-100, #ccfbf1); }
  .ng-hint { font-size: 0.78rem; color: var(--ink-500); line-height: 1.45; }
  .ng-hint code { background: #f1f5f9; padding: 0.05rem 0.3rem; border-radius: 3px; font-size: 0.75rem; }
  .ng-conventions {
    background: #f8fafc; border: 1px solid var(--line-soft);
    border-radius: 8px; padding: 0.75rem;
  }
  .ng-conv-title { margin: 0 0 0.4rem; font-size: 0.78rem; font-weight: 600; color: var(--ink-500); text-transform: uppercase; letter-spacing: 0.04em; }
  .ng-conventions ul { margin: 0; padding-left: 1.1rem; }
  .ng-conventions li { font-size: 0.8rem; color: var(--ink-600); margin-bottom: 0.2rem; }
  .ng-conventions code { background: #e0f2fe; color: #0369a1; padding: 0.05rem 0.3rem; border-radius: 3px; font-size: 0.75rem; }
  .ng-error { margin: 0; color: #dc2626; background: #fef2f2; border: 1px solid #fecaca; padding: 0.5rem 0.75rem; border-radius: 6px; font-size: 0.83rem; }
  .ng-actions { display: flex; gap: 0.5rem; justify-content: flex-end; padding-top: 0.25rem; }

  /* ---- Dark mode overrides (scoped rules out-specify global theme.css) ---- */
  :global(:is([data-theme="dark"], .dark)) .info-panel { background: rgba(59,130,246,0.1); border-color: rgba(59,130,246,0.3); color: var(--ink-700); }

  :global(:is([data-theme="dark"], .dark)) .gl-version-select { background: var(--bg-soft); border-color: var(--line-strong); color: var(--ink-700); }
  :global(:is([data-theme="dark"], .dark)) .gl-version-select.gl-version-pinned { background: rgba(245,158,11,0.18); border-color: rgba(245,158,11,0.4); color: #fcd34d; }
  :global(:is([data-theme="dark"], .dark)) .version-banner { background: rgba(245,158,11,0.12); border-color: rgba(245,158,11,0.35); color: #fcd34d; }
  :global(:is([data-theme="dark"], .dark)) .version-banner-clear { background: rgba(245,158,11,0.16); border-color: rgba(245,158,11,0.4); color: #fcd34d; }
  :global(:is([data-theme="dark"], .dark)) .version-banner-clear:hover { background: rgba(245,158,11,0.26); }

  :global(:is([data-theme="dark"], .dark)) .create-form { background: var(--bg-soft); }
  :global(:is([data-theme="dark"], .dark)) .filter-input { background: var(--bg-soft); color: var(--ink-500); }
  :global(:is([data-theme="dark"], .dark)) .filter-input input { color: var(--ink-900); }
  :global(:is([data-theme="dark"], .dark)) .filter-clear { background: var(--line-strong); color: var(--ink-600); }
  :global(:is([data-theme="dark"], .dark)) .filter-clear:hover { background: var(--ink-400); }

  :global(:is([data-theme="dark"], .dark)) .error { color: #fca5a5; background: rgba(220,38,38,0.12); border-color: rgba(220,38,38,0.35); }
  :global(:is([data-theme="dark"], .dark) .btn-danger) { color: #fca5a5; border-color: rgba(220,38,38,0.4); background: transparent; }
  :global(:is([data-theme="dark"], .dark) .btn-danger:hover) { background: rgba(220,38,38,0.14); border-color: rgba(220,38,38,0.55); }

  :global(:is([data-theme="dark"], .dark)) .role-model      { background: rgba(16,185,129,0.18); color: #6ee7b7; }
  :global(:is([data-theme="dark"], .dark)) .role-vocabulary { background: rgba(168,85,247,0.18); color: #d8b4fe; }
  :global(:is([data-theme="dark"], .dark)) .role-shapes     { background: rgba(234,179,8,0.2); color: #fde047; }
  :global(:is([data-theme="dark"], .dark)) .role-entailment { background: rgba(139,92,246,0.2); color: #c4b5fd; }
  :global(:is([data-theme="dark"], .dark)) .role-instances  { background: rgba(59,130,246,0.2); color: #93c5fd; }
  :global(:is([data-theme="dark"], .dark)) .role-system     { background: rgba(255,255,255,0.06); color: var(--ink-500); }

  :global(:is([data-theme="dark"], .dark)) .role-apply-btn { background: var(--bg-soft); color: var(--ink-600); }
  :global(:is([data-theme="dark"], .dark)) .detect-btn { background: var(--bg-soft); color: var(--ink-600); }

  :global(:is([data-theme="dark"], .dark)) .row-selected { background: var(--bg-accent-soft) !important; }

  :global(:is([data-theme="dark"], .dark)) table tfoot td { background: var(--bg-soft); border-top-color: var(--line-strong); }

  :global(:is([data-theme="dark"], .dark)) .cd-box,
  :global(:is([data-theme="dark"], .dark)) .ng-box { background: var(--bg-strong); }
  :global(:is([data-theme="dark"], .dark)) .req,
  :global(:is([data-theme="dark"], .dark)) .ng-req { color: #fca5a5; }
  :global(:is([data-theme="dark"], .dark)) .cd-field input,
  :global(:is([data-theme="dark"], .dark)) .ng-field input { background: var(--bg-soft); color: var(--ink-900); border-color: var(--line-strong); }
  :global(:is([data-theme="dark"], .dark)) .owner-opt { background: var(--bg-soft); }
  :global(:is([data-theme="dark"], .dark)) .owner-opt-sel { background: var(--bg-accent-soft); border-color: var(--brand-300); color: var(--brand-700); }
  :global(:is([data-theme="dark"], .dark)) .cd-selected-graphs { background: var(--bg-soft); }
  :global(:is([data-theme="dark"], .dark)) .iri-pill { background: rgba(59,130,246,0.18); color: #93c5fd; }

  :global(:is([data-theme="dark"], .dark)) .ng-icon-wrap { background: var(--brand-100); border-color: var(--brand-200); color: var(--brand-700); }
  :global(:is([data-theme="dark"], .dark)) .ng-hint code { background: rgba(255,255,255,0.06); }
  :global(:is([data-theme="dark"], .dark)) .ng-conventions { background: var(--bg-soft); }
  :global(:is([data-theme="dark"], .dark)) .ng-conventions code { background: rgba(59,130,246,0.18); color: #93c5fd; }
  :global(:is([data-theme="dark"], .dark)) .ng-error { color: #fca5a5; background: rgba(220,38,38,0.12); border-color: rgba(220,38,38,0.35); }
</style>
