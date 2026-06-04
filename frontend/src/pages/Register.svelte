<script>
  import { register as apiRegister, setTokens } from '../lib/api.js';
  import { refreshUser } from '../lib/stores.js';
  import { navigate, Link } from '../lib/router/index.js';
  import { t } from 'svelte-i18n';
  import { UserPlus, Loader2 } from 'lucide-svelte';

  let username = '';
  let email = '';
  let password = '';
  let confirmPassword = '';
  let error = '';
  let loading = false;

  async function handleRegister() {
    error = '';
    if (password !== confirmPassword) {
      error = $t('pages.register.passwordsMismatch');
      return;
    }
    loading = true;
    try {
      const res = await apiRegister(username, email, password);
      setTokens(res.access_token, res.refresh_token);
      await refreshUser();
      navigate('/', { replace: true });
    } catch (e) {
      error = e.message;
    } finally {
      loading = false;
    }
  }
</script>

<div class="register-wrap">
<div class="card register-card">
  <div class="register-mark" aria-hidden="true">
    <svg viewBox="0 0 32 32" width="22" height="22" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round">
      <circle cx="9" cy="9" r="2.8"/><circle cx="23" cy="9" r="2.8"/><circle cx="16" cy="23" r="2.8"/>
      <line x1="12" y1="9" x2="20" y2="9"/><line x1="10.5" y1="11.5" x2="14.5" y2="20.5"/><line x1="21.5" y1="11.5" x2="17.5" y2="20.5"/>
    </svg>
  </div>
  <h2>{$t('pages.register.heading')}</h2>
  <p class="register-sub">{$t('pages.register.subtitle')}</p>
  {#if error}
    <p class="error">{error}</p>
  {/if}
  <form on:submit|preventDefault={handleRegister}>
    <div class="form-group">
      <label for="username">{$t('pages.register.username')}</label>
      <input id="username" bind:value={username} required />
    </div>
    <div class="form-group">
      <label for="email">{$t('pages.register.email')}</label>
      <input id="email" type="email" bind:value={email} required />
    </div>
    <div class="form-group">
      <label for="password">{$t('pages.register.password')}</label>
      <input id="password" type="password" autocomplete="new-password" bind:value={password} required />
    </div>
    <div class="form-group">
      <label for="confirmPassword">{$t('pages.register.confirmPassword')}</label>
      <input id="confirmPassword" type="password" autocomplete="new-password" bind:value={confirmPassword} required />
    </div>
    <button class="btn" type="submit" disabled={loading}>
      {#if loading}<Loader2 size={16} class="spin" /> {$t('pages.register.submitting')}{:else}<UserPlus size={16} /> {$t('pages.register.submit')}{/if}
    </button>
  </form>
  <p class="login-link">
    {$t('pages.register.hasAccount')} <Link to="/login">{$t('pages.register.login')}</Link>
  </p>
</div>
</div>

<style>
  .register-wrap {
    display: grid;
    place-items: center;
    min-height: 60vh;
    padding: 1rem;
  }
  .register-card {
    width: 100%;
    max-width: 420px;
    padding: 2rem 2rem 1.5rem;
  }
  .register-mark {
    display: grid; place-items: center;
    width: 44px; height: 44px;
    border-radius: 12px;
    background: linear-gradient(135deg, var(--brand-300), var(--brand-600));
    color: white;
    box-shadow: 0 8px 18px rgba(47, 122, 140, 0.3);
    margin-bottom: 0.85rem;
  }
  h2 { margin: 0 0 0.25rem; font-size: 1.4rem; }
  .register-sub { margin: 0 0 1.25rem; color: var(--ink-500); font-size: 0.9rem; }
  .register-card .btn { width: 100%; }
  .login-link { margin-top: 1rem; font-size: 0.88rem; color: var(--ink-500); }
  .login-link :global(a) { color: var(--brand-600); text-decoration: none; font-weight: 600; }
  .login-link :global(a:hover) { text-decoration: underline; }
</style>
