<script>
  import { onMount } from 'svelte';
  import { verifyEmail } from '../lib/api.js';
  import { refreshUser, isAuthenticated } from '../lib/stores.js';
  import { Link } from '../lib/router/index.js';
  import { t } from 'svelte-i18n';
  import { Loader2, MailCheck, MailX } from 'lucide-svelte';

  // 'pending' | 'ok' | 'error'
  let state = 'pending';
  let error = '';
  let result = null;

  onMount(async () => {
    const token = new URLSearchParams(window.location.search).get('token') || '';
    if (!token) {
      state = 'error';
      error = $t('pages.recovery.missingToken');
      return;
    }
    try {
      result = await verifyEmail(token);
      state = 'ok';
      // Refresh the session profile so the verified badge updates immediately.
      if ($isAuthenticated) await refreshUser();
    } catch (e) {
      state = 'error';
      error = e.message;
    }
  });
</script>

<div class="verify-wrap">
<div class="card verify-card">
  {#if state === 'pending'}
    <div class="verify-notice">
      <Loader2 size={32} class="animate-spin" />
      <h2>{$t('pages.verifyEmail.checking')}</h2>
    </div>
  {:else if state === 'ok'}
    <div class="verify-notice">
      <MailCheck size={36} class="ok-icon" />
      <h2>{$t('pages.verifyEmail.okHeading')}</h2>
      <p class="verify-sub">
        {#if result?.kind === 'change_email'}
          {$t('pages.verifyEmail.okBodyChanged', { values: { email: result?.email || '' } })}
        {:else}
          {$t('pages.verifyEmail.okBody')}
        {/if}
      </p>
      {#if $isAuthenticated}
        <Link to="/" class="btn">{$t('pages.verifyEmail.continue')}</Link>
      {:else}
        <Link to="/login" class="btn">{$t('pages.recovery.backToLogin')}</Link>
      {/if}
    </div>
  {:else}
    <div class="verify-notice">
      <MailX size={36} class="err-icon" />
      <h2>{$t('pages.verifyEmail.errorHeading')}</h2>
      <p class="verify-sub">{error}</p>
      <p class="verify-sub">{$t('pages.verifyEmail.errorHint')}</p>
      <Link to="/login" class="btn">{$t('pages.recovery.backToLogin')}</Link>
    </div>
  {/if}
</div>
</div>

<style>
  .verify-wrap {
    display: grid;
    place-items: center;
    min-height: 60vh;
    padding: 1rem;
  }
  .verify-card {
    width: 100%;
    max-width: 420px;
    padding: 2rem 2rem 1.5rem;
  }
  .verify-notice { text-align: center; padding: 0.5rem 0 0.75rem; }
  .verify-notice h2 { margin: 0.5rem 0 0.25rem; font-size: 1.3rem; }
  .verify-sub { margin: 0.5rem 0 0.75rem; color: var(--ink-500); font-size: 0.9rem; }
  .verify-notice :global(.ok-icon) { color: #2d7d46; }
  .verify-notice :global(.err-icon) { color: #c62828; }
  .verify-notice :global(a.btn) {
    display: inline-flex;
    justify-content: center;
    text-decoration: none;
    margin-top: 0.5rem;
  }
  :global(:is([data-theme="dark"], .dark)) .verify-notice :global(.ok-icon) { color: #6ee7b7; }
  :global(:is([data-theme="dark"], .dark)) .verify-notice :global(.err-icon) { color: #fca5a5; }
</style>
