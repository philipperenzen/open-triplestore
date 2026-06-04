<script>
  import { adminListUsers, adminCreateUser, adminUpdateUser, adminDeleteUser, adminResetPassword, adminPurgeUser } from '../lib/api.js';
  import { user as currentUserStore, isAdmin, authInitialized } from '../lib/stores.js';
  import { SYSTEM_ROLES } from '../lib/permissions.js';
  import { navigate } from '../lib/router/index.js';
  import { Users, Plus, Trash2, Edit3, Key, Loader2, Search, ChevronLeft, ChevronRight, X, ShieldOff } from 'lucide-svelte';
  import ConfirmModal from '../components/ConfirmModal.svelte';
  import Avatar from '../components/Avatar.svelte';
  import Select from '../components/Select.svelte';
  import { t } from 'svelte-i18n';

  let users = [];
  let total = 0;
  let page = 1;
  let limit = 20;
  let search = '';
  let loading = false;
  let searchTimeout;

  // Create user modal
  let showCreateModal = false;
  let createForm = { username: '', email: '', password: '', role: 'user', can_publish: false };
  let createError = '';
  let createLoading = false;

  // Edit user modal
  let showEditModal = false;
  let editUser = null;
  let editForm = { email: '', role: '', is_active: true, can_publish: false };
  let editError = '';
  let editLoading = false;

  // Reset password modal
  let showResetModal = false;
  let resetUserId = '';
  let resetUsername = '';
  let resetPassword = '';
  let resetError = '';
  let resetLoading = false;

  let currentUser;
  currentUserStore.subscribe(v => currentUser = v);

  let _guardChecked = false;
  $: if ($authInitialized && !_guardChecked) {
    _guardChecked = true;
    if (!$isAdmin) navigate('/');
    else loadUsers();
  }

  async function loadUsers() {
    loading = true;
    try {
      const params = { page, limit };
      if (search) params.search = search;
      const res = await adminListUsers(params);
      users = res.users;
      total = res.total;
    } catch (e) {
      alert(e.message);
    }
    loading = false;
  }

  function handleSearch() {
    clearTimeout(searchTimeout);
    searchTimeout = setTimeout(() => {
      page = 1;
      loadUsers();
    }, 300);
  }

  function nextPage() {
    if (page * limit < total) { page++; loadUsers(); }
  }

  function prevPage() {
    if (page > 1) { page--; loadUsers(); }
  }

  async function handleCreate() {
    createError = '';
    createLoading = true;
    try {
      await adminCreateUser(createForm);
      showCreateModal = false;
      createForm = { username: '', email: '', password: '', role: 'user', can_publish: false };
      await loadUsers();
    } catch (e) {
      createError = e.message;
    }
    createLoading = false;
  }

  function openEdit(u) {
    editUser = u;
    editForm = { email: u.email, role: u.role, is_active: u.is_active, can_publish: u.can_publish ?? false };
    editError = '';
    showEditModal = true;
  }

  async function handleEdit() {
    editError = '';
    editLoading = true;
    try {
      await adminUpdateUser(editUser.id, editForm);
      showEditModal = false;
      await loadUsers();
    } catch (e) {
      editError = e.message;
    }
    editLoading = false;
  }

  function openReset(u) {
    resetUserId = u.id;
    resetUsername = u.username;
    resetPassword = '';
    resetError = '';
    showResetModal = true;
  }

  async function handleReset() {
    resetError = '';
    if (resetPassword.length < 8) {
      resetError = $t('pages.admin.passwordMinLength');
      return;
    }
    resetLoading = true;
    try {
      await adminResetPassword(resetUserId, resetPassword);
      showResetModal = false;
    } catch (e) {
      resetError = e.message;
    }
    resetLoading = false;
  }

  let deactivateTarget = null;
  let purgeTarget = null;

  async function doDeactivate() {
    try {
      await adminDeleteUser(deactivateTarget.id);
      deactivateTarget = null;
      await loadUsers();
    } catch (e) {
      deactivateTarget = null;
      alert(e.message);
    }
  }

  async function doPurge() {
    try {
      await adminPurgeUser(purgeTarget.id);
      purgeTarget = null;
      await loadUsers();
    } catch (e) {
      purgeTarget = null;
      alert(e.message);
    }
  }

  function roleBadgeClass(role) {
    if (role === 'super_admin') return 'badge-super';
    if (role === 'admin') return 'badge-admin';
    return 'badge-user';
  }

  $: totalPages = Math.ceil(total / limit);
</script>

<div class="admin-users">
  <div class="header-row">
    <h2><Users size={20} /> {$t('pages.admin.title')}</h2>
    <button class="btn" on:click={() => { showCreateModal = true; createError = ''; }}>
      <Plus size={14} /> {$t('pages.admin.newUser')}
    </button>
  </div>

  <div class="search-bar">
    <Search size={16} />
    <input placeholder={$t('pages.admin.searchPlaceholder')} bind:value={search} on:input={handleSearch} />
  </div>

  {#if loading}
    <div class="loading"><Loader2 size={24} class="animate-spin" /> {$t('system.loading')}</div>
  {:else}
    <table class="data-table">
      <thead>
        <tr>
          <th>{$t('pages.admin.colUsername')}</th>
          <th>{$t('pages.admin.colEmail')}</th>
          <th>{$t('pages.admin.colRole')}</th>
          <th>{$t('pages.admin.colStatus')}</th>
          <th>{$t('pages.admin.colProfile')}</th>
          <th>{$t('pages.admin.colCreated')}</th>
          <th>{$t('pages.admin.colActions')}</th>
        </tr>
      </thead>
      <tbody>
        {#each users as u}
          <tr class:inactive={!u.is_active}>
            <td>
              <span style="display:inline-flex;align-items:center;gap:0.5rem;">
                <Avatar kind="user" id={u.id} name={u.username} hasImage={!!u.avatar_key} size={24} />
                <strong>{u.username}</strong>
              </span>
            </td>
            <td>{u.email}</td>
            <td>
              <span class="role-badge {roleBadgeClass(u.role)}">{u.role.replace('_', ' ')}</span>
              {#if u.can_publish && !['admin','super_admin'].includes(u.role)}
                <span class="pub-badge">{$t('pages.admin.publisherBadge')}</span>
              {/if}
            </td>
            <td>{u.is_active ? $t('pages.admin.statusActive') : $t('pages.admin.statusInactive')}</td>
            <td><span class="vis-badge" class:public={u.is_public}>{u.is_public ? $t('pages.admin.visPublic') : $t('pages.admin.visPrivate')}</span></td>
            <td>{new Date(u.created_at).toLocaleDateString()}</td>
            <td class="actions">
              <button class="btn btn-sm btn-ghost" on:click={() => openEdit(u)} title={$t('system.edit')}>
                <Edit3 size={14} />
              </button>
              <button class="btn btn-sm btn-ghost" on:click={() => openReset(u)} title={$t('pages.admin.resetPassword')}>
                <Key size={14} />
              </button>
              {#if u.id !== currentUser?.id && u.is_active}
                <button class="btn btn-sm btn-ghost btn-danger" on:click={() => deactivateTarget = u} title={$t('pages.admin.deactivate')}>
                  <Trash2 size={14} />
                </button>
              {/if}
              {#if u.id !== currentUser?.id && !u.is_active}
                <button class="btn btn-sm btn-ghost btn-purge" on:click={() => purgeTarget = u} title={$t('pages.admin.permanentlyDelete')}>
                  <ShieldOff size={14} />
                </button>
              {/if}
            </td>
          </tr>
        {/each}
      </tbody>
    </table>

    {#if totalPages > 1}
      <div class="pagination">
        <button class="btn btn-sm" on:click={prevPage} disabled={page <= 1}>
          <ChevronLeft size={14} /> {$t('system.back')}
        </button>
        <span>{$t('pages.admin.pageInfo', { values: { page, totalPages, total } })}</span>
        <button class="btn btn-sm" on:click={nextPage} disabled={page >= totalPages}>
          {$t('system.next')} <ChevronRight size={14} />
        </button>
      </div>
    {/if}
  {/if}
</div>

{#if deactivateTarget}
  <ConfirmModal
    title={$t('pages.admin.deactivateConfirmTitle', { values: { username: deactivateTarget.username } })}
    message={$t('pages.admin.deactivateConfirmMessage')}
    confirmLabel={$t('pages.admin.deactivate')}
    confirmVariant="warning"
    on:confirm={doDeactivate}
    on:cancel={() => deactivateTarget = null}
  />
{/if}

{#if purgeTarget}
  <ConfirmModal
    title={$t('pages.admin.purgeConfirmTitle', { values: { username: purgeTarget.username } })}
    message={$t('pages.admin.purgeConfirmMessage')}
    confirmLabel={$t('pages.admin.deleteForever')}
    on:confirm={doPurge}
    on:cancel={() => purgeTarget = null}
  />
{/if}

<!-- Create User Modal -->
{#if showCreateModal}
  <div class="modal-overlay" on:click={() => showCreateModal = false} role="presentation" on:keydown={(e) => e.key === 'Escape' && (showCreateModal = false)}>
    <div class="modal" on:click|stopPropagation on:keydown|stopPropagation role="dialog" aria-modal="true" aria-label={$t('pages.admin.createUser')} tabindex="-1">
      <div class="modal-header">
        <h3>{$t('pages.admin.createUser')}</h3>
        <button class="btn btn-sm btn-ghost" on:click={() => showCreateModal = false}><X size={16} /></button>
      </div>
      {#if createError}<p class="error">{createError}</p>{/if}
      <form on:submit|preventDefault={handleCreate}>
        <div class="form-group">
          <label for="admin-create-username">{$t('pages.admin.colUsername')}</label>
          <input id="admin-create-username" bind:value={createForm.username} required minlength="3" />
        </div>
        <div class="form-group">
          <label for="admin-create-email">{$t('pages.admin.colEmail')}</label>
          <input id="admin-create-email" type="email" bind:value={createForm.email} required />
        </div>
        <div class="form-group">
          <label for="admin-create-password">{$t('pages.admin.password')}</label>
          <input id="admin-create-password" type="password" bind:value={createForm.password} required minlength="8" />
        </div>
        <div class="form-group">
          <label for="admin-create-role">{$t('pages.admin.colRole')}</label>
          <Select id="admin-create-role" bind:value={createForm.role}
            options={SYSTEM_ROLES.filter(r => r.value !== 'super_admin' || currentUser?.role === 'super_admin').map(r => ({ value: r.value, label: r.label }))} />
        </div>
        <div class="form-group">
          <label class="checkbox-label">
            <input type="checkbox" bind:checked={createForm.can_publish} />
            {$t('pages.admin.grantPublishPermission')}
          </label>
        </div>
        <button class="btn" type="submit" disabled={createLoading}>
          {#if createLoading}<Loader2 size={14} class="animate-spin" /> {$t('pages.admin.creating')}{:else}{$t('pages.admin.createUser')}{/if}
        </button>
      </form>
    </div>
  </div>
{/if}

<!-- Edit User Modal -->
{#if showEditModal}
  <div class="modal-overlay" on:click={() => showEditModal = false} role="presentation" on:keydown={(e) => e.key === 'Escape' && (showEditModal = false)}>
    <div class="modal" on:click|stopPropagation on:keydown|stopPropagation role="dialog" aria-modal="true" aria-label={$t('pages.admin.editUserAria')} tabindex="-1">
      <div class="modal-header">
        <h3>{$t('pages.admin.editUserTitle', { values: { username: editUser.username } })}</h3>
        <button class="btn btn-sm btn-ghost" on:click={() => showEditModal = false}><X size={16} /></button>
      </div>
      {#if editError}<p class="error">{editError}</p>{/if}
      <form on:submit|preventDefault={handleEdit}>
        <div class="form-group">
          <label for="admin-edit-email">{$t('pages.admin.colEmail')}</label>
          <input id="admin-edit-email" type="email" bind:value={editForm.email} required />
        </div>
        <div class="form-group">
          <label for="admin-edit-role">{$t('pages.admin.colRole')}</label>
          <Select id="admin-edit-role" bind:value={editForm.role}
            options={SYSTEM_ROLES.filter(r => r.value !== 'super_admin' || currentUser?.role === 'super_admin').map(r => ({ value: r.value, label: r.label }))} />
        </div>
        <div class="form-group">
          <label class="checkbox-label">
            <input type="checkbox" bind:checked={editForm.can_publish} />
            {$t('pages.admin.publishPermission')}
          </label>
          {#if ['admin','super_admin'].includes(editForm.role)}
            <span class="field-hint">{$t('pages.admin.publishPermissionHint')}</span>
          {/if}
        </div>
        <div class="form-group">
          <label class="checkbox-label">
            <input type="checkbox" bind:checked={editForm.is_active} />
            {$t('pages.admin.statusActive')}
          </label>
        </div>
        <button class="btn" type="submit" disabled={editLoading}>
          {#if editLoading}<Loader2 size={14} class="animate-spin" /> {$t('pages.admin.saving')}{:else}{$t('pages.admin.saveChanges')}{/if}
        </button>
      </form>
    </div>
  </div>
{/if}

<!-- Reset Password Modal -->
{#if showResetModal}
  <div class="modal-overlay" on:click={() => showResetModal = false} role="presentation" on:keydown={(e) => e.key === 'Escape' && (showResetModal = false)}>
    <div class="modal" on:click|stopPropagation on:keydown|stopPropagation role="dialog" aria-modal="true" aria-label={$t('pages.admin.resetPassword')} tabindex="-1">
      <div class="modal-header">
        <h3>{$t('pages.admin.resetPasswordTitle', { values: { username: resetUsername } })}</h3>
        <button class="btn btn-sm btn-ghost" on:click={() => showResetModal = false}><X size={16} /></button>
      </div>
      {#if resetError}<p class="error">{resetError}</p>{/if}
      <form on:submit|preventDefault={handleReset}>
        <input type="text" name="username" autocomplete="username" value={resetUsername} hidden />
        <div class="form-group">
          <label for="admin-reset-password">{$t('pages.admin.newPassword')}</label>
          <input id="admin-reset-password" type="password" bind:value={resetPassword} required minlength="8" />
        </div>
        <button class="btn" type="submit" disabled={resetLoading}>
          {#if resetLoading}<Loader2 size={14} class="animate-spin" /> {$t('pages.admin.resetting')}{:else}{$t('pages.admin.resetPassword')}{/if}
        </button>
      </form>
    </div>
  </div>
{/if}

<style>
  .admin-users { max-width: 1000px; }
  .header-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 1rem;
  }
  h2 {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    margin: 0;
  }
  .search-bar {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.6rem 0.85rem;
    border: 1px solid var(--line-soft);
    border-radius: 12px;
    background: white;
    margin-bottom: 1rem;
  }
  .search-bar input {
    border: none;
    outline: none;
    flex: 1;
    font-size: 0.9rem;
    background: transparent;
  }
  .loading {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 2rem;
    justify-content: center;
    color: var(--ink-500);
  }
  .data-table {
    width: 100%;
    border-collapse: collapse;
    font-size: 0.9rem;
    background: white;
    border-radius: 12px;
    overflow: hidden;
    border: 1px solid var(--line-soft);
  }
  .data-table th, .data-table td {
    padding: 0.65rem 0.85rem;
    text-align: left;
    border-bottom: 1px solid var(--line-soft);
  }
  .data-table th {
    font-weight: 600;
    color: var(--ink-500);
    font-size: 0.78rem;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    background: var(--bg-accent, #f8f9fa);
  }
  tr.inactive { opacity: 0.5; }
  .actions {
    display: flex;
    gap: 0.25rem;
  }
  .role-badge {
    display: inline-block;
    padding: 0.2rem 0.55rem;
    border-radius: 20px;
    font-size: 0.78rem;
    font-weight: 600;
  }
  .badge-super { background: #ede7f6; color: #4527a0; }
  .badge-admin { background: #e3f2fd; color: #1565c0; }
  .badge-user { background: #f5f5f5; color: #616161; }
  .pub-badge {
    display: inline-block;
    padding: 0.15rem 0.45rem;
    border-radius: 20px;
    font-size: 0.72rem;
    font-weight: 600;
    background: #fef3c7;
    color: #92400e;
    margin-left: 0.25rem;
    vertical-align: middle;
  }
  .field-hint {
    font-size: 0.78rem;
    color: var(--ink-400, #9ca3af);
    display: block;
    margin-top: 0.2rem;
  }
  .vis-badge {
    display: inline-block;
    padding: 0.18rem 0.5rem;
    border-radius: 20px;
    font-size: 0.75rem;
    font-weight: 600;
    background: #f5f5f5;
    color: #616161;
  }
  .vis-badge.public {
    background: #d4edda;
    color: #155724;
  }
  .pagination {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 1rem;
    margin-top: 1rem;
    font-size: 0.9rem;
    color: var(--ink-500);
  }
  .btn-danger { color: #c62828; }
  .btn-danger:hover { background: #ffebee; }
  .btn-purge { color: #6a1b9a; }
  .btn-purge:hover { background: #f3e5f5; }

  .modal-overlay {
    position: fixed;
    inset: 0;
    background: rgba(0,0,0,0.4);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 100;
  }
  .modal {
    background: white;
    border-radius: 18px;
    padding: 1.5rem;
    width: 90%;
    max-width: 440px;
    box-shadow: 0 10px 40px rgba(0,0,0,0.2);
  }
  .modal-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 1rem;
  }
  .modal-header h3 { margin: 0; }
  .checkbox-label {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    cursor: pointer;
  }

  @media (max-width: 640px) {
    .data-table { font-size: 0.8rem; }
    .data-table th, .data-table td { padding: 0.5rem; }
  }

  :global(:is([data-theme="dark"], .dark)) .search-bar { background: var(--bg-strong); }
  :global(:is([data-theme="dark"], .dark)) .data-table { background: var(--bg-strong); }
  :global(:is([data-theme="dark"], .dark)) .badge-super { background: rgba(139,92,246,0.2); color: #c4b5fd; }
  :global(:is([data-theme="dark"], .dark)) .badge-admin { background: rgba(59,130,246,0.2); color: #93c5fd; }
  :global(:is([data-theme="dark"], .dark)) .badge-user { background: rgba(255,255,255,0.08); color: var(--ink-400); }
  :global(:is([data-theme="dark"], .dark)) .pub-badge { background: rgba(245,158,11,0.18); color: #fcd34d; }
  :global(:is([data-theme="dark"], .dark)) .vis-badge { background: rgba(255,255,255,0.08); color: var(--ink-400); }
  :global(:is([data-theme="dark"], .dark)) .vis-badge.public { background: rgba(16,185,129,0.18); color: #6ee7b7; }
  :global(:is([data-theme="dark"], .dark)) .btn-danger { color: #fca5a5; }
  :global(:is([data-theme="dark"], .dark)) .btn-danger:hover { background: rgba(239,68,68,0.15); }
  :global(:is([data-theme="dark"], .dark)) .btn-purge { color: #c4b5fd; }
  :global(:is([data-theme="dark"], .dark)) .btn-purge:hover { background: rgba(139,92,246,0.15); }
  :global(:is([data-theme="dark"], .dark)) .modal { background: var(--bg-strong); }
</style>
