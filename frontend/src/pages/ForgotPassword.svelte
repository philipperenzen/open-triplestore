<script>
  import { forgotPassword, forgotUsername } from '../lib/api.js';
  import { Link } from '../lib/router/index.js';
  import { validateEmail } from '../lib/validate.ts';
  import { t } from 'svelte-i18n';
  import { KeyRound, UserSearch, Loader2, MailCheck } from 'lucide-svelte';

  // 'password' | 'username'
  let mode = 'password';
  let identifier = '';
  let email = '';
  let error = '';
  let sent = false;
  let loading = false;

  async function handleSubmit() {
    error = '';
    if (mode === 'username' && validateEmail(email)) {
      error = $t('pages.recovery.emailFormatError');
      return;
    }
    loading = true;
    try {
      if (mode === 'password') {
        await forgotPassword(identifier.trim());
      } else {
        await forgotUsername(email.trim());
      }
      sent = true;
    } catch (e) {
      error = e.message;
    } finally {
      loading = false;
    }
  }

  function switchMode(m) {
    mode = m;
    error = '';
    sent = false;
  }
</script>

<div class="recovery-wrap">
<div class="card recovery-card">
  {#if sent}
    <div class="sent-notice">
      <MailCheck size={36} class="sent-icon" />
      <h2>{$t('pages.recovery.sentHeading')}</h2>
      <p class="recovery-sub">
        {mode === 'password' ? $t('pages.recovery.sentBodyPassword') : $t('pages.recovery.sentBodyUsername')}
      </p>
      <Link to="/login" class="btn">{$t('pages.recovery.backToLogin')}</Link>
    </div>
  {:else}
    <h2>{$t('pages.recovery.heading')}</h2>
    <p class="recovery-sub">{$t('pages.recovery.subtitle')}</p>

    <div class="mode-tabs" role="tablist" aria-label={$t('pages.recovery.heading')}>
      <button
        class="mode-tab"
        class:active={mode === 'password'}
        role="tab"
        aria-selected={mode === 'password'}
        on:click={() => switchMode('password')}
      >
        <KeyRound size={14} /> {$t('pages.recovery.tabPassword')}
      </button>
      <button
        class="mode-tab"
        class:active={mode === 'username'}
        role="tab"
        aria-selected={mode === 'username'}
        on:click={() => switchMode('username')}
      >
        <UserSearch size={14} /> {$t('pages.recovery.tabUsername')}
      </button>
    </div>

    {#if error}
      <p class="error">{error}</p>
    {/if}

    <form on:submit|preventDefault={handleSubmit}>
      {#if mode === 'password'}
        <div class="form-group">
          <label for="identifier">{$t('pages.recovery.identifier')}</label>
          <input
            id="identifier"
            bind:value={identifier}
            required
            autocomplete="username"
            placeholder={$t('pages.recovery.identifierPlaceholder')}
          />
          <p class="field-hint">{$t('pages.recovery.passwordHint')}</p>
        </div>
      {:else}
        <div class="form-group">
          <label for="recovery-email">{$t('pages.recovery.email')}</label>
          <input id="recovery-email" type="email" bind:value={email} required autocomplete="email" />
          <p class="field-hint">{$t('pages.recovery.usernameHint')}</p>
        </div>
      {/if}
      <button class="btn" type="submit" disabled={loading || (mode === 'password' ? !identifier.trim() : !email.trim())}>
        {#if loading}<Loader2 size={14} class="animate-spin" /> {$t('pages.recovery.sending')}{:else}{$t('pages.recovery.send')}{/if}
      </button>
    </form>

    <p class="back-link">
      <Link to="/login">{$t('pages.recovery.backToLogin')}</Link>
    </p>
  {/if}
</div>
</div>

<style>
  .recovery-wrap {
    display: grid;
    place-items: center;
    min-height: 60vh;
    padding: 1rem;
  }
  .recovery-card {
    width: 100%;
    max-width: 420px;
    padding: 2rem 2rem 1.5rem;
  }
  h2 { margin: 0 0 0.25rem; font-size: 1.4rem; }
  .recovery-sub { margin: 0 0 1.25rem; color: var(--ink-500); font-size: 0.9rem; }
  .recovery-card .btn { width: 100%; }
  .mode-tabs {
    display: flex;
    gap: 0.4rem;
    margin-bottom: 1.1rem;
  }
  .mode-tab {
    flex: 1;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 0.35rem;
    padding: 0.45rem 0.6rem;
    border: 1px solid var(--line-soft, #e2e8f0);
    border-radius: 10px;
    background: none;
    color: var(--ink-500);
    font-size: 0.85rem;
    cursor: pointer;
  }
  .mode-tab.active {
    background: var(--brand-100, #e0f2f1);
    border-color: var(--brand-300, #80cbc4);
    color: var(--brand-600);
    font-weight: 600;
  }
  .field-hint {
    margin: 0.35rem 0 0;
    font-size: 0.78rem;
    color: var(--ink-400);
  }
  .back-link { margin-top: 1rem; font-size: 0.88rem; }
  .back-link :global(a) { color: var(--brand-600); text-decoration: none; }
  .back-link :global(a:hover) { text-decoration: underline; }
  .sent-notice { text-align: center; padding: 0.5rem 0 0.75rem; }
  .sent-notice :global(.sent-icon) { color: var(--brand-600); }
  .sent-notice h2 { margin-top: 0.5rem; }
  .sent-notice :global(a.btn) {
    display: inline-flex;
    justify-content: center;
    text-decoration: none;
  }
  :global(:is([data-theme="dark"], .dark)) .mode-tab.active { background: rgba(45,212,191,0.12); }
</style>
