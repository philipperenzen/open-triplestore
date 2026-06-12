<script>
  import { onMount } from 'svelte';
  import { login as apiLogin, verify2fa, setTokens, getOauthProviders } from '../lib/api.js';
  import { refreshUser } from '../lib/stores.js';
  import { navigate } from '../lib/router/index.js';
  import { Link } from '../lib/router/index.js';
  import { t } from 'svelte-i18n';
  import { LogIn, Loader2, ShieldCheck, ArrowLeft } from 'lucide-svelte';

  let username = '';
  let password = '';
  let error = '';
  let loading = false;
  let ssoProviders = [];

  // Two-factor step: set when login answered with mfa_required.
  let mfaToken = '';
  let mfaCode = '';
  let mfaInput;

  onMount(async () => {
    try {
      ssoProviders = await getOauthProviders();
    } catch {
      // SSO unavailable — hide buttons silently
    }
  });

  function finishLogin(res) {
    setTokens(res.access_token, res.refresh_token);
    return refreshUser().then(() => navigate('/', { replace: true }));
  }

  async function handleLogin() {
    error = '';
    loading = true;
    try {
      const res = await apiLogin(username, password);
      if (res.mfa_required) {
        // Correct password, but the account asks for a second factor.
        mfaToken = res.mfa_token;
        mfaCode = '';
        setTimeout(() => mfaInput?.focus(), 50);
        return;
      }
      await finishLogin(res);
    } catch (e) {
      error = e.message;
    } finally {
      loading = false;
    }
  }

  async function handleMfaVerify() {
    error = '';
    loading = true;
    try {
      const res = await verify2fa(mfaToken, mfaCode.trim());
      await finishLogin(res);
    } catch (e) {
      error = e.message;
    } finally {
      loading = false;
    }
  }

  function backToPassword() {
    mfaToken = '';
    mfaCode = '';
    error = '';
  }

  function beginSso(slug) {
    // Redirect to server-side OIDC/SAML authorize endpoint
    window.location.href = `/api/auth/oauth/${slug}/authorize`;
  }
</script>

<div class="login-wrap">
<div class="card login-card">
  <div class="login-mark" aria-hidden="true">
    <svg viewBox="0 0 32 32" width="22" height="22" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round">
      <circle cx="9" cy="9" r="2.8"/><circle cx="23" cy="9" r="2.8"/><circle cx="16" cy="23" r="2.8"/>
      <line x1="12" y1="9" x2="20" y2="9"/><line x1="10.5" y1="11.5" x2="14.5" y2="20.5"/><line x1="21.5" y1="11.5" x2="17.5" y2="20.5"/>
    </svg>
  </div>

  {#if mfaToken}
    <h2>{$t('pages.login.mfaHeading')}</h2>
    <p class="login-sub">{$t('pages.login.mfaSubtitle')}</p>
    {#if error}
      <p class="error">{error}</p>
    {/if}
    <form on:submit|preventDefault={handleMfaVerify}>
      <div class="form-group">
        <label for="mfa-code">{$t('pages.login.mfaCode')}</label>
        <input
          id="mfa-code"
          bind:this={mfaInput}
          bind:value={mfaCode}
          required
          autocomplete="one-time-code"
          inputmode="numeric"
          placeholder="123456"
        />
        <p class="field-hint">{$t('pages.login.mfaRecoveryHint')}</p>
      </div>
      <button class="btn" type="submit" disabled={loading || !mfaCode.trim()}>
        {#if loading}<Loader2 size={14} class="animate-spin" /> {$t('pages.login.submitting')}{:else}<ShieldCheck size={14} /> {$t('pages.login.mfaSubmit')}{/if}
      </button>
    </form>
    <p class="register-link">
      <button class="linklike" type="button" on:click={backToPassword}>
        <ArrowLeft size={12} /> {$t('pages.login.mfaBack')}
      </button>
    </p>
  {:else}
    <h2>{$t('pages.login.heading')}</h2>
    <p class="login-sub">{$t('pages.login.subtitle')}</p>
    {#if error}
      <p class="error">{error}</p>
    {/if}
    <form on:submit|preventDefault={handleLogin}>
      <div class="form-group">
        <label for="username">{$t('pages.login.username')}</label>
        <input id="username" bind:value={username} required autocomplete="username" />
      </div>
      <div class="form-group">
        <label for="password">{$t('pages.login.password')}</label>
        <input id="password" type="password" bind:value={password} required autocomplete="current-password" />
        <p class="forgot-link">
          <Link to="/forgot-password">{$t('pages.login.forgotPassword')}</Link>
        </p>
      </div>
      <button class="btn" type="submit" disabled={loading}>
        {#if loading}<Loader2 size={14} class="animate-spin" /> {$t('pages.login.submitting')}{:else}<LogIn size={14} /> {$t('pages.login.submit')}{/if}
      </button>
    </form>

    {#if ssoProviders.length > 0}
      <div class="sso-divider">
        <span>{$t('pages.login.ssoDivider')}</span>
      </div>
      <div class="sso-buttons">
        {#each ssoProviders as provider}
          <button class="btn btn-sso" type="button" on:click={() => beginSso(provider.slug)}>
            {provider.name}
          </button>
        {/each}
      </div>
    {/if}

    <p class="register-link">
      {$t('pages.login.noAccount')} <Link to="/register">{$t('pages.login.register')}</Link>
    </p>
  {/if}
</div>
</div>

<style>
  .login-wrap {
    display: grid;
    place-items: center;
    min-height: 60vh;
    padding: 1rem;
  }
  .login-card {
    width: 100%;
    max-width: 420px;
    padding: 2rem 2rem 1.5rem;
    text-align: left;
  }
  .login-mark {
    display: grid; place-items: center;
    width: 44px; height: 44px;
    border-radius: 12px;
    background: linear-gradient(135deg, var(--brand-300), var(--brand-600));
    color: white;
    box-shadow: 0 8px 18px rgba(47, 122, 140, 0.3);
    margin-bottom: 0.85rem;
  }
  h2 { margin: 0 0 0.25rem; font-size: 1.4rem; }
  .login-sub { margin: 0 0 1.25rem; color: var(--ink-500); font-size: 0.9rem; }
  .login-card .btn { width: 100%; }
  .register-link {
    margin-top: 1rem;
    font-size: 0.9rem;
  }
  .forgot-link {
    margin: 0.35rem 0 0;
    font-size: 0.82rem;
    text-align: right;
  }
  .forgot-link :global(a) { color: var(--brand-600); text-decoration: none; }
  .forgot-link :global(a:hover) { text-decoration: underline; }
  .field-hint {
    margin: 0.35rem 0 0;
    font-size: 0.78rem;
    color: var(--ink-400);
  }
  .linklike {
    display: inline-flex;
    align-items: center;
    gap: 0.3rem;
    background: none;
    border: none;
    padding: 0;
    color: var(--brand-600);
    cursor: pointer;
    font-size: 0.9rem;
  }
  .linklike:hover { text-decoration: underline; }
  .sso-divider {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    margin: 1.25rem 0 0.75rem;
    color: var(--color-muted, #888);
    font-size: 0.85rem;
  }
  .sso-divider::before,
  .sso-divider::after {
    content: '';
    flex: 1;
    height: 1px;
    background: var(--color-border, #ddd);
  }
  .sso-buttons {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
  }
  .btn-sso {
    width: 100%;
    background: var(--color-surface-2, #f5f5f5);
    border: 1px solid var(--color-border, #ddd);
    color: var(--color-text, #333);
  }
  .btn-sso:hover {
    background: var(--color-surface-3, #eee);
  }
</style>
