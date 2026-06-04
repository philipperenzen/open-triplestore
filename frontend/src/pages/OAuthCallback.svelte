<script>
  import { onMount } from 'svelte';
  import { t } from 'svelte-i18n';
  import { navigate } from '../lib/router/index.js';
  import { setTokens } from '../lib/api.js';
  import { refreshUser } from '../lib/stores.js';
  import { Loader2 } from 'lucide-svelte';

  let error = '';
  let status = $t('pages.oAuthCallback.processing');

  onMount(async () => {
    // The OIDC callback is handled server-side at /api/auth/oauth/:slug/callback.
    // After the server exchanges the code it redirects back here with the tokens
    // encoded as URL hash parameters (fragment) so they are never sent to the server.
    //
    // Alternatively, if the server responds with JSON directly (no redirect), the
    // tokens arrive in the URL search params when the server redirects to this page.
    try {
      // M-3: tokens arrive only in the URL hash (fragment) — never in search params.
      // The server redirects here with #access_token=...&refresh_token=... so the
      // values are never sent to any server in the Referer header.
      const hash   = new URLSearchParams(window.location.hash.replace(/^#/, ''));
      const params = new URLSearchParams(window.location.search);

      const access  = hash.get('access_token');
      const refresh = hash.get('refresh_token');
      const errMsg  = params.get('error') || hash.get('error');

      // Remove tokens from the URL bar immediately so they are not visible or
      // bookmarkable, and cannot be leaked via Referer if the user clicks a link.
      history.replaceState(null, '', location.pathname);

      if (errMsg) {
        const desc = params.get('error_description') || hash.get('error_description') || '';
        error = desc ? `${errMsg}: ${desc}` : errMsg;
        return;
      }

      if (access) {
        setTokens(access, refresh || null);
        await refreshUser();
        navigate('/', { replace: true });
        return;
      }

      // No tokens in fragment — something went wrong server-side.
      error = $t('pages.oAuthCallback.noTokens');
    } catch (e) {
      error = $t('pages.oAuthCallback.loginError', { values: { message: e.message } });
    }
  });
</script>

<div class="callback-shell">
  {#if error}
    <div class="card callback-card error-card">
      <h2>{$t('pages.oAuthCallback.loginFailed')}</h2>
      <p class="error">{error}</p>
      <a href="/login" class="btn">{$t('pages.oAuthCallback.backToLogin')}</a>
    </div>
  {:else}
    <div class="card callback-card">
      <Loader2 size={32} class="animate-spin" />
      <p>{status}</p>
    </div>
  {/if}
</div>

<style>
  .callback-shell {
    display: flex;
    justify-content: center;
    align-items: center;
    min-height: 60vh;
  }
  .callback-card {
    text-align: center;
    padding: 2rem 3rem;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 1rem;
  }
  .error-card h2 { margin-top: 0; color: var(--color-error, #c0392b); }
</style>
