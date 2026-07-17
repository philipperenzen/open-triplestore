<script>
  import { onMount } from 'svelte';
  import { resetPassword } from '../lib/api.js';
  import { Link, navigate } from '../lib/router/index.js';
  import { validatePassword } from '../lib/validate.ts';
  import { t } from 'svelte-i18n';
  import { KeyRound, Loader2, CheckCircle2 } from 'lucide-svelte';

  let token = '';
  let newPassword = '';
  let confirmPassword = '';
  let error = '';
  let loading = false;
  let done = false;

  let passwordTouched = false;
  let confirmTouched = false;

  $: passwordError = validatePassword(newPassword);
  $: confirmError = newPassword !== confirmPassword ? 'mismatch' : null;

  onMount(() => {
    token = new URLSearchParams(window.location.search).get('token') || '';
    if (!token) error = $t('pages.recovery.missingToken');
  });

  async function handleReset() {
    passwordTouched = true;
    confirmTouched = true;
    if (passwordError || confirmError) return;
    error = '';
    loading = true;
    try {
      await resetPassword(token, newPassword);
      done = true;
      setTimeout(() => navigate('/login', { replace: true }), 2500);
    } catch (e) {
      error = e.message;
    } finally {
      loading = false;
    }
  }
</script>

<div class="reset-wrap">
<div class="card reset-card">
  {#if done}
    <div class="done-notice">
      <CheckCircle2 size={36} class="done-icon" />
      <h2>{$t('pages.recovery.resetDoneHeading')}</h2>
      <p class="reset-sub">{$t('pages.recovery.resetDoneBody')}</p>
      <Link to="/login" class="btn">{$t('pages.recovery.backToLogin')}</Link>
    </div>
  {:else}
    <h2>{$t('pages.recovery.resetHeading')}</h2>
    <p class="reset-sub">{$t('pages.recovery.resetSubtitle')}</p>
    {#if error}
      <p class="error">{error}</p>
    {/if}
    <form on:submit|preventDefault={handleReset}>
      <div class="form-group">
        <label for="new-password">{$t('pages.recovery.newPassword')}</label>
        <input
          id="new-password"
          type="password"
          autocomplete="new-password"
          bind:value={newPassword}
          required
          aria-invalid={passwordTouched && !!passwordError}
          on:blur={() => passwordTouched = true}
        />
        <p class="field-hint" class:field-error={passwordTouched && passwordError}>
          {$t('pages.register.passwordHint')}
        </p>
      </div>
      <div class="form-group">
        <label for="confirm-password">{$t('pages.recovery.confirmNewPassword')}</label>
        <input
          id="confirm-password"
          type="password"
          autocomplete="new-password"
          bind:value={confirmPassword}
          required
          aria-invalid={confirmTouched && !!confirmError}
          on:blur={() => confirmTouched = true}
        />
        {#if confirmTouched && confirmError}
          <p class="field-error">{$t('pages.register.passwordsMismatch')}</p>
        {/if}
      </div>
      <button class="btn" type="submit" disabled={loading || !token}>
        {#if loading}<Loader2 size={14} class="animate-spin" /> {$t('pages.recovery.resetting')}{:else}<KeyRound size={14} /> {$t('pages.recovery.resetSubmit')}{/if}
      </button>
    </form>
    <p class="back-link">
      <Link to="/login">{$t('pages.recovery.backToLogin')}</Link>
    </p>
  {/if}
</div>
</div>

<style>
  .reset-wrap {
    display: grid;
    place-items: center;
    min-height: 60vh;
    padding: 1rem;
  }
  .reset-card {
    width: 100%;
    max-width: 420px;
    padding: 2rem 2rem 1.5rem;
  }
  h2 { margin: 0 0 0.25rem; font-size: 1.4rem; }
  .reset-sub { margin: 0 0 1.25rem; color: var(--ink-500); font-size: 0.9rem; }
  .reset-card .btn { width: 100%; }
  .field-hint { margin: 0.35rem 0 0; font-size: 0.78rem; color: var(--ink-400); }
  .field-error { margin: 0.35rem 0 0; font-size: 0.78rem; color: #c62828; }
  input[aria-invalid="true"] { border-color: #ef9a9a; }
  .back-link { margin-top: 1rem; font-size: 0.88rem; }
  .back-link :global(a) { color: var(--brand-600); text-decoration: none; }
  .back-link :global(a:hover) { text-decoration: underline; }
  .done-notice { text-align: center; padding: 0.5rem 0 0.75rem; }
  .done-notice :global(.done-icon) { color: #2d7d46; }
  .done-notice h2 { margin-top: 0.5rem; }
  .done-notice :global(a.btn) {
    display: inline-flex;
    justify-content: center;
    text-decoration: none;
  }
  :global(:is([data-theme="dark"], .dark)) .field-error { color: #fca5a5; }
  :global(:is([data-theme="dark"], .dark)) input[aria-invalid="true"] { border-color: rgba(239,68,68,0.5); }
  :global(:is([data-theme="dark"], .dark)) .done-notice :global(.done-icon) { color: #6ee7b7; }
</style>
