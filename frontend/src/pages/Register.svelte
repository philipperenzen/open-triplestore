<script>
  import { register as apiRegister, setTokens, getAuthFeatures } from '../lib/api.js';
  import { refreshUser } from '../lib/stores.js';
  import { navigate, Link } from '../lib/router/index.js';
  import { validateEmail, validateUsername, validatePassword } from '../lib/validate.ts';
  import { t } from 'svelte-i18n';
  import { UserPlus, Loader2, ArrowRight, ArrowLeft, MailCheck, CheckCircle2 } from 'lucide-svelte';
  import { onMount } from 'svelte';

  // Wizard: 1 = account (username/email), 2 = password, 3 = done/verify notice.
  let step = 1;
  let username = '';
  let email = '';
  let password = '';
  let confirmPassword = '';
  let error = '';
  let loading = false;

  // Set after a successful registration that did NOT auto-login (the server
  // requires a verified email before the first session).
  let verificationRequired = false;
  // Whether the server actually delivers email (vs logging it).
  let emailDelivery = null;

  let usernameTouched = false;
  let emailTouched = false;
  let passwordTouched = false;
  let confirmTouched = false;

  onMount(async () => {
    try {
      const features = await getAuthFeatures();
      emailDelivery = !!features.email_delivery;
      if (features.registration_disabled) {
        error = $t('pages.register.registrationDisabled');
      }
    } catch {
      // Feature probe is cosmetic only.
    }
  });

  $: usernameError = validateUsername(username);
  $: emailError = validateEmail(email);
  $: passwordError = validatePassword(password);
  $: confirmError = password !== confirmPassword ? 'mismatch' : null;
  $: step1Valid = !usernameError && !emailError;
  $: step2Valid = !passwordError && !confirmError;

  function usernameMessage(code) {
    if (code === 'length') return $t('pages.register.usernameLengthError');
    if (code === 'start') return $t('pages.register.usernameStartError');
    return $t('pages.register.usernameCharsetError');
  }
  function emailMessage(code) {
    if (code === 'domain') return $t('pages.register.emailDomainError');
    return $t('pages.register.emailFormatError');
  }

  function goToStep2() {
    usernameTouched = true;
    emailTouched = true;
    if (step1Valid) {
      error = '';
      step = 2;
    }
  }

  async function handleRegister() {
    passwordTouched = true;
    confirmTouched = true;
    if (!step2Valid) return;
    error = '';
    loading = true;
    try {
      const res = await apiRegister(username, email.trim(), password);
      if (res.verification_required) {
        verificationRequired = true;
        step = 3;
        return;
      }
      setTokens(res.access_token, res.refresh_token);
      await refreshUser();
      if (res.user && res.user.email_verified === false) {
        // Signed in, but the address still wants confirming — tell them once.
        step = 3;
        return;
      }
      navigate('/', { replace: true });
    } catch (e) {
      error = e.message;
      // Conflicts (taken username/email) belong to step 1 fields.
      if (/username|email/i.test(e.message)) step = 1;
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

  {#if step < 3}
    <h2>{$t('pages.register.heading')}</h2>
    <p class="register-sub">{$t('pages.register.subtitle')}</p>

    <ol class="steps" aria-label={$t('pages.register.stepsAria')}>
      <li class:active={step === 1} class:done={step > 1}>
        <span class="step-dot">{#if step > 1}<CheckCircle2 size={14} />{:else}1{/if}</span>
        {$t('pages.register.stepAccount')}
      </li>
      <li class:active={step === 2}>
        <span class="step-dot">2</span>
        {$t('pages.register.stepSecurity')}
      </li>
    </ol>

    {#if error}
      <p class="error">{error}</p>
    {/if}
  {/if}

  {#if step === 1}
    <form on:submit|preventDefault={goToStep2}>
      <div class="form-group">
        <label for="username">{$t('pages.register.username')}</label>
        <input
          id="username"
          bind:value={username}
          required
          autocomplete="username"
          aria-invalid={usernameTouched && !!usernameError}
          on:blur={() => usernameTouched = true}
        />
        {#if usernameTouched && usernameError}
          <p class="field-error">{usernameMessage(usernameError)}</p>
        {/if}
      </div>
      <div class="form-group">
        <label for="email">{$t('pages.register.email')}</label>
        <input
          id="email"
          type="email"
          bind:value={email}
          required
          autocomplete="email"
          aria-invalid={emailTouched && !!emailError}
          on:blur={() => emailTouched = true}
        />
        {#if emailTouched && emailError}
          <p class="field-error">{emailMessage(emailError)}</p>
        {/if}
      </div>
      <button class="btn" type="submit">
        {$t('pages.register.next')} <ArrowRight size={16} />
      </button>
    </form>
  {:else if step === 2}
    <form on:submit|preventDefault={handleRegister}>
      <input type="text" name="username" autocomplete="username" value={username} hidden />
      <div class="form-group">
        <label for="password">{$t('pages.register.password')}</label>
        <input
          id="password"
          type="password"
          autocomplete="new-password"
          bind:value={password}
          required
          aria-invalid={passwordTouched && !!passwordError}
          on:blur={() => passwordTouched = true}
        />
        <p class="field-hint" class:field-error={passwordTouched && passwordError}>
          {$t('pages.register.passwordHint')}
        </p>
      </div>
      <div class="form-group">
        <label for="confirmPassword">{$t('pages.register.confirmPassword')}</label>
        <input
          id="confirmPassword"
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
      <div class="step-actions">
        <button class="btn btn-ghost" type="button" on:click={() => { step = 1; error = ''; }}>
          <ArrowLeft size={16} /> {$t('pages.register.back')}
        </button>
        <button class="btn" type="submit" disabled={loading}>
          {#if loading}<Loader2 size={16} class="spin" /> {$t('pages.register.submitting')}{:else}<UserPlus size={16} /> {$t('pages.register.submit')}{/if}
        </button>
      </div>
    </form>
  {:else}
    <div class="verify-notice">
      <MailCheck size={36} class="verify-icon" />
      <h2>{$t('pages.register.verifyHeading')}</h2>
      <p class="register-sub">
        {$t('pages.register.verifyBody', { values: { email } })}
        {#if emailDelivery === false}
          <br /><em>{$t('pages.register.verifyNoSmtpHint')}</em>
        {/if}
      </p>
      {#if verificationRequired}
        <Link to="/login" class="btn">{$t('pages.register.goToLogin')}</Link>
      {:else}
        <button class="btn" on:click={() => navigate('/', { replace: true })}>
          {$t('pages.register.continue')}
        </button>
      {/if}
    </div>
  {/if}

  {#if step < 3}
    <p class="login-link">
      {$t('pages.register.hasAccount')} <Link to="/login">{$t('pages.register.login')}</Link>
    </p>
  {/if}
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

  .steps {
    display: flex;
    gap: 1rem;
    list-style: none;
    padding: 0;
    margin: 0 0 1.25rem;
    font-size: 0.82rem;
    color: var(--ink-400);
  }
  .steps li {
    display: flex;
    align-items: center;
    gap: 0.4rem;
  }
  .steps li.active { color: var(--brand-600); font-weight: 600; }
  .steps li.done { color: var(--ink-500); }
  .step-dot {
    display: grid;
    place-items: center;
    width: 20px;
    height: 20px;
    border-radius: 50%;
    border: 1.5px solid currentColor;
    font-size: 0.7rem;
    font-weight: 700;
  }
  .step-actions {
    display: flex;
    gap: 0.6rem;
  }
  .step-actions .btn { flex: 1; }
  .field-error {
    margin: 0.35rem 0 0;
    font-size: 0.78rem;
    color: #c62828;
  }
  .field-hint {
    margin: 0.35rem 0 0;
    font-size: 0.78rem;
    color: var(--ink-400);
  }
  input[aria-invalid="true"] { border-color: #ef9a9a; }
  .verify-notice {
    text-align: center;
    padding: 0.5rem 0 0.75rem;
  }
  .verify-notice :global(.verify-icon) { color: var(--brand-600); }
  .verify-notice h2 { margin-top: 0.5rem; }
  .verify-notice :global(a.btn) {
    display: inline-flex;
    justify-content: center;
    text-decoration: none;
  }
  :global(:is([data-theme="dark"], .dark)) .field-error { color: #fca5a5; }
  :global(:is([data-theme="dark"], .dark)) input[aria-invalid="true"] { border-color: rgba(239,68,68,0.5); }
</style>
