<script>
  import { onMount, tick } from 'svelte';
  import { t } from 'svelte-i18n';
  import PageHeader from '../components/PageHeader.svelte';
  import { Loader2, BookOpen, ExternalLink, Globe, Boxes } from 'lucide-svelte';
  import SwaggerUIBundle from 'swagger-ui-dist/swagger-ui-bundle.js';
  import 'swagger-ui-dist/swagger-ui.css';

  // A real, bundled OpenAPI viewer (Swagger UI) — no external CDN, works offline.
  // The viewer is scoped by a `?dataset=`/`?organisation=`/`?group=` query param;
  // without one it shows the whole-triplestore API. When scoped, a toggle flips to
  // the whole-triplestore spec. Specs are fetched WITH credentials, so they reflect
  // the public endpoints plus whatever the signed-in user is allowed to see, and
  // "Try it out" calls are credentialed the same way.
  const SERVER = { kind: 'server', labelKey: 'pages.apiDocs.scopeWholeTriplestore', url: '/api-docs/openapi.json' };
  let scope = SERVER;       // the scoped spec target (dataset/org/group) or SERVER
  let active = 'scope';     // 'scope' | 'server' — which spec is displayed

  let spec = null;
  let loading = true;
  let error = '';
  let container;            // Swagger UI mounts into this node

  const specUrlFor = (which) => (which === 'server' ? SERVER.url : scope.url);

  onMount(async () => {
    const p = new URLSearchParams(window.location.search);
    if (p.get('dataset'))
      scope = { kind: 'dataset', labelKey: 'pages.apiDocs.scopeDataset', url: `/api/datasets/${encodeURIComponent(p.get('dataset'))}/openapi.json` };
    else if (p.get('organisation'))
      scope = { kind: 'organisation', labelKey: 'pages.apiDocs.scopeOrganisation', url: `/api/organisations/${encodeURIComponent(p.get('organisation'))}/openapi.json` };
    else if (p.get('group'))
      scope = { kind: 'group', labelKey: 'pages.apiDocs.scopeGroup', url: `/api/groups/${encodeURIComponent(p.get('group'))}/openapi.json` };
    else scope = SERVER;
    active = 'scope';
    await load();
  });

  async function load() {
    loading = true;
    error = '';
    spec = null;
    try {
      const res = await fetch(specUrlFor(active), { credentials: 'include', headers: { Accept: 'application/json' } });
      if (!res.ok) throw new Error((await res.text()) || res.statusText);
      spec = await res.json();
      await tick();
      mount();
    } catch (e) {
      error = e.message || $t('pages.apiDocs.loadFailed');
    } finally {
      loading = false;
    }
  }

  function mount() {
    if (!container || !spec) return;
    container.innerHTML = '';
    SwaggerUIBundle({
      spec,
      domNode: container,
      deepLinking: false,
      layout: 'BaseLayout',
      presets: [SwaggerUIBundle.presets.apis],
      tryItOutEnabled: true,
      persistAuthorization: true,
      defaultModelsExpandDepth: 1,
      docExpansion: 'list',
      // Send cookies on every "Try it out" request so authenticated users can
      // exercise the services they have access to straight from the docs.
      requestInterceptor: (req) => {
        req.credentials = 'include';
        return req;
      },
    });
  }

  async function show(which) {
    if (which === active) return;
    active = which;
    await load();
  }

  $: showToggle = scope.kind !== 'server';
  $: currentUrl = specUrlFor(active);
</script>

<PageHeader title={$t('pages.apiDocs.title')} icon={BookOpen} breadcrumbs={[{ label: $t('pages.apiDocs.breadcrumbApiServices') }, { label: $t('pages.apiDocs.breadcrumbDocs') }]} />

<div class="docs">
  <div class="bar">
    {#if showToggle}
      <div class="seg" role="tablist" aria-label={$t('pages.apiDocs.scopeAriaLabel')}>
        <button class="seg-btn" class:active={active === 'scope'} on:click={() => show('scope')}>
          <Boxes size={14} /> {$t(scope.labelKey)}
        </button>
        <button class="seg-btn" class:active={active === 'server'} on:click={() => show('server')}>
          <Globe size={14} /> {$t('pages.apiDocs.scopeWholeTriplestore')}
        </button>
      </div>
    {:else}
      <span class="scope-label"><Globe size={14} /> {$t('pages.apiDocs.scopeWholeTriplestoreApi')}</span>
    {/if}
    <a class="btn ghost" href={currentUrl} target="_blank" rel="noopener">openapi.json <ExternalLink size={13} /></a>
  </div>

  {#if loading}
    <p class="muted load"><Loader2 class="spin" size={16} /> {$t('pages.apiDocs.loadingDescription')}</p>
  {:else if error}
    <div class="err-box">
      <strong>{$t('pages.apiDocs.couldNotLoad')}</strong>
      <p>{error}</p>
      <a class="btn" href={currentUrl} target="_blank" rel="noopener">{$t('pages.apiDocs.openOpenapiJson')} <ExternalLink size={13} /></a>
    </div>
  {/if}

  <div class="swagger-host" class:hidden={loading || !!error} bind:this={container}></div>
</div>

<style>
  .docs { padding: 1rem 1.25rem 4rem; }
  .muted { color: #64748b; }
  .load { padding: 1rem 0; }
  .bar { display: flex; align-items: center; justify-content: space-between; gap: 1rem; flex-wrap: wrap; margin-bottom: 1rem; }
  .seg { display: inline-flex; border: 1px solid #d0d7de; border-radius: 8px; overflow: hidden; background: #fff; }
  .seg-btn {
    display: inline-flex; align-items: center; gap: .4rem;
    padding: .4rem .8rem; border: 0; background: transparent; cursor: pointer;
    font-size: .82rem; color: #475569; border-right: 1px solid #e2e8f0;
  }
  .seg-btn:last-child { border-right: 0; }
  .seg-btn.active { background: #2563eb; color: #fff; }
  .scope-label { display: inline-flex; align-items: center; gap: .4rem; font-size: .85rem; color: #475569; font-weight: 600; }
  .btn { display: inline-flex; align-items: center; gap: .35rem; padding: .35rem .6rem; border: 1px solid #d0d7de; border-radius: 6px; background: #fff; color: inherit; cursor: pointer; font-size: .82rem; text-decoration: none; }
  .btn.ghost { background: transparent; }
  .err-box { border: 1px solid #f3c9c9; background: #fff8f8; color: #991b1b; border-radius: 8px; padding: 1rem; }
  .err-box p { margin: .4rem 0 .8rem; }

  /* Tame Swagger UI so it sits inside the app shell rather than fighting it. */
  .swagger-host { margin: 0 -0.25rem; }
  .swagger-host.hidden { display: none; }
  :global(.swagger-host .swagger-ui .topbar) { display: none; }
  :global(.swagger-host .swagger-ui .info) { margin: 1rem 0; }
  :global(.swagger-host .swagger-ui .scheme-container) { background: transparent; box-shadow: none; padding: 0 0 1rem; margin: 0; }
  :global(.swagger-host .swagger-ui .wrapper) { padding: 0; max-width: none; }

  :global(.spin) { animation: spin 1s linear infinite; }
  @keyframes spin { to { transform: rotate(360deg); } }

  /* ── Dark theme ──────────────────────────────────────────────────────────── */
  /* Page chrome (Svelte-scoped elements). */
  :global(html.dark) .muted { color: #94a3b8; }
  :global(html.dark) .seg { border-color: rgba(226, 232, 240, 0.16); background: #1e293b; }
  :global(html.dark) .seg-btn { color: #cbd5e1; border-right-color: rgba(226, 232, 240, 0.1); }
  :global(html.dark) .scope-label { color: #cbd5e1; }
  :global(html.dark) .btn { border-color: rgba(226, 232, 240, 0.16); background: rgba(255, 255, 255, 0.04); }
  :global(html.dark) .btn.ghost { background: transparent; }
  :global(html.dark) .err-box { border-color: rgba(248, 113, 113, 0.3); background: rgba(248, 113, 113, 0.1); color: #fca5a5; }

  /* Swagger UI ships its own light stylesheet — repaint its surfaces for dark.
     These selectors are fully global because Swagger injects markup outside the
     Svelte scope; they stay gated behind html.dark and the .swagger-host root. */
  :global(html.dark .swagger-host .swagger-ui) { color: #cbd5e1; }
  :global(html.dark .swagger-host .swagger-ui .info .title),
  :global(html.dark .swagger-host .swagger-ui .info h1),
  :global(html.dark .swagger-host .swagger-ui .info h2),
  :global(html.dark .swagger-host .swagger-ui .opblock-tag),
  :global(html.dark .swagger-host .swagger-ui .opblock .opblock-section-header h4),
  :global(html.dark .swagger-host .swagger-ui .opblock .opblock-section-header label),
  :global(html.dark .swagger-host .swagger-ui .opblock .opblock-summary-path),
  :global(html.dark .swagger-host .swagger-ui .opblock .opblock-summary-path__deprecated),
  :global(html.dark .swagger-host .swagger-ui .opblock .opblock-summary-operation-id),
  :global(html.dark .swagger-host .swagger-ui .parameter__name),
  :global(html.dark .swagger-host .swagger-ui .responses-inner h4),
  :global(html.dark .swagger-host .swagger-ui .responses-inner h5),
  :global(html.dark .swagger-host .swagger-ui .response-col_status),
  :global(html.dark .swagger-host .swagger-ui table thead tr th),
  :global(html.dark .swagger-host .swagger-ui table thead tr td),
  :global(html.dark .swagger-host .swagger-ui .model-title),
  :global(html.dark .swagger-host .swagger-ui section.models h4),
  :global(html.dark .swagger-host .swagger-ui .tab li) { color: #e2e8f0; }
  :global(html.dark .swagger-host .swagger-ui .info li),
  :global(html.dark .swagger-host .swagger-ui .info p),
  :global(html.dark .swagger-host .swagger-ui .info a),
  :global(html.dark .swagger-host .swagger-ui .opblock .opblock-summary-description),
  :global(html.dark .swagger-host .swagger-ui .opblock-description-wrapper p),
  :global(html.dark .swagger-host .swagger-ui .opblock-title_normal p),
  :global(html.dark .swagger-host .swagger-ui .renderedMarkdown p),
  :global(html.dark .swagger-host .swagger-ui .parameter__type),
  :global(html.dark .swagger-host .swagger-ui .parameter__in),
  :global(html.dark .swagger-host .swagger-ui .response-col_links),
  :global(html.dark .swagger-host .swagger-ui .opblock-tag small),
  :global(html.dark .swagger-host .swagger-ui label),
  :global(html.dark .swagger-host .swagger-ui .model) { color: #94a3b8; }
  :global(html.dark .swagger-host .swagger-ui .prop-type) { color: #93c5fd; }
  /* Surfaces */
  :global(html.dark .swagger-host .swagger-ui .opblock) { background: rgba(255, 255, 255, 0.02); box-shadow: none; }
  :global(html.dark .swagger-host .swagger-ui .opblock .opblock-section-header) { background: rgba(255, 255, 255, 0.04); box-shadow: none; }
  :global(html.dark .swagger-host .swagger-ui .opblock .opblock-summary) { border-bottom-color: rgba(255, 255, 255, 0.08); }
  :global(html.dark .swagger-host .swagger-ui .opblock-tag) { border-bottom-color: rgba(255, 255, 255, 0.1); }
  :global(html.dark .swagger-host .swagger-ui .model-box) { background: rgba(255, 255, 255, 0.03); }
  :global(html.dark .swagger-host .swagger-ui section.models) { border-color: rgba(255, 255, 255, 0.14); background: rgba(255, 255, 255, 0.02); }
  :global(html.dark .swagger-host .swagger-ui section.models.is-open h4) { border-bottom-color: rgba(255, 255, 255, 0.14); }
  :global(html.dark .swagger-host .swagger-ui table thead tr th),
  :global(html.dark .swagger-host .swagger-ui table thead tr td) { border-bottom-color: rgba(255, 255, 255, 0.12); }
  /* Form controls + code blocks */
  :global(html.dark .swagger-host .swagger-ui input[type=text]),
  :global(html.dark .swagger-host .swagger-ui input[type=password]),
  :global(html.dark .swagger-host .swagger-ui input[type=email]),
  :global(html.dark .swagger-host .swagger-ui input[type=search]),
  :global(html.dark .swagger-host .swagger-ui textarea),
  :global(html.dark .swagger-host .swagger-ui select) {
    background: #0f172a; color: #e2e8f0; border-color: rgba(255, 255, 255, 0.18);
  }
  :global(html.dark .swagger-host .swagger-ui .btn) { color: #e2e8f0; border-color: rgba(255, 255, 255, 0.22); background: transparent; }
  :global(html.dark .swagger-host .swagger-ui .btn.cancel) { color: #fca5a5; border-color: rgba(248, 113, 113, 0.6); }
  :global(html.dark .swagger-host .swagger-ui .highlight-code),
  :global(html.dark .swagger-host .swagger-ui .highlight-code > .microlight) { background: #0b1220 !important; }
  :global(html.dark .swagger-host .swagger-ui .renderedMarkdown code),
  :global(html.dark .swagger-host .swagger-ui .markdown code) { background: rgba(255, 255, 255, 0.1); color: #fca5a5; }
  /* Expand/collapse + copy arrows are SVGs filled from currentColor. */
  :global(html.dark .swagger-host .swagger-ui .expand-operation svg),
  :global(html.dark .swagger-host .swagger-ui .model-toggle svg),
  :global(html.dark .swagger-host .swagger-ui svg.arrow) { fill: #94a3b8; }
</style>
