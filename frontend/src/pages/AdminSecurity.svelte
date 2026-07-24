<script>
  import { t } from 'svelte-i18n';
  import { isAdmin, authInitialized } from '../lib/stores.js';
  import { SYSTEM_ROLES, GRAPH_PERMISSIONS } from '../lib/permissions.js';
  import { navigate } from '../lib/router/index.js';
  import { Shield, Plus, Trash2, Edit3, Loader2, Key, Globe, Lock, Server, Check, Minus, UserPlus } from 'lucide-svelte';
  import ConfirmModal from '../components/ConfirmModal.svelte';
  import Select from '../components/Select.svelte';
  import Combobox from '../components/Combobox.svelte';
  import {
    adminListOauthProviders, adminCreateOauthProvider, adminUpdateOauthProvider, adminDeleteOauthProvider,
    listEndpointAclRules, createEndpointAclRule, updateEndpointAclRule, deleteEndpointAclRule,
    listGraphAclRules, grantGraphPermission, revokeGraphPermission,
    listTripleSecurityLabels, createTripleSecurityLabel, deleteTripleSecurityLabel,
    browseGraphs, adminListUsers, listOrganisations,
    adminGetGuestRegistration, adminSetGuestRegistration,
  } from '../lib/api.js';

  // ── Tab state ────────────────────────────────────────────────────────────────
  let activeTab = 'providers'; // 'providers' | 'endpoint-acl' | 'graph-acl' | 'triple-labels' | 'registration'

  // ── Guest self-registration toggle ───────────────────────────────────────────
  let guestReg = null;        // null = not loaded; { enabled }
  let guestRegBusy = false;
  let guestSwept = null;      // last sweep result: { enabled, guests_swept }

  async function loadGuestReg() {
    try { guestReg = await adminGetGuestRegistration(); } catch (e) { alert(e.message); }
  }

  async function toggleGuestReg() {
    if (!guestReg || guestRegBusy) return;
    guestRegBusy = true;
    try {
      guestSwept = await adminSetGuestRegistration(!guestReg.enabled);
      guestReg = { enabled: guestSwept.enabled };
    } catch (e) {
      alert(e.message);
    }
    guestRegBusy = false;
  }

  // ── OAuth Providers ───────────────────────────────────────────────────────────
  let providers = [];
  let providersLoading = false;
  let showProviderForm = false;
  let editingProvider = null;
  let providerForm = emptyProviderForm();
  let providerError = '';
  let providerLoading = false;

  const ROLE_CLAIM_MAP_PLACEHOLDER = '{"Administrators": "admin", "Publishers": "user"}';

  function emptyProviderForm() {
    return {
      name: '', slug: '', provider_type: 'oidc',
      client_id: '', client_secret: '',
      discovery_url: '', authorization_endpoint: '',
      token_endpoint: '', userinfo_endpoint: '',
      scopes: 'openid email profile',
      role_claim: '', default_role: 'user',
      role_claim_map: '{}',
      auto_provision: true, enabled: true,
    };
  }

  async function loadProviders() {
    providersLoading = true;
    try { providers = await adminListOauthProviders(); } catch (e) { alert(e.message); }
    providersLoading = false;
  }

  function openCreateProvider() {
    editingProvider = null;
    providerForm = emptyProviderForm();
    providerError = '';
    showProviderForm = true;
  }

  function openEditProvider(p) {
    editingProvider = p;
    providerForm = {
      name: p.name, slug: p.slug, provider_type: p.provider_type,
      client_id: p.client_id, client_secret: '',
      discovery_url: p.discovery_url || '',
      authorization_endpoint: p.authorization_endpoint || '',
      token_endpoint: p.token_endpoint || '',
      userinfo_endpoint: p.userinfo_endpoint || '',
      scopes: (p.scopes || []).join(' '),
      role_claim: p.role_claim || '',
      default_role: p.default_role || 'user',
      role_claim_map: p.role_claim_map ? JSON.stringify(p.role_claim_map, null, 2) : '{}',
      auto_provision: p.auto_provision !== false,
      enabled: p.enabled !== false,
    };
    providerError = '';
    showProviderForm = true;
  }

  async function submitProvider() {
    providerError = '';
    providerLoading = true;
    try {
      const payload = {
        ...providerForm,
        scopes: providerForm.scopes.split(/\s+/).filter(Boolean),
        role_claim_map: JSON.parse(providerForm.role_claim_map || '{}'),
      };
      if (!payload.client_secret) delete payload.client_secret;
      if (editingProvider) {
        await adminUpdateOauthProvider(editingProvider.id, payload);
      } else {
        await adminCreateOauthProvider(payload);
      }
      showProviderForm = false;
      await loadProviders();
    } catch (e) {
      providerError = e.message;
    }
    providerLoading = false;
  }

  let deleteProviderTarget = null;

  async function doDeleteProvider() {
    try { await adminDeleteOauthProvider(deleteProviderTarget); await loadProviders(); } catch (e) { alert(e.message); }
    deleteProviderTarget = null;
  }

  // ── Endpoint ACL ──────────────────────────────────────────────────────────────
  let endpointRules = [];
  let endpointLoading = false;
  let showEndpointForm = false;
  let editingEndpoint = null;
  let endpointForm = emptyEndpointForm();
  let endpointError = '';
  let endpointSaving = false;

  function emptyEndpointForm() {
    return { principal_type: 'role', principal_id: 'user', path_pattern: '/api/', method: '*', effect: 'allow', priority: 10 };
  }

  async function loadEndpointRules() {
    endpointLoading = true;
    try { endpointRules = await listEndpointAclRules(); } catch (e) { alert(e.message); }
    endpointLoading = false;
  }

  function openCreateEndpoint() {
    editingEndpoint = null;
    endpointForm = emptyEndpointForm();
    endpointError = '';
    showEndpointForm = true;
  }

  function openEditEndpoint(r) {
    editingEndpoint = r;
    endpointForm = { ...r };
    endpointError = '';
    showEndpointForm = true;
  }

  async function submitEndpoint() {
    endpointError = '';
    endpointSaving = true;
    try {
      if (editingEndpoint) {
        await updateEndpointAclRule(editingEndpoint.id, endpointForm);
      } else {
        await createEndpointAclRule(endpointForm);
      }
      showEndpointForm = false;
      await loadEndpointRules();
    } catch (e) {
      endpointError = e.message;
    }
    endpointSaving = false;
  }

  let deleteEndpointTarget = null;

  async function doDeleteEndpointRule() {
    try { await deleteEndpointAclRule(deleteEndpointTarget); await loadEndpointRules(); } catch (e) { alert(e.message); }
    deleteEndpointTarget = null;
  }

  // ── Graph ACL ─────────────────────────────────────────────────────────────────
  let graphRules = [];
  let graphLoading = false;
  let showGraphForm = false;
  let graphForm = { principal_type: 'user', principal_id: '', graph_iri: '', permission: 'read' };
  let graphError = '';
  let graphSaving = false;

  async function loadGraphRules() {
    graphLoading = true;
    try { graphRules = await listGraphAclRules(); } catch (e) { alert(e.message); }
    graphLoading = false;
  }

  async function submitGraphGrant() {
    graphError = '';
    graphSaving = true;
    try {
      await grantGraphPermission(graphForm);
      showGraphForm = false;
      graphForm = { principal_type: 'user', principal_id: '', graph_iri: '', permission: 'read' };
      await loadGraphRules();
    } catch (e) {
      graphError = e.message;
    }
    graphSaving = false;
  }

  let revokeGraphTarget = null;

  async function doRevokeGraph() {
    try { await revokeGraphPermission(revokeGraphTarget); await loadGraphRules(); } catch (e) { alert(e.message); }
    revokeGraphTarget = null;
  }

  // ── Triple security labels ────────────────────────────────────────────────────
  let tripleLabels = [];
  let tripleLoading = false;
  let showTripleForm = false;
  let tripleForm = { subject_iri: '', predicate_iri: '', object_value: '', label_graph_iri: '' };
  let tripleError = '';
  let tripleSaving = false;

  async function loadTripleLabels() {
    tripleLoading = true;
    try { tripleLabels = await listTripleSecurityLabels(); } catch (e) { alert(e.message); }
    tripleLoading = false;
  }

  async function submitTripleLabel() {
    tripleError = '';
    tripleSaving = true;
    try {
      await createTripleSecurityLabel(tripleForm);
      showTripleForm = false;
      tripleForm = { subject_iri: '', predicate_iri: '', object_value: '', label_graph_iri: '' };
      await loadTripleLabels();
    } catch (e) {
      tripleError = e.message;
    }
    tripleSaving = false;
  }

  let deleteLabelTarget = null;

  async function doDeleteTripleLabel() {
    try { await deleteTripleSecurityLabel(deleteLabelTarget); await loadTripleLabels(); } catch (e) { alert(e.message); }
    deleteLabelTarget = null;
  }

  // ── Smart-dropdown data ──────────────────────────────────────────────────────
  let availableGraphIris = [];
  let availableUsers = [];
  let availableOrgs = [];

  async function loadDropdownData() {
    try {
      const graphs = await browseGraphs();
      availableGraphIris = graphs
        .filter((g) => g.iri !== null)
        .map((g) => g.iri);
    } catch { /* non-fatal */ }
    try {
      const users = await adminListUsers({ limit: 200 });
      availableUsers = (users.users || users || []).map((u) => ({ id: u.id, username: u.username }));
    } catch { /* non-fatal */ }
    try {
      const orgs = await listOrganisations();
      availableOrgs = (orgs || []).map((o) => ({ id: o.id, name: o.name }));
    } catch { /* non-fatal */ }
  }

  /** Returns suggestion list entries for a principal_id field based on the current type. */
  function principalSuggestions(type) {
    if (type === 'user') return availableUsers.map(u => ({ value: u.id, label: u.username }));
    if (type === 'role') return SYSTEM_ROLES.map(r => ({ value: r.value, label: r.value }));
    if (type === 'organisation') return availableOrgs.map(o => ({ value: o.id, label: o.name }));
    return [];
  }

  // ── Lifecycle ─────────────────────────────────────────────────────────────────
  let _guardChecked = false;
  $: if ($authInitialized && !_guardChecked) {
    _guardChecked = true;
    if (!$isAdmin) navigate('/');
    else Promise.all([loadProviders(), loadEndpointRules(), loadGraphRules(), loadTripleLabels(), loadDropdownData()]);
  }

  function switchTab(tab) {
    activeTab = tab;
    showProviderForm = false;
    showEndpointForm = false;
    showGraphForm = false;
    showTripleForm = false;
    if (tab === 'registration' && guestReg === null) loadGuestReg();
  }
</script>

<div class="admin-security">
  <div class="page-header">
    <Shield size={24} />
    <h1>{$t('pages.adminSecurity.heading')}</h1>
  </div>

  <!-- Tab bar -->
  <div class="tabs" role="tablist">
    <button class="tab" class:active={activeTab === 'providers'} role="tab"
      on:click={() => switchTab('providers')}>
      <Key size={15} /> {$t('pages.adminSecurity.tabProviders')}
    </button>
    <button class="tab" class:active={activeTab === 'endpoint-acl'} role="tab"
      on:click={() => switchTab('endpoint-acl')}>
      <Server size={15} /> {$t('pages.adminSecurity.tabEndpointAcl')}
    </button>
    <button class="tab" class:active={activeTab === 'graph-acl'} role="tab"
      on:click={() => switchTab('graph-acl')}>
      <Globe size={15} /> {$t('pages.adminSecurity.tabGraphAcl')}
    </button>
    <button class="tab" class:active={activeTab === 'triple-labels'} role="tab"
      on:click={() => switchTab('triple-labels')}>
      <Lock size={15} /> {$t('pages.adminSecurity.tabTripleLabels')}
    </button>
    <button class="tab" class:active={activeTab === 'registration'} role="tab"
      on:click={() => switchTab('registration')}>
      <UserPlus size={15} /> {$t('pages.adminSecurity.tabRegistration')}
    </button>
  </div>

  <!-- ── Registration (guest self-registration toggle) ── -->
  {#if activeTab === 'registration'}
    <section class="tab-panel">
      <div class="panel-header">
        <p class="hint">{$t('pages.adminSecurity.guestRegHint')}</p>
      </div>
      {#if guestReg === null}
        <p class="hint"><Loader2 size={14} class="spin" /> {$t('common.loading')}</p>
      {:else}
        <div class="form-card">
          <h3>{$t('pages.adminSecurity.guestRegTitle')}</h3>
          <p class="hint">
            {guestReg.enabled
              ? $t('pages.adminSecurity.guestRegOnState')
              : $t('pages.adminSecurity.guestRegOffState')}
          </p>
          <button class="btn btn-sm" disabled={guestRegBusy} on:click={toggleGuestReg}>
            {#if guestRegBusy}<Loader2 size={14} class="spin" />{/if}
            {guestReg.enabled
              ? $t('pages.adminSecurity.guestRegTurnOff')
              : $t('pages.adminSecurity.guestRegTurnOn')}
          </button>
          {#if guestSwept}
            <p class="hint">
              {guestSwept.enabled
                ? $t('pages.adminSecurity.guestSweptEnabled', { values: { n: guestSwept.guests_swept } })
                : $t('pages.adminSecurity.guestSweptDisabled', { values: { n: guestSwept.guests_swept } })}
            </p>
          {/if}
        </div>
      {/if}
    </section>
  {/if}

  <!-- ── SSO / OAuth Providers ── -->
  {#if activeTab === 'providers'}
    <section class="tab-panel">
      <div class="panel-header">
        <p class="hint">{$t('pages.adminSecurity.providersHint')}</p>
        <button class="btn btn-sm" on:click={openCreateProvider}><Plus size={14}/> {$t('pages.adminSecurity.addProvider')}</button>
      </div>

      {#if showProviderForm}
        <div class="form-card">
          <h3>{editingProvider ? $t('pages.adminSecurity.editProvider') : $t('pages.adminSecurity.newProvider')}</h3>
          {#if providerError}<p class="error">{providerError}</p>{/if}

          <div class="form-grid">
            <div class="form-group">
              <label for="prov-name">{$t('pages.adminSecurity.displayName')}</label>
              <input id="prov-name" bind:value={providerForm.name} placeholder="Azure AD" required />
            </div>
            <div class="form-group">
              <label for="prov-slug">{$t('pages.adminSecurity.slug')} <span class="hint-sm">{$t('pages.adminSecurity.slugHint')}</span></label>
              <input id="prov-slug" bind:value={providerForm.slug} placeholder="azure-ad" required />
            </div>
            <div class="form-group">
              <label for="prov-type">{$t('pages.adminSecurity.providerType')}</label>
              <Select id="prov-type" bind:value={providerForm.provider_type} options={[
                { value: 'oidc', label: $t('pages.adminSecurity.providerTypeOidc') },
                { value: 'azure_ad', label: $t('pages.adminSecurity.providerTypeAzure') },
                { value: 'saml', label: $t('pages.adminSecurity.providerTypeSaml') },
              ]} />
            </div>
            <div class="form-group">
              <label for="prov-client-id">{$t('pages.adminSecurity.clientId')}</label>
              <input id="prov-client-id" bind:value={providerForm.client_id} placeholder={$t('pages.adminSecurity.clientIdPlaceholder')} />
            </div>
            <div class="form-group">
              <label for="prov-client-secret">{$t('pages.adminSecurity.clientSecret')} {editingProvider ? $t('pages.adminSecurity.clientSecretKeep') : ''}</label>
              <input id="prov-client-secret" type="password" bind:value={providerForm.client_secret} placeholder="••••••••" />
            </div>
            {#if providerForm.provider_type !== 'saml'}
              <div class="form-group full">
                <label for="prov-discovery-url">{$t('pages.adminSecurity.discoveryUrl')} <span class="hint-sm">{$t('pages.adminSecurity.discoveryUrlHint')}</span></label>
                <input id="prov-discovery-url" bind:value={providerForm.discovery_url}
                  placeholder="https://login.microsoftonline.com/[TENANT_ID]/v2.0/.well-known/openid-configuration" />
              </div>
              <div class="form-group">
                <label for="prov-scopes">{$t('pages.adminSecurity.scopes')} <span class="hint-sm">{$t('pages.adminSecurity.scopesHint')}</span></label>
                <input id="prov-scopes" bind:value={providerForm.scopes} placeholder="openid email profile" />
              </div>
            {/if}
            {#if providerForm.provider_type === 'saml'}
              <div class="form-group full">
                <label for="prov-idp-metadata">{$t('pages.adminSecurity.idpMetadata')}</label>
                <input id="prov-idp-metadata" bind:value={providerForm.discovery_url} placeholder="https://login.microsoftonline.com/[TENANT_ID]/federationmetadata/2007-06/federationmetadata.xml" />
              </div>
            {/if}
            <div class="form-group">
              <label for="prov-role-claim">{$t('pages.adminSecurity.roleClaimKey')} <span class="hint-sm">{$t('pages.adminSecurity.roleClaimKeyHint')}</span></label>
              <input id="prov-role-claim" bind:value={providerForm.role_claim} placeholder="roles" />
            </div>
            <div class="form-group">
              <label for="prov-default-role">{$t('pages.adminSecurity.defaultRole')}</label>
              <Select id="prov-default-role" bind:value={providerForm.default_role} options={[
                { value: 'user', label: $t('pages.adminSecurity.roleUser') },
                { value: 'admin', label: $t('pages.adminSecurity.roleAdmin') },
                { value: 'super_admin', label: $t('pages.adminSecurity.roleSuperAdmin') },
              ]} />
            </div>
            <div class="form-group full">
              <label for="prov-role-map">{$t('pages.adminSecurity.roleClaimMap')} <span class="hint-sm">{$t('pages.adminSecurity.roleClaimMapHint')}</span></label>
              <textarea id="prov-role-map" bind:value={providerForm.role_claim_map} rows="3"
                placeholder={ROLE_CLAIM_MAP_PLACEHOLDER}></textarea>
            </div>
            <div class="form-group full toggles-section">
              <div class="toggle-row">
                <div class="toggle-info">
                  <span class="toggle-label">{$t('pages.adminSecurity.autoProvision')}</span>
                  <span class="toggle-desc">{$t('pages.adminSecurity.autoProvisionDesc')}</span>
                </div>
                <button
                  type="button"
                  class="ios-toggle"
                  class:ios-toggle-on={providerForm.auto_provision}
                  role="switch"
                  aria-checked={providerForm.auto_provision}
                  aria-label={$t('pages.adminSecurity.autoProvision')}
                  on:click={() => providerForm.auto_provision = !providerForm.auto_provision}
                ><span class="ios-thumb"></span></button>
              </div>
              <div class="toggle-row">
                <div class="toggle-info">
                  <span class="toggle-label">{$t('pages.adminSecurity.enabled')}</span>
                  <span class="toggle-desc">{$t('pages.adminSecurity.enabledDesc')}</span>
                </div>
                <button
                  type="button"
                  class="ios-toggle"
                  class:ios-toggle-on={providerForm.enabled}
                  role="switch"
                  aria-checked={providerForm.enabled}
                  aria-label={$t('pages.adminSecurity.enabled')}
                  on:click={() => providerForm.enabled = !providerForm.enabled}
                ><span class="ios-thumb"></span></button>
              </div>
            </div>
          </div>

          <div class="form-actions">
            <button class="btn btn-sm btn-ghost" on:click={() => showProviderForm = false}>{$t('system.cancel')}</button>
            <button class="btn btn-sm" on:click={submitProvider} disabled={providerLoading}>
              {#if providerLoading}<Loader2 size={14} class="spin" />{/if}
              {editingProvider ? $t('pages.adminSecurity.saveChanges') : $t('pages.adminSecurity.createProvider')}
            </button>
          </div>
        </div>
      {/if}

      {#if providersLoading}
        <div class="loading"><Loader2 size={18} class="spin" /> {$t('system.loading')}</div>
      {:else if providers.length === 0 && !showProviderForm}
        <div class="empty-state">
          <Key size={32} />
          <p>{$t('pages.adminSecurity.providersEmpty1')}<br/>{$t('pages.adminSecurity.providersEmpty2')}</p>
        </div>
      {:else}
        <table class="data-table">
          <thead><tr><th>{$t('pages.adminSecurity.colName')}</th><th>{$t('pages.adminSecurity.colType')}</th><th>{$t('pages.adminSecurity.colSlug')}</th><th>{$t('pages.adminSecurity.colAutoProvision')}</th><th>{$t('pages.adminSecurity.colStatus')}</th><th></th></tr></thead>
          <tbody>
            {#each providers as p}
              <tr>
                <td><strong>{p.name}</strong></td>
                <td><code>{p.provider_type}</code></td>
                <td><code>{p.slug}</code></td>
                <td>
                  {#if p.auto_provision}
                    <span class="cell-flag on"><Check size={13} /> {$t('pages.adminSecurity.on')}</span>
                  {:else}
                    <span class="cell-flag off"><Minus size={13} /> {$t('pages.adminSecurity.off')}</span>
                  {/if}
                </td>
                <td>
                  <span class="status-badge {p.enabled ? 'badge-green' : 'badge-gray'}">
                    <span class="status-dot"></span>{p.enabled ? $t('pages.adminSecurity.statusEnabled') : $t('pages.adminSecurity.statusDisabled')}
                  </span>
                </td>
                <td class="actions">
                  <button class="btn-icon" title={$t('system.edit')} on:click={() => openEditProvider(p)}><Edit3 size={14}/></button>
                  <button class="btn-icon btn-danger" title={$t('system.delete')} on:click={() => deleteProviderTarget = p.id}><Trash2 size={14}/></button>
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      {/if}
    </section>
  {/if}

  <!-- ── Endpoint ACL ── -->
  {#if activeTab === 'endpoint-acl'}
    <section class="tab-panel">
      <div class="panel-header">
        <p class="hint">{$t('pages.adminSecurity.endpointHint1')} <code>deny</code> {$t('pages.adminSecurity.endpointHint2')} <code>allow</code> {$t('pages.adminSecurity.endpointHint3')}.</p>
        <button class="btn btn-sm" on:click={openCreateEndpoint}><Plus size={14}/> {$t('pages.adminSecurity.addRule')}</button>
      </div>

      {#if showEndpointForm}
        <div class="form-card">
          <h3>{editingEndpoint ? $t('pages.adminSecurity.editRule') : $t('pages.adminSecurity.newRule')}</h3>
          {#if endpointError}<p class="error">{endpointError}</p>{/if}
          <div class="form-grid">
            <div class="form-group">
              <label for="ep-principal-type">{$t('pages.adminSecurity.principalType')}</label>
              <Select id="ep-principal-type" bind:value={endpointForm.principal_type} options={[
                { value: 'user', label: $t('pages.adminSecurity.principalUser') },
                { value: 'role', label: $t('pages.adminSecurity.principalRole') },
                { value: 'organisation', label: $t('pages.adminSecurity.principalOrganisation') },
                { value: 'group', label: $t('pages.adminSecurity.principalGroup') },
              ]} />
            </div>
            <div class="form-group">
              <label for="ep-principal-id">{$t('pages.adminSecurity.principalIdValue')} <span class="hint-sm">{$t('pages.adminSecurity.principalIdValueHint')}</span></label>
              <Combobox id="ep-principal-id" bind:value={endpointForm.principal_id}
                suggestions={principalSuggestions(endpointForm.principal_type).map(s => ({ value: s.value, label: s.label }))}
                placeholder="user / admin / org-id" />
            </div>
            <div class="form-group">
              <label for="ep-path">{$t('pages.adminSecurity.pathPattern')} <span class="hint-sm">{$t('pages.adminSecurity.pathPatternHint')}</span></label>
              <input id="ep-path" bind:value={endpointForm.path_pattern} placeholder="/api/admin/**" />
            </div>
            <div class="form-group">
              <label for="ep-method">{$t('pages.adminSecurity.httpMethod')}</label>
              <Select id="ep-method" bind:value={endpointForm.method} options={[
                { value: '*', label: $t('pages.adminSecurity.methodAny') },
                { value: 'GET', label: 'GET' },
                { value: 'POST', label: 'POST' },
                { value: 'PUT', label: 'PUT' },
                { value: 'DELETE', label: 'DELETE' },
              ]} />
            </div>
            <div class="form-group">
              <label for="ep-effect">{$t('pages.adminSecurity.effect')}</label>
              <Select id="ep-effect" bind:value={endpointForm.effect} options={[
                { value: 'allow', label: $t('pages.adminSecurity.effectAllow') },
                { value: 'deny', label: $t('pages.adminSecurity.effectDeny') },
              ]} />
            </div>
            <div class="form-group">
              <label for="ep-priority">{$t('pages.adminSecurity.priority')} <span class="hint-sm">{$t('pages.adminSecurity.priorityHint')}</span></label>
              <input id="ep-priority" type="number" bind:value={endpointForm.priority} min="0" max="9999" />
            </div>
          </div>
          <div class="form-actions">
            <button class="btn btn-sm btn-ghost" on:click={() => showEndpointForm = false}>{$t('system.cancel')}</button>
            <button class="btn btn-sm" on:click={submitEndpoint} disabled={endpointSaving}>
              {#if endpointSaving}<Loader2 size={14} class="spin" />{/if}
              {editingEndpoint ? $t('system.save') : $t('system.create')}
            </button>
          </div>
        </div>
      {/if}

      {#if endpointLoading}
        <div class="loading"><Loader2 size={18} class="spin" /> {$t('system.loading')}</div>
      {:else if endpointRules.length === 0 && !showEndpointForm}
        <div class="empty-state"><Server size={32}/><p>{$t('pages.adminSecurity.endpointEmpty1')}<br/>{$t('pages.adminSecurity.endpointEmpty2')}</p></div>
      {:else}
        <table class="data-table">
          <thead><tr><th>{$t('pages.adminSecurity.colPrincipal')}</th><th>{$t('pages.adminSecurity.pathPattern')}</th><th>{$t('pages.adminSecurity.colMethod')}</th><th>{$t('pages.adminSecurity.effect')}</th><th>{$t('pages.adminSecurity.priority')}</th><th></th></tr></thead>
          <tbody>
            {#each endpointRules as r}
              <tr>
                <td><code>{r.principal_type}:{r.principal_id}</code></td>
                <td><code>{r.path_pattern}</code></td>
                <td><code>{r.method}</code></td>
                <td><span class="badge {r.effect === 'allow' ? 'badge-green' : 'badge-red'}">{r.effect}</span></td>
                <td>{r.priority}</td>
                <td class="actions">
                  <button class="btn-icon" title={$t('system.edit')} on:click={() => openEditEndpoint(r)}><Edit3 size={14}/></button>
                  <button class="btn-icon btn-danger" title={$t('system.delete')} on:click={() => deleteEndpointTarget = r.id}><Trash2 size={14}/></button>
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      {/if}
    </section>
  {/if}

  <!-- ── Graph ACL ── -->
  {#if activeTab === 'graph-acl'}
    <section class="tab-panel">
      <div class="panel-header">
        <p class="hint">{$t('pages.adminSecurity.graphHint')}</p>
        <button class="btn btn-sm" on:click={() => { showGraphForm = true; graphError = ''; }}><Plus size={14}/> {$t('pages.adminSecurity.grantPermission')}</button>
      </div>

      {#if showGraphForm}
        <div class="form-card">
          <h3>{$t('pages.adminSecurity.grantGraphPermission')}</h3>
          {#if graphError}<p class="error">{graphError}</p>{/if}
          <div class="form-grid">
            <div class="form-group">
              <label for="g-principal-type">{$t('pages.adminSecurity.principalType')}</label>
              <Select id="g-principal-type" bind:value={graphForm.principal_type} options={[
                { value: 'user', label: $t('pages.adminSecurity.principalUser') },
                { value: 'role', label: $t('pages.adminSecurity.principalRole') },
                { value: 'organisation', label: $t('pages.adminSecurity.principalOrganisation') },
                { value: 'group', label: $t('pages.adminSecurity.principalGroup') },
                { value: 'public', label: $t('pages.adminSecurity.principalPublic') },
              ]} />
            </div>
            <div class="form-group">
              <label for="g-principal-id">{$t('pages.adminSecurity.principalId')}</label>
              <Combobox id="g-principal-id" bind:value={graphForm.principal_id}
                suggestions={principalSuggestions(graphForm.principal_type).map(s => ({ value: s.value, label: s.label }))}
                placeholder="user-id / role name / org-id"
                disabled={graphForm.principal_type === 'public'} />
            </div>
            <div class="form-group full">
              <label for="g-graph-iri">{$t('pages.adminSecurity.graphIri')}</label>
              <Combobox id="g-graph-iri" bind:value={graphForm.graph_iri}
                suggestions={availableGraphIris}
                placeholder="https://example.org/my-graph" />
            </div>
            <div class="form-group">
              <label for="g-permission">{$t('pages.adminSecurity.permission')}</label>
              <Select id="g-permission" bind:value={graphForm.permission}
                options={GRAPH_PERMISSIONS.map(p => ({ value: p.value, label: p.label }))} />
            </div>
          </div>
          <div class="form-actions">
            <button class="btn btn-sm btn-ghost" on:click={() => showGraphForm = false}>{$t('system.cancel')}</button>
            <button class="btn btn-sm" on:click={submitGraphGrant} disabled={graphSaving}>
              {#if graphSaving}<Loader2 size={14} class="spin" />{/if} {$t('pages.adminSecurity.grant')}
            </button>
          </div>
        </div>
      {/if}

      {#if graphLoading}
        <div class="loading"><Loader2 size={18} class="spin" /> {$t('system.loading')}</div>
      {:else if graphRules.length === 0 && !showGraphForm}
        <div class="empty-state"><Globe size={32}/><p>{$t('pages.adminSecurity.graphEmpty1')}<br/>{$t('pages.adminSecurity.graphEmpty2')}</p></div>
      {:else}
        <table class="data-table">
          <thead><tr><th>{$t('pages.adminSecurity.colPrincipal')}</th><th>{$t('pages.adminSecurity.graphIri')}</th><th>{$t('pages.adminSecurity.permission')}</th><th></th></tr></thead>
          <tbody>
            {#each graphRules as r}
              <tr>
                <td><code>{r.principal_type}:{r.principal_id}</code></td>
                <td class="iri-cell" title={r.graph_iri}>{r.graph_iri}</td>
                <td><span class="badge {r.permission === 'admin' ? 'badge-purple' : r.permission === 'write' ? 'badge-blue' : 'badge-gray'}">{r.permission}</span></td>
                <td class="actions">
                  <button class="btn-icon btn-danger" title={$t('pages.adminSecurity.revoke')} on:click={() => revokeGraphTarget = r.id}><Trash2 size={14}/></button>
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      {/if}
    </section>
  {/if}

  <!-- ── Triple Security Labels ── -->
  {#if activeTab === 'triple-labels'}
    <section class="tab-panel">
      <div class="panel-header">
        <p class="hint">{$t('pages.adminSecurity.tripleHint')}</p>
        <button class="btn btn-sm" on:click={() => { showTripleForm = true; tripleError = ''; }}><Plus size={14}/> {$t('pages.adminSecurity.addLabel')}</button>
      </div>

      {#if showTripleForm}
        <div class="form-card">
          <h3>{$t('pages.adminSecurity.addTripleLabel')}</h3>
          {#if tripleError}<p class="error">{tripleError}</p>{/if}
          <div class="form-grid">
            <div class="form-group">
              <label for="tl-subject">{$t('pages.adminSecurity.subjectIri')}</label>
              <input id="tl-subject" bind:value={tripleForm.subject_iri} placeholder="https://example.org/subject" />
            </div>
            <div class="form-group">
              <label for="tl-predicate">{$t('pages.adminSecurity.predicateIri')}</label>
              <input id="tl-predicate" bind:value={tripleForm.predicate_iri} placeholder="https://schema.org/salary" />
            </div>
            <div class="form-group full">
              <label for="tl-object">{$t('pages.adminSecurity.objectValue')} <span class="hint-sm">{$t('pages.adminSecurity.objectValueHint')}</span></label>
              <input id="tl-object" bind:value={tripleForm.object_value} placeholder="&quot;confidential&quot; or https://…" />
            </div>
            <div class="form-group full">
              <label for="tl-label-graph">{$t('pages.adminSecurity.labelGraphIri')} <span class="hint-sm">{$t('pages.adminSecurity.labelGraphIriHint')}</span></label>
              <Combobox id="tl-label-graph" bind:value={tripleForm.label_graph_iri}
                suggestions={availableGraphIris}
                placeholder="https://example.org/labels/confidential" />
            </div>
          </div>
          <div class="form-actions">
            <button class="btn btn-sm btn-ghost" on:click={() => showTripleForm = false}>{$t('system.cancel')}</button>
            <button class="btn btn-sm" on:click={submitTripleLabel} disabled={tripleSaving}>
              {#if tripleSaving}<Loader2 size={14} class="spin" />{/if} {$t('pages.adminSecurity.addLabel')}
            </button>
          </div>
        </div>
      {/if}

      {#if tripleLoading}
        <div class="loading"><Loader2 size={18} class="spin" /> {$t('system.loading')}</div>
      {:else if tripleLabels.length === 0 && !showTripleForm}
        <div class="empty-state"><Lock size={32}/><p>{$t('pages.adminSecurity.tripleEmpty1')}<br/>{$t('pages.adminSecurity.tripleEmpty2')}</p></div>
      {:else}
        <table class="data-table">
          <thead><tr><th>{$t('pages.adminSecurity.colSubject')}</th><th>{$t('pages.adminSecurity.colPredicate')}</th><th>{$t('pages.adminSecurity.colObject')}</th><th>{$t('pages.adminSecurity.colLabelGraph')}</th><th></th></tr></thead>
          <tbody>
            {#each tripleLabels as l}
              <tr>
                <td class="iri-cell" title={l.subject_iri}>{l.subject_iri}</td>
                <td class="iri-cell" title={l.predicate_iri}>{l.predicate_iri}</td>
                <td class="iri-cell">{l.object_value}</td>
                <td class="iri-cell" title={l.label_graph_iri}>{l.label_graph_iri}</td>
                <td class="actions">
                  <button class="btn-icon btn-danger" title={$t('system.remove')} on:click={() => deleteLabelTarget = l.id}><Trash2 size={14}/></button>
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      {/if}
    </section>
  {/if}
</div>

{#if deleteProviderTarget !== null}
  <ConfirmModal
    title={$t('pages.adminSecurity.deleteProviderTitle')}
    message={$t('pages.adminSecurity.deleteProviderMessage')}
    confirmLabel={$t('pages.adminSecurity.deleteProviderConfirm')}
    on:confirm={doDeleteProvider}
    on:cancel={() => deleteProviderTarget = null}
  />
{/if}

{#if deleteEndpointTarget !== null}
  <ConfirmModal
    title={$t('pages.adminSecurity.deleteRuleTitle')}
    message={$t('pages.adminSecurity.cannotBeUndone')}
    confirmLabel={$t('pages.adminSecurity.deleteRuleConfirm')}
    on:confirm={doDeleteEndpointRule}
    on:cancel={() => deleteEndpointTarget = null}
  />
{/if}

{#if revokeGraphTarget !== null}
  <ConfirmModal
    title={$t('pages.adminSecurity.revokeGraphTitle')}
    message={$t('pages.adminSecurity.revokeGraphMessage')}
    confirmLabel={$t('pages.adminSecurity.revoke')}
    confirmVariant="warning"
    on:confirm={doRevokeGraph}
    on:cancel={() => revokeGraphTarget = null}
  />
{/if}

{#if deleteLabelTarget !== null}
  <ConfirmModal
    title={$t('pages.adminSecurity.removeLabelTitle')}
    message={$t('pages.adminSecurity.cannotBeUndone')}
    confirmLabel={$t('pages.adminSecurity.removeLabelConfirm')}
    on:confirm={doDeleteTripleLabel}
    on:cancel={() => deleteLabelTarget = null}
  />
{/if}

<style>
  .admin-security { max-width: 1100px; margin: 0 auto; padding: 1.5rem 1rem; }

  .page-header {
    display: flex; align-items: center; gap: 0.75rem;
    margin-bottom: 1.5rem;
  }
  .page-header h1 { margin: 0; font-size: 1.5rem; }

  .tabs {
    display: flex; gap: 0; border-bottom: 2px solid var(--color-border, #ddd);
    margin-bottom: 1.5rem; overflow-x: auto;
  }
  .tab {
    display: flex; align-items: center; gap: 0.4rem;
    padding: 0.6rem 1.1rem; border: none; background: none;
    color: var(--color-muted, #888); font-size: 0.9rem; cursor: pointer;
    border-bottom: 2px solid transparent; margin-bottom: -2px;
    white-space: nowrap; transition: color 0.15s;
  }
  .tab:hover { color: var(--color-text, #333); }
  .tab.active { color: var(--color-primary, #4f46e5); border-bottom-color: var(--color-primary, #4f46e5); font-weight: 600; }

  .panel-header { display: flex; align-items: flex-start; justify-content: space-between; gap: 1rem; margin-bottom: 1rem; }
  .hint { color: var(--color-muted, #666); font-size: 0.85rem; margin: 0; }
  .hint-sm { color: var(--color-muted, #888); font-size: 0.78rem; font-weight: normal; }

  .form-card {
    background: var(--color-surface, #fff);
    border: 1px solid var(--color-border, #ddd);
    border-radius: 8px; padding: 1.25rem; margin-bottom: 1.25rem;
  }
  .form-card h3 { margin: 0 0 1rem; font-size: 1rem; }
  .form-grid { display: grid; grid-template-columns: 1fr 1fr; gap: 0.75rem 1rem; }
  .form-group { display: flex; flex-direction: column; gap: 0.3rem; }
  .form-group.full { grid-column: 1 / -1; }
  .form-group label { font-size: 0.85rem; font-weight: 500; }
  .form-group input, .form-group textarea {
    padding: 0.45rem 0.65rem; border: 1px solid var(--color-border, #ddd);
    border-radius: 5px; font-size: 0.9rem; background: var(--color-surface, #fff);
    color: var(--color-text, #333); width: 100%; box-sizing: border-box;
  }
  /* Toggle rows (auto-provision / enabled) */
  .toggles-section {
    gap: 0; padding: 0.25rem 1rem; margin-top: 0.25rem;
    background: var(--bg-soft, #f8fafc);
    border: 1px solid var(--line-soft, #e2e8f0);
    border-radius: 10px;
  }
  .toggle-row {
    display: flex; align-items: center; justify-content: space-between; gap: 1.25rem;
    padding: 0.8rem 0;
  }
  .toggle-row + .toggle-row { border-top: 1px solid var(--line-soft, #eef2f6); }
  .toggle-info { display: flex; flex-direction: column; gap: 0.2rem; min-width: 0; }
  .toggle-label { font-size: 0.9rem; font-weight: 600; color: var(--ink-800, #1e293b); }
  .toggle-desc { font-size: 0.78rem; color: var(--color-muted, #64748b); line-height: 1.4; }

  .ios-toggle {
    position: relative; display: block;
    width: 44px; min-width: 44px; height: 26px;
    border-radius: 13px; border: none; cursor: pointer;
    background: var(--ink-300, #cbd5e1);
    transition: background 0.22s ease;
    padding: 0; flex-shrink: 0;
  }
  .ios-toggle-on { background: var(--success-500, #10b981); }
  .ios-thumb {
    position: absolute; top: 3px; left: 3px;
    width: 20px; height: 20px; border-radius: 50%;
    background: white; box-shadow: 0 1px 4px rgba(0,0,0,0.25);
    transition: left 0.22s ease;
  }
  .ios-toggle-on .ios-thumb { left: 21px; }

  /* Table flag (auto-provision on/off) */
  .cell-flag { display: inline-flex; align-items: center; gap: 0.3rem; font-size: 0.82rem; font-weight: 500; }
  .cell-flag.on { color: var(--success-500, #16a34a); }
  .cell-flag.off { color: var(--color-muted, #94a3b8); }

  /* Status badge with leading dot */
  .status-badge { display: inline-flex; align-items: center; gap: 0.4rem; padding: 0.15rem 0.55rem; border-radius: 999px; font-size: 0.75rem; font-weight: 600; }
  .status-dot { width: 7px; height: 7px; border-radius: 50%; background: currentColor; flex-shrink: 0; }

  .form-actions { display: flex; justify-content: flex-end; gap: 0.5rem; margin-top: 1rem; }

  .data-table { width: 100%; border-collapse: collapse; font-size: 0.88rem; }
  .data-table th { text-align: left; padding: 0.55rem 0.75rem; border-bottom: 2px solid var(--color-border, #ddd); color: var(--color-muted, #666); font-weight: 600; white-space: nowrap; }
  .data-table td { padding: 0.55rem 0.75rem; border-bottom: 1px solid var(--color-border-light, #eee); vertical-align: middle; }
  .data-table tr:last-child td { border-bottom: none; }
  .iri-cell { max-width: 240px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: 0.8rem; font-family: monospace; }

  .actions { display: flex; gap: 0.35rem; justify-content: flex-end; }
  .btn-icon { background: none; border: 1px solid transparent; border-radius: 4px; padding: 0.3rem; cursor: pointer; color: var(--color-muted, #666); display: inline-flex; }
  .btn-icon:hover { background: var(--color-surface-2, #f5f5f5); border-color: var(--color-border, #ddd); }
  .btn-icon.btn-danger:hover { color: var(--color-danger, #dc2626); background: #fee2e2; border-color: #fecaca; }

  .badge { display: inline-block; padding: 0.15rem 0.5rem; border-radius: 999px; font-size: 0.75rem; font-weight: 600; }
  .badge-green { background: #dcfce7; color: #166534; }
  .badge-gray  { background: #f3f4f6; color: #374151; }
  .badge-red   { background: #fee2e2; color: #991b1b; }
  .badge-blue  { background: #dbeafe; color: #1e40af; }
  .badge-purple{ background: #ede9fe; color: #4c1d95; }

  .loading { display: flex; align-items: center; gap: 0.5rem; color: var(--color-muted, #888); padding: 2rem 0; }
  .empty-state { display: flex; flex-direction: column; align-items: center; gap: 0.75rem; padding: 3rem 1rem; color: var(--color-muted, #888); text-align: center; }
  .empty-state p { margin: 0; }

  .error { color: var(--color-danger, #dc2626); font-size: 0.88rem; margin: 0 0 0.5rem; }

  :global(.spin) { animation: spin 1s linear infinite; }
  @keyframes spin { to { transform: rotate(360deg); } }

  @media (max-width: 640px) {
    .form-grid { grid-template-columns: 1fr; }
    .form-group.full { grid-column: unset; }
    .panel-header { flex-direction: column; align-items: flex-start; }
  }

  :global(:is([data-theme="dark"], .dark)) .admin-security {
    --color-border: var(--line-strong);
    --color-border-light: var(--line-soft);
    --color-muted: var(--ink-500);
    --color-text: var(--ink-900);
    --color-surface: var(--bg-strong);
    --color-surface-2: rgba(255,255,255,0.06);
    --color-primary: var(--brand-700);
    --color-danger: #fca5a5;
  }
  :global(:is([data-theme="dark"], .dark)) .btn-icon.btn-danger:hover { background: rgba(239,68,68,0.18); border-color: rgba(239,68,68,0.35); }
  :global(:is([data-theme="dark"], .dark)) .badge-green { background: rgba(16,185,129,0.18); color: #6ee7b7; }
  :global(:is([data-theme="dark"], .dark)) .badge-gray { background: rgba(255,255,255,0.08); color: var(--ink-400); }
  :global(:is([data-theme="dark"], .dark)) .badge-red { background: rgba(239,68,68,0.18); color: #fca5a5; }
  :global(:is([data-theme="dark"], .dark)) .badge-blue { background: rgba(59,130,246,0.2); color: #93c5fd; }
  :global(:is([data-theme="dark"], .dark)) .badge-purple { background: rgba(139,92,246,0.2); color: #c4b5fd; }
</style>
