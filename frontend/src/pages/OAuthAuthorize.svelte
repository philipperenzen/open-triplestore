<script>
  // The OIDC provider's authorize page (Unified Accounts): client apps
  // (viewer / validation / forms) redirect here with the standard
  // authorization-code + PKCE query parameters. The SPA drives the flow — a
  // signed-out visitor goes through /login first (with ?next= back here), a
  // signed-in one sees the consent card (first time per app), and approval
  // asks the backend to mint the single-use code and bounces back to the app.
  import { onMount } from 'svelte';
  import { t } from 'svelte-i18n';
  import { isAuthenticated, authInitialized, user } from '../lib/stores.js';
  import { navigate } from '../lib/router/index.js';
  import { oauthAuthorize } from '../lib/api.js';
  import { Loader2, ShieldCheck, XCircle } from 'lucide-svelte';

  let params = null;      // validated query params
  let paramError = '';
  let clientName = '';
  let scope = '';
  let requiresConsent = false;
  let busy = true;
  let approveBusy = false;
  let error = '';
  let checked = false;

  function parseParams() {
    const q = new URLSearchParams(window.location.search);
    const p = {
      client_id: q.get('client_id') || '',
      redirect_uri: q.get('redirect_uri') || '',
      scope: q.get('scope') || '',
      state: q.get('state') || undefined,
      nonce: q.get('nonce') || undefined,
      code_challenge: q.get('code_challenge') || undefined,
      code_challenge_method: q.get('code_challenge_method') || undefined,
    };
    if (!p.client_id || !p.redirect_uri) {
      paramError = $t('pages.oauthAuthorize.missingParams');
      return null;
    }
    if ((q.get('response_type') || 'code') !== 'code') {
      paramError = $t('pages.oauthAuthorize.unsupportedResponseType');
      return null;
    }
    return p;
  }

  async function check() {
    busy = true;
    error = '';
    try {
      const res = await oauthAuthorize({ ...params, decision: 'check' });
      clientName = res.client_name;
      scope = res.scope;
      requiresConsent = res.requires_consent;
      checked = true;
      if (!requiresConsent) {
        await approve();
        return;
      }
    } catch (e) {
      error = e.message;
    }
    busy = false;
  }

  async function approve() {
    approveBusy = true;
    error = '';
    try {
      const res = await oauthAuthorize({ ...params, decision: 'approve' });
      window.location.replace(res.redirect_to);
      return;
    } catch (e) {
      error = e.message;
      busy = false;
      approveBusy = false;
    }
  }

  function deny() {
    const sep = params.redirect_uri.includes('?') ? '&' : '?';
    const state = params.state ? `&state=${encodeURIComponent(params.state)}` : '';
    window.location.replace(`${params.redirect_uri}${sep}error=access_denied${state}`);
  }

  onMount(() => {
    params = parseParams();
    if (!params) { busy = false; return; }
    const unsub = authInitialized.subscribe((ready) => {
      if (!ready) return;
      let authed = false;
      isAuthenticated.subscribe((v) => { authed = v; })();
      if (!authed) {
        const here = window.location.pathname + window.location.search;
        navigate(`/login?next=${encodeURIComponent(here)}`, { replace: true });
      } else {
        void check();
      }
    });
    return unsub;
  });
</script>

<div class="authorize-wrap">
  <div class="authorize-card">
    {#if paramError}
      <XCircle size={28} class="err-icon" />
      <h2>{$t('pages.oauthAuthorize.badRequestTitle')}</h2>
      <p class="muted">{paramError}</p>
    {:else if busy && !checked}
      <Loader2 size={24} class="spin" />
      <p class="muted">{$t('common.loading')}</p>
    {:else if error}
      <XCircle size={28} class="err-icon" />
      <h2>{$t('pages.oauthAuthorize.errorTitle')}</h2>
      <p class="muted">{error}</p>
    {:else if requiresConsent}
      <ShieldCheck size={28} />
      <h2>{$t('pages.oauthAuthorize.consentTitle', { values: { client: clientName } })}</h2>
      <p class="muted">
        {$t('pages.oauthAuthorize.consentBody', {
          values: { client: clientName, username: $user?.username ?? '' },
        })}
      </p>
      <p class="scopes"><code>{scope}</code></p>
      <div class="actions">
        <button class="btn" disabled={approveBusy} on:click={() => void approve()}>
          {#if approveBusy}<Loader2 size={14} class="spin" />{/if}
          {$t('pages.oauthAuthorize.approve')}
        </button>
        <button class="btn btn-ghost" disabled={approveBusy} on:click={deny}>
          {$t('pages.oauthAuthorize.deny')}
        </button>
      </div>
    {:else}
      <Loader2 size={24} class="spin" />
      <p class="muted">{$t('pages.oauthAuthorize.redirecting')}</p>
    {/if}
  </div>
</div>

<style>
  .authorize-wrap {
    display: flex;
    justify-content: center;
    padding: 4rem 1rem;
  }
  .authorize-card {
    max-width: 26rem;
    width: 100%;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 0.75rem;
    text-align: center;
    padding: 2rem;
    border: 1px solid var(--border, rgba(128, 128, 128, 0.25));
    border-radius: 12px;
  }
  .actions {
    display: flex;
    gap: 0.75rem;
    margin-top: 0.5rem;
  }
  .scopes {
    font-size: 0.85rem;
    opacity: 0.8;
  }
</style>
