<script>
  import { onMount } from 'svelte';
  import { t } from 'svelte-i18n';
  import { getMe, updateMe, changePassword, listApiTokens, createApiToken, revokeApiToken, uploadUserAvatar, getUserAvatarUrl, selfDeactivate, selfPurge, logout } from '../lib/api.js';
  import { refreshUser, isAuthenticated, authInitialized } from '../lib/stores.js';
  import { TOKEN_SCOPES } from '../lib/permissions.js';
  import { navigate } from '../lib/router/index.js';
  import { Key, Plus, Trash2, Copy, Loader2, Check, Globe, Lock, Camera, AlertTriangle } from 'lucide-svelte';
  import ConfirmModal from '../components/ConfirmModal.svelte';

  let profile = null;
  let profileError = '';
  let profileSuccess = '';
  let profileLoading = false;

  let privacyLoading = false;
  let privacySuccess = '';

  let editUsername = '';
  let editEmail = '';
  let editDisplayName = '';
  let editBio = '';
  let editWebsite = '';
  let editPhone = '';
  let editOrganization = '';

  let currentPassword = '';
  let newPassword = '';
  let confirmPassword = '';
  let pwError = '';
  let pwSuccess = '';
  let pwLoading = false;

  let avatarInput;
  let uploadingAvatar = false;
  let avatarVersion = 0;

  let tokens = [];
  let tokensLoading = false;
  let tokenName = '';
  let tokenScopes = ['read'];
  let tokenExpiryDays = '';
  let createTokenLoading = false;
  let createdToken = null;
  let copied = false;

  let _guardChecked = false;
  $: if ($authInitialized && !_guardChecked) {
    _guardChecked = true;
    if (!$isAuthenticated) navigate('/login');
  }

  onMount(async () => {
    try {
      profile = await getMe();
      editUsername = profile.username;
      editEmail = profile.email;
      editDisplayName = profile.display_name || '';
      editBio = profile.bio || '';
      editWebsite = profile.website || '';
      editPhone = profile.phone || '';
      editOrganization = profile.organization || '';
    } catch {}
    await loadTokens();
  });

  async function handleAvatarUpload(e) {
    const file = e.target.files?.[0];
    if (!file) return;
    uploadingAvatar = true;
    profileError = '';
    try {
      await uploadUserAvatar(file);
      profile = { ...profile, avatar_key: true };
      avatarVersion++;
    } catch (e) {
      profileError = e.message;
    } finally {
      uploadingAvatar = false;
      if (avatarInput) avatarInput.value = '';
    }
  }

  async function handlePrivacyToggle() {
    privacySuccess = '';
    privacyLoading = true;
    try {
      const updated = await updateMe({ is_public: !profile.is_public });
      profile = updated;
      privacySuccess = $t('pages.settings.privacySettingSaved');
      setTimeout(() => privacySuccess = '', 3000);
    } catch (e) {
      profileError = e.message;
    }
    privacyLoading = false;
  }

  async function loadTokens() {
    tokensLoading = true;
    try {
      tokens = await listApiTokens();
    } catch {}
    tokensLoading = false;
  }

  async function handleProfileUpdate() {
    profileError = '';
    profileSuccess = '';
    profileLoading = true;
    try {
      await updateMe({
        username: editUsername,
        email: editEmail,
        display_name: editDisplayName || null,
        bio: editBio || null,
        website: editWebsite || null,
        phone: editPhone || null,
        organization: editOrganization || null,
      });
      await refreshUser();
      profileSuccess = $t('pages.settings.profileUpdated');
    } catch (e) {
      profileError = e.message;
    }
    profileLoading = false;
  }

  async function handleChangePassword() {
    pwError = '';
    pwSuccess = '';
    if (newPassword !== confirmPassword) {
      pwError = $t('pages.settings.passwordsDoNotMatch');
      return;
    }
    pwLoading = true;
    try {
      await changePassword(currentPassword, newPassword);
      pwSuccess = $t('pages.settings.passwordChanged');
      currentPassword = '';
      newPassword = '';
      confirmPassword = '';
    } catch (e) {
      pwError = e.message;
    }
    pwLoading = false;
  }

  function toggleScope(scope) {
    if (tokenScopes.includes(scope)) {
      tokenScopes = tokenScopes.filter(s => s !== scope);
    } else {
      tokenScopes = [...tokenScopes, scope];
    }
  }

  async function handleCreateToken() {
    createTokenLoading = true;
    createdToken = null;
    try {
      const data = {
        name: tokenName,
        scopes: tokenScopes,
      };
      if (tokenExpiryDays) data.expires_in_days = parseInt(tokenExpiryDays);
      const res = await createApiToken(data);
      createdToken = res.token;
      tokenName = '';
      tokenScopes = ['read'];
      tokenExpiryDays = '';
      await loadTokens();
    } catch (e) {
      alert(e.message);
    }
    createTokenLoading = false;
  }

  let revokeTokenId = null;

  async function doRevokeToken() {
    try {
      await revokeApiToken(revokeTokenId);
      revokeTokenId = null;
      await loadTokens();
    } catch (e) {
      revokeTokenId = null;
      alert(e.message);
    }
  }

  function copyToken() {
    navigator.clipboard.writeText(createdToken);
    copied = true;
    setTimeout(() => copied = false, 2000);
  }

  // ── Danger Zone ──────────────────────────────────────────────────────────────

  let showDeactivateModal = false;
  let showPurgeModal = false;
  let dangerPassword = '';
  let dangerError = '';
  let dangerLoading = false;

  async function handleSelfDeactivate() {
    dangerError = '';
    dangerLoading = true;
    try {
      await selfDeactivate(dangerPassword);
      await logout();
      navigate('/login');
    } catch (e) {
      dangerError = e.message;
    } finally {
      dangerLoading = false;
    }
  }

  async function handleSelfPurge() {
    dangerError = '';
    dangerLoading = true;
    try {
      await selfPurge(dangerPassword);
      await logout();
      navigate('/login');
    } catch (e) {
      dangerError = e.message;
    } finally {
      dangerLoading = false;
    }
  }

  function openDeactivate() {
    dangerPassword = '';
    dangerError = '';
    showDeactivateModal = true;
    showPurgeModal = false;
  }

  function openPurge() {
    dangerPassword = '';
    dangerError = '';
    showPurgeModal = true;
    showDeactivateModal = false;
  }
</script>

<div class="settings-page">
  <div class="card">
    <h2>{$t('pages.settings.profileHeading')}</h2>
    {#if profileError}<p class="error">{profileError}</p>{/if}
    {#if profileSuccess}<p class="success">{profileSuccess}</p>{/if}

    <div class="avatar-row">
      {#if profile?.avatar_key}
        <img
          src="{getUserAvatarUrl(profile.id)}?v={avatarVersion}"
          alt={$t('pages.settings.avatarAlt')}
          class="avatar-preview"
          on:error={e => { /** @type {HTMLElement} */ (e.currentTarget).style.display = 'none'; }}
        />
      {:else}
        <div class="avatar-placeholder">
          {profile?.username?.[0]?.toUpperCase() || '?'}
        </div>
      {/if}
      <label class="btn btn-sm btn-ghost avatar-upload-label">
        {#if uploadingAvatar}<Loader2 size={14} class="animate-spin" /> {$t('pages.settings.uploading')}{:else}<Camera size={14} /> {$t('pages.settings.changeAvatar')}{/if}
        <input bind:this={avatarInput} type="file" accept="image/*" on:change={handleAvatarUpload} style="display:none" />
      </label>
    </div>

    <form on:submit|preventDefault={handleProfileUpdate}>
      <div class="form-group">
        <label for="username">{$t('pages.settings.username')}</label>
        <input id="username" bind:value={editUsername} required />
      </div>
      <div class="form-group">
        <label for="email">{$t('pages.settings.email')}</label>
        <input id="email" type="email" bind:value={editEmail} required />
      </div>
      <div class="form-group">
        <label for="settings-display-name">{$t('pages.settings.displayName')}</label>
        <input id="settings-display-name" bind:value={editDisplayName} placeholder={$t('pages.settings.displayNamePlaceholder')} />
      </div>
      <div class="form-group">
        <label for="settings-bio">{$t('pages.settings.bio')}</label>
        <textarea id="settings-bio" bind:value={editBio} rows="2" placeholder={$t('pages.settings.bioPlaceholder')}></textarea>
      </div>
      <div class="form-group">
        <label for="settings-website">{$t('pages.settings.website')}</label>
        <input id="settings-website" bind:value={editWebsite} placeholder="https://…" />
      </div>
      <div class="form-group">
        <label for="settings-phone">{$t('pages.settings.phone')}</label>
        <input id="settings-phone" bind:value={editPhone} placeholder={$t('pages.settings.phonePlaceholder')} />
      </div>
      <div class="form-group">
        <label for="settings-organization">{$t('pages.settings.organization')}</label>
        <input id="settings-organization" bind:value={editOrganization} placeholder={$t('pages.settings.organizationPlaceholder')} />
      </div>
      <div class="form-group">
        <label for="settings-profile-role">{$t('pages.settings.role')}</label>
        <input id="settings-profile-role" value={profile?.role || ''} disabled />
      </div>
      {#if profile}
        <p class="settings-linked-data-hint">
          {$t('pages.settings.linkedDataHint')} <code>urn:system:user:{profile.id}</code>
          {#if profile.is_public}{$t('pages.settings.linkedDataPublic')}{:else}{$t('pages.settings.linkedDataPrivate')}{/if}
        </p>
      {/if}
      <button class="btn" type="submit" disabled={profileLoading}>
        {#if profileLoading}<Loader2 size={14} class="animate-spin" /> {$t('pages.settings.saving')}{:else}{$t('pages.settings.saveChanges')}{/if}
      </button>
    </form>
  </div>

  <div class="card">
    <h2>{$t('pages.settings.changePasswordHeading')}</h2>
    {#if pwError}<p class="error">{pwError}</p>{/if}
    {#if pwSuccess}<p class="success">{pwSuccess}</p>{/if}
    <form on:submit|preventDefault={handleChangePassword}>
      <input type="text" name="username" autocomplete="username" value={profile?.username ?? ''} hidden />
      <div class="form-group">
        <label for="currentPw">{$t('pages.settings.currentPassword')}</label>
        <input id="currentPw" type="password" autocomplete="current-password" bind:value={currentPassword} required />
      </div>
      <div class="form-group">
        <label for="newPw">{$t('pages.settings.newPassword')}</label>
        <input id="newPw" type="password" autocomplete="new-password" bind:value={newPassword} required minlength="8" />
      </div>
      <div class="form-group">
        <label for="confirmPw">{$t('pages.settings.confirmNewPassword')}</label>
        <input id="confirmPw" type="password" autocomplete="new-password" bind:value={confirmPassword} required />
      </div>
      <button class="btn" type="submit" disabled={pwLoading}>
        {#if pwLoading}<Loader2 size={14} class="animate-spin" /> {$t('pages.settings.changing')}{:else}{$t('pages.settings.changePasswordHeading')}{/if}
      </button>
    </form>
  </div>

  <div class="card">
    <h2>
      {#if profile?.is_public}<Globe size={18} />{:else}<Lock size={18} />{/if}
      {$t('pages.settings.privacyHeading')}
    </h2>
    {#if privacySuccess}<p class="success">{privacySuccess}</p>{/if}
    {#if profile}
      <div class="privacy-row">
        <div>
          <div class="privacy-label">
            {#if profile.is_public}
              <Globe size={16} class="privacy-icon public" /> {$t('pages.settings.publicProfile')}
            {:else}
              <Lock size={16} class="privacy-icon private" /> {$t('pages.settings.privateProfile')}
            {/if}
          </div>
          <p class="hint">
            {#if profile.is_public}
              {$t('pages.settings.publicProfileHint')}
            {:else}
              {$t('pages.settings.privateProfileHint')}
            {/if}
          </p>
        </div>
        <button
          class="btn btn-sm"
          on:click={handlePrivacyToggle}
          disabled={privacyLoading}
          title={profile.is_public ? $t('pages.settings.makeProfilePrivate') : $t('pages.settings.makeProfilePublic')}
        >
          {#if privacyLoading}<Loader2 size={14} class="animate-spin" />{:else if profile.is_public}<Lock size={14} /> {$t('pages.settings.makePrivate')}{:else}<Globe size={14} /> {$t('pages.settings.makePublic')}{/if}
        </button>
      </div>
    {:else}
      <p class="hint">{$t('system.loading')}</p>
    {/if}
  </div>

  <div class="card danger-zone-card">
    <h2><AlertTriangle size={18} /> {$t('pages.settings.dangerZone')}</h2>
    <p class="hint">{$t('pages.settings.dangerZoneHint')}</p>

    {#if profile?.role !== 'super_admin'}
      <div class="danger-actions">
        <div class="danger-row">
          <div>
            <strong>{$t('pages.settings.pauseAccount')}</strong>
            <p class="hint">{$t('pages.settings.pauseAccountHint')}</p>
          </div>
          <button class="btn btn-sm btn-warn" on:click={openDeactivate}>{$t('pages.settings.deactivateAccount')}</button>
        </div>
        <div class="danger-row">
          <div>
            <strong>{$t('pages.settings.deleteAccountPermanently')}</strong>
            <p class="hint">{$t('pages.settings.deleteAccountHint')}</p>
          </div>
          <button class="btn btn-sm btn-danger" on:click={openPurge}>{$t('pages.settings.deleteMyAccount')}</button>
        </div>
      </div>
    {:else}
      <p class="hint">{$t('pages.settings.superAdminCannotSelfDelete')}</p>
    {/if}
  </div>

  <div class="card">
    <h2><Key size={18} /> {$t('pages.settings.apiTokensHeading')}</h2>
    <p class="hint">{$t('pages.settings.apiTokensHint')}</p>

    <div class="scope-docs">
      <p class="scope-docs-title">{$t('pages.settings.tokenScopes')}</p>
      <div class="scope-rows">
        <div class="scope-row">
          <code class="scope-pill scope-read">read</code>
          <div>
            <p>{$t('pages.settings.scopeReadDesc')}</p>
            <p class="scope-note">{$t('pages.settings.scopeReadNote')}</p>
          </div>
        </div>
        <div class="scope-row">
          <code class="scope-pill scope-write">write</code>
          <div>
            <p>{$t('pages.settings.scopeWriteDescPre')} <code>read</code>{$t('pages.settings.scopeWriteDescPost')}</p>
            <p class="scope-note">{$t('pages.settings.scopeWriteNote')}</p>
          </div>
        </div>
        <div class="scope-row">
          <code class="scope-pill scope-admin">admin</code>
          <div>
            <p>{$t('pages.settings.scopeAdminDescPre')} <code>read</code> + <code>write</code>{$t('pages.settings.scopeAdminDescPost')}</p>
            <p class="scope-note">{$t('pages.settings.scopeAdminNotePre')} <strong>admin</strong> {$t('pages.settings.scopeAdminNoteOr')} <strong>super_admin</strong> {$t('pages.settings.scopeAdminNotePost')}</p>
          </div>
        </div>
      </div>
    </div>

    {#if createdToken}
      <div class="token-created">
        <strong>{$t('pages.settings.newTokenCreated')}</strong>
        <div class="token-value">
          <code>{createdToken}</code>
          <button class="btn btn-sm" on:click={copyToken}>
            {#if copied}<Check size={14} /> {$t('system.copied')}{:else}<Copy size={14} /> {$t('system.copy')}{/if}
          </button>
        </div>
      </div>
    {/if}

    <form class="create-token-form" on:submit|preventDefault={handleCreateToken}>
      <div class="form-row">
        <div class="form-group" style="flex:2">
          <label for="tokenName">{$t('pages.settings.name')}</label>
          <input id="tokenName" bind:value={tokenName} placeholder={$t('pages.settings.tokenNamePlaceholder')} required />
        </div>
        <div class="form-group" style="flex:1">
          <label for="tokenExpiry">{$t('pages.settings.expiresInDays')}</label>
          <input id="tokenExpiry" type="number" bind:value={tokenExpiryDays} placeholder={$t('pages.settings.never')} min="1" />
        </div>
      </div>
      <div class="form-group">
        <span class="group-label">{$t('pages.settings.scopes')}</span>
        <div class="scope-chips">
          {#each TOKEN_SCOPES as s}
            <label class="scope-chip" class:active={tokenScopes.includes(s.value)}>
              <input type="checkbox" checked={tokenScopes.includes(s.value)} on:change={() => toggleScope(s.value)} />
              {s.label}
            </label>
          {/each}
        </div>
      </div>
      <button class="btn" type="submit" disabled={createTokenLoading || !tokenName || tokenScopes.length === 0}>
        {#if createTokenLoading}<Loader2 size={14} class="animate-spin" /> {$t('pages.settings.creating')}{:else}<Plus size={14} /> {$t('pages.settings.createToken')}{/if}
      </button>
    </form>

    {#if tokensLoading}
      <p>{$t('pages.settings.loadingTokens')}</p>
    {:else if tokens.length === 0}
      <p class="hint">{$t('pages.settings.noTokensYet')}</p>
    {:else}
      <table class="data-table">
        <thead>
          <tr>
            <th>{$t('pages.settings.name')}</th>
            <th>{$t('pages.settings.prefix')}</th>
            <th>{$t('pages.settings.scopes')}</th>
            <th>{$t('pages.settings.expires')}</th>
            <th>{$t('pages.settings.lastUsed')}</th>
            <th>{$t('pages.settings.status')}</th>
            <th></th>
          </tr>
        </thead>
        <tbody>
          {#each tokens as token}
            <tr class:revoked={token.revoked}>
              <td>{token.name}</td>
              <td><code>{token.token_prefix}</code></td>
              <td>{token.scopes.join(', ')}</td>
              <td>{token.expires_at ? new Date(token.expires_at).toLocaleDateString() : $t('pages.settings.never')}</td>
              <td>{token.last_used_at ? new Date(token.last_used_at).toLocaleDateString() : $t('pages.settings.never')}</td>
              <td>{token.revoked ? $t('pages.settings.revoked') : $t('pages.settings.active')}</td>
              <td>
                {#if !token.revoked}
                  <button class="btn btn-sm btn-danger" on:click={() => revokeTokenId = token.id} title={$t('pages.settings.revoke')}>
                    <Trash2 size={14} />
                  </button>
                {/if}
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    {/if}
  </div>
</div>

{#if revokeTokenId !== null}
  <ConfirmModal
    title={$t('pages.settings.revokeTokenTitle')}
    message={$t('pages.settings.revokeTokenMessage')}
    confirmLabel={$t('pages.settings.revoke')}
    confirmVariant="warning"
    on:confirm={doRevokeToken}
    on:cancel={() => revokeTokenId = null}
  />
{/if}

<!-- Deactivate modal -->
{#if showDeactivateModal}
  <div class="modal-backdrop" on:click={() => showDeactivateModal = false} role="presentation" on:keydown={(e) => e.key === 'Escape' && (showDeactivateModal = false)}>
    <div class="modal-box" on:click|stopPropagation on:keydown|stopPropagation role="dialog" aria-modal="true" aria-label={$t('pages.settings.deactivateAccountAria')} tabindex="-1">
      <div class="modal-header">
        <h3><AlertTriangle size={18} /> {$t('pages.settings.deactivateAccountTitle')}</h3>
      </div>
      <p>{$t('pages.settings.deactivateModalBody1')}</p>
      <p>{$t('pages.settings.deactivateModalBody2')}</p>
      {#if dangerError}<p class="error">{dangerError}</p>{/if}
      <div class="form-group">
        <label for="settings-deactivate-password">{$t('pages.settings.confirmWithPassword')}</label>
        <input type="text" name="username" autocomplete="username" value={profile?.username ?? ''} hidden />
        <input id="settings-deactivate-password" type="password" bind:value={dangerPassword} placeholder={$t('pages.settings.currentPasswordPlaceholder')} autocomplete="current-password" />
      </div>
      <div class="modal-footer">
        <button class="btn btn-sm btn-ghost" on:click={() => showDeactivateModal = false}>{$t('system.cancel')}</button>
        <button class="btn btn-sm btn-warn" on:click={handleSelfDeactivate} disabled={dangerLoading || !dangerPassword}>
          {#if dangerLoading}<Loader2 size={14} class="animate-spin" /> {$t('pages.settings.deactivating')}{:else}{$t('pages.settings.deactivateAccount')}{/if}
        </button>
      </div>
    </div>
  </div>
{/if}

<!-- Purge modal -->
{#if showPurgeModal}
  <div class="modal-backdrop" on:click={() => showPurgeModal = false} role="presentation" on:keydown={(e) => e.key === 'Escape' && (showPurgeModal = false)}>
    <div class="modal-box" on:click|stopPropagation on:keydown|stopPropagation role="dialog" aria-modal="true" aria-label={$t('pages.settings.deleteAccountAria')} tabindex="-1">
      <div class="modal-header">
        <h3><AlertTriangle size={18} /> {$t('pages.settings.deleteAccountTitle')}</h3>
      </div>
      <p><strong>{$t('pages.settings.cannotBeUndone')}</strong> {$t('pages.settings.purgeModalBody')}</p>
      {#if dangerError}<p class="error">{dangerError}</p>{/if}
      <div class="form-group">
        <label for="settings-purge-password">{$t('pages.settings.confirmWithPassword')}</label>
        <input type="text" name="username" autocomplete="username" value={profile?.username ?? ''} hidden />
        <input id="settings-purge-password" type="password" bind:value={dangerPassword} placeholder={$t('pages.settings.currentPasswordPlaceholder')} autocomplete="current-password" />
      </div>
      <div class="modal-footer">
        <button class="btn btn-sm btn-ghost" on:click={() => showPurgeModal = false}>{$t('system.cancel')}</button>
        <button class="btn btn-sm btn-danger" on:click={handleSelfPurge} disabled={dangerLoading || !dangerPassword}>
          {#if dangerLoading}<Loader2 size={14} class="animate-spin" /> {$t('pages.settings.deleting')}{:else}{$t('pages.settings.deleteAccountForever')}{/if}
        </button>
      </div>
    </div>
  </div>
{/if}

<style>
  .settings-page {
    display: flex;
    flex-direction: column;
    gap: 1.5rem;
    max-width: 800px;
  }
  h2 {
    margin-top: 0;
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }
  .avatar-row {
    display: flex;
    align-items: center;
    gap: 1rem;
    margin-bottom: 1.25rem;
  }
  .avatar-preview {
    width: 64px;
    height: 64px;
    border-radius: 50%;
    object-fit: cover;
    border: 2px solid var(--line-soft);
  }
  .avatar-placeholder {
    width: 64px;
    height: 64px;
    border-radius: 50%;
    background: var(--brand-100, #e0f2f1);
    color: var(--brand-600);
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: 1.5rem;
    font-weight: 700;
    border: 2px solid var(--line-soft);
    flex-shrink: 0;
  }
  .avatar-upload-label {
    cursor: pointer;
    display: inline-flex;
    align-items: center;
    gap: 0.4rem;
  }
  .hint {
    color: var(--ink-500);
    font-size: 0.9rem;
  }
  .success {
    color: #2d7d46;
    background: #e8f5e9;
    padding: 0.5rem 0.75rem;
    border-radius: 8px;
    font-size: 0.9rem;
  }
  .token-created {
    background: #e3f2fd;
    border: 1px solid #90caf9;
    padding: 1rem;
    border-radius: 12px;
    margin-bottom: 1rem;
  }
  .token-value {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    margin-top: 0.5rem;
  }
  .token-value code {
    flex: 1;
    padding: 0.5rem;
    background: white;
    border: 1px solid var(--line-soft);
    border-radius: 8px;
    font-size: 0.85rem;
    word-break: break-all;
  }
  .form-row {
    display: flex;
    gap: 1rem;
  }
  .scope-docs {
    margin: 0.75rem 0 1rem;
    border: 1px solid var(--line-soft, #e2e8f0);
    border-radius: 10px;
    overflow: hidden;
    font-size: 0.825rem;
  }
  .scope-docs-title {
    font-size: 0.75rem;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--ink-400, #9ca3af);
    padding: 0.5rem 0.75rem;
    background: var(--bg-soft, #f9fafb);
    border-bottom: 1px solid var(--line-soft, #e2e8f0);
    margin: 0;
  }
  .scope-rows { display: flex; flex-direction: column; }
  .scope-row {
    display: flex;
    align-items: flex-start;
    gap: 0.75rem;
    padding: 0.625rem 0.75rem;
    border-bottom: 1px solid var(--line-soft, #e2e8f0);
  }
  .scope-row:last-child { border-bottom: none; }
  .scope-row > div p { margin: 0 0 0.15rem; color: var(--ink-700, #374151); line-height: 1.5; }
  .scope-note { font-size: 0.775rem; color: var(--ink-400, #9ca3af) !important; }
  .scope-pill {
    font-family: ui-monospace, monospace;
    font-size: 0.75rem;
    font-weight: 700;
    padding: 0.2rem 0.55rem;
    border-radius: 0.375rem;
    white-space: nowrap;
    margin-top: 0.1rem;
  }
  .scope-read { background: #dbeafe; color: #1d4ed8; }
  .scope-write { background: #fef3c7; color: #92400e; }
  .scope-admin { background: #fee2e2; color: #991b1b; }
  .create-token-form {
    margin: 1rem 0;
    padding: 1rem;
    background: var(--bg-accent, #f8f9fa);
    border-radius: 12px;
  }
  .scope-chips {
    display: flex;
    gap: 0.5rem;
  }
  .scope-chip {
    display: flex;
    align-items: center;
    gap: 0.3rem;
    padding: 0.4rem 0.8rem;
    border-radius: 20px;
    border: 1px solid var(--line-soft);
    cursor: pointer;
    font-size: 0.85rem;
    transition: all 0.15s ease;
  }
  .scope-chip.active {
    background: var(--brand-100, #e0f2f1);
    border-color: var(--brand-300, #80cbc4);
  }
  .scope-chip input {
    display: none;
  }
  .data-table {
    width: 100%;
    border-collapse: collapse;
    font-size: 0.9rem;
  }
  .data-table th, .data-table td {
    padding: 0.6rem 0.75rem;
    text-align: left;
    border-bottom: 1px solid var(--line-soft);
  }
  .data-table th {
    font-weight: 600;
    color: var(--ink-500);
    font-size: 0.8rem;
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }
  tr.revoked {
    opacity: 0.5;
  }
  .btn-danger {
    color: #c62828;
    border-color: #ef9a9a;
  }
  .btn-danger:hover {
    background: #ffebee;
  }
  .privacy-row {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 1rem;
  }
  .privacy-label {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    font-weight: 600;
    font-size: 0.95rem;
    margin-bottom: 0.25rem;
  }
  :global(.privacy-icon.public) { color: #2d7d46; }
  :global(.privacy-icon.private) { color: var(--ink-500); }

  /* Danger zone */
  .danger-zone-card { border-color: #ffcdd2; }
  .danger-zone-card h2 { color: #c62828; }
  .danger-actions { display: flex; flex-direction: column; gap: 1rem; margin-top: 0.75rem; }
  .danger-row {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 1rem;
    padding: 0.75rem;
    border: 1px solid var(--line-soft);
    border-radius: 10px;
    background: #fafafa;
  }
  .danger-row > div { flex: 1; }
  .danger-row strong { font-size: 0.9rem; }
  .btn-warn { color: #e65100; border-color: #ffcc80; }
  .btn-warn:hover { background: #fff3e0; }

  /* Danger modals */
  .modal-backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0,0,0,0.35);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 100;
  }
  .modal-box {
    background: white;
    border-radius: 1rem;
    padding: 1.5rem;
    width: min(460px, calc(100vw - 2rem));
    box-shadow: 0 20px 60px rgba(0,0,0,0.18);
  }
  .modal-header {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    margin-bottom: 1rem;
  }
  .modal-header h3 { margin: 0; color: #c62828; }
  .modal-box p { font-size: 0.9rem; color: var(--ink-600); margin: 0 0 0.75rem; }
  .modal-footer { display: flex; justify-content: flex-end; gap: 0.5rem; margin-top: 1rem; }

  @media (max-width: 640px) {
    .form-row { flex-direction: column; }
    .data-table { font-size: 0.8rem; }
    .danger-row { flex-direction: column; }
  }
  .settings-linked-data-hint {
    font-size: 0.78rem; color: var(--ink-400); margin: 0.5rem 0;
  }
  .settings-linked-data-hint code { font-size: 0.75rem; background: var(--bg-accent-soft); padding: 1px 5px; border-radius: 4px; }

  /* ---- Dark mode overrides (scoped rules out-specify global theme.css) ---- */
  :global(:is([data-theme="dark"], .dark)) .success { background: rgba(16,185,129,0.14); color: #6ee7b7; }
  :global(:is([data-theme="dark"], .dark)) .token-created { background: rgba(59,130,246,0.12); border-color: rgba(59,130,246,0.35); }
  :global(:is([data-theme="dark"], .dark)) .token-value code { background: var(--bg-soft); }
  :global(:is([data-theme="dark"], .dark)) .scope-read { background: rgba(59,130,246,0.2); color: #93c5fd; }
  :global(:is([data-theme="dark"], .dark)) .scope-write { background: rgba(245,158,11,0.18); color: #fcd34d; }
  :global(:is([data-theme="dark"], .dark)) .scope-admin { background: rgba(239,68,68,0.18); color: #fca5a5; }
  :global(:is([data-theme="dark"], .dark)) .btn-danger { color: #fca5a5; border-color: rgba(239,68,68,0.4); }
  :global(:is([data-theme="dark"], .dark)) .btn-danger:hover { background: rgba(239,68,68,0.14); }
  :global(:is([data-theme="dark"], .dark) .privacy-icon.public) { color: #6ee7b7; }
  :global(:is([data-theme="dark"], .dark)) .danger-zone-card { border-color: rgba(239,68,68,0.4); }
  :global(:is([data-theme="dark"], .dark)) .danger-zone-card h2,
  :global(:is([data-theme="dark"], .dark)) .modal-header h3 { color: #fca5a5; }
  :global(:is([data-theme="dark"], .dark)) .danger-row { background: var(--bg-soft); }
  :global(:is([data-theme="dark"], .dark)) .btn-warn { color: #fcd34d; border-color: rgba(245,158,11,0.4); }
  :global(:is([data-theme="dark"], .dark)) .btn-warn:hover { background: rgba(245,158,11,0.14); }
  :global(:is([data-theme="dark"], .dark)) .modal-box { background: var(--bg-strong); }
</style>
