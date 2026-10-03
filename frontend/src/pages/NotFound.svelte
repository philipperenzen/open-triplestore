<script>
  // The catch-all page: rendered by <Fallback> in App.svelte when no route
  // matches the path, instead of an empty page shell.
  import { Link } from '../lib/router/index.js';
  import { location } from '../lib/locationStore.js';
  import { t } from 'svelte-i18n';
  import { MapPinOff } from 'lucide-svelte';
</script>

<div class="nf-wrap">
  <section class="card nf-card" aria-labelledby="nf-title">
    <MapPinOff size={36} class="nf-icon" />
    <h2 id="nf-title">{$t('pages.notFound.title')}</h2>
    <p class="nf-sub">{$t('pages.notFound.detail')}</p>
    <p class="nf-path"><code>{$location?.pathname ?? ''}</code></p>
    <div class="nf-actions">
      <Link to="/" class="btn">{$t('pages.notFound.home')}</Link>
      <Link to="/docs" class="btn btn-ghost">{$t('pages.notFound.docs')}</Link>
    </div>
  </section>
</div>

<style>
  .nf-wrap {
    display: grid;
    place-items: center;
    min-height: 60vh;
    padding: 1rem;
  }
  .nf-card {
    width: 100%;
    max-width: 440px;
    padding: 2rem 2rem 1.5rem;
    text-align: center;
  }
  .nf-card h2 { margin: 0.5rem 0 0.25rem; font-size: 1.3rem; }
  .nf-sub { margin: 0.5rem 0 0.25rem; color: var(--ink-500); font-size: 0.9rem; }
  .nf-path { margin: 0 0 0.75rem; word-break: break-all; font-size: 0.8rem; color: var(--ink-500); }
  .nf-card :global(.nf-icon) { color: var(--ink-400); }
  .nf-actions { display: flex; gap: 0.5rem; justify-content: center; flex-wrap: wrap; }
  .nf-actions :global(a.btn) { display: inline-flex; justify-content: center; text-decoration: none; }
</style>
