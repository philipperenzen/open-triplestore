<script>
  import { onMount } from 'svelte';
  import { Link, navigate } from '../lib/router/index.js';
  import { browseStats, listDatasets } from '../lib/api.js';
  import { formatNumber } from '../lib/rdf-utils.js';
  import { authInitialized, isAuthenticated } from '../lib/stores.js';

  import { t } from 'svelte-i18n';
  import { Upload, Rows3, Terminal, ShieldCheck, ArrowRight } from 'lucide-svelte';
  import LinkedDataBackground from '../components/LinkedDataBackground.svelte';

  const WORKFLOWS = [
    { href: '/import', kickerKey: 'pages.home.workflows.importKicker', titleKey: 'pages.home.workflows.importTitle', descKey: 'pages.home.workflows.importDesc', ctaKey: 'pages.home.workflows.importCta', icon: Upload },
    { href: '/browse', kickerKey: 'pages.home.workflows.browseKicker', titleKey: 'pages.home.workflows.browseTitle', descKey: 'pages.home.workflows.browseDesc', ctaKey: 'pages.home.workflows.browseCta', icon: Rows3 },
    { href: '/sparql', kickerKey: 'pages.home.workflows.sparqlKicker', titleKey: 'pages.home.workflows.sparqlTitle', descKey: 'pages.home.workflows.sparqlDesc', ctaKey: 'pages.home.workflows.sparqlCta', icon: Terminal },
    { href: '/validation', kickerKey: 'pages.home.workflows.validateKicker', titleKey: 'pages.home.workflows.validateTitle', descKey: 'pages.home.workflows.validateDesc', ctaKey: 'pages.home.workflows.validateCta', icon: ShieldCheck },
  ];

  const CAPABILITIES = ['SPARQL 1.1/1.2', 'RDF-star', 'GeoSPARQL 1.1', 'SHACL', 'SHACL-AF'];

  let searchQuery = '';
  let stats = null;
  let statsLoading = true;
  let datasets = [];
  let error = '';

  function doSearch() {
    const q = searchQuery.trim();
    if (!q) return;
    if (q.startsWith('http://') || q.startsWith('https://') || q.startsWith('urn:')) {
      navigate(`/resource?iri=${encodeURIComponent(q)}`);
    } else {
      navigate(`/browse?subject=${encodeURIComponent(q)}`);
    }
  }

  async function fetchStats() {
    statsLoading = true;
    try {
      const s = await browseStats();
      stats = s;
    } catch (_) {
      // silently ignore — metrics are non-critical
    } finally {
      statsLoading = false;
    }
  }

  onMount(async () => {
    try {
      const d = await listDatasets();
      datasets = d || [];
    } catch (e) {
      error = e.message;
    }
  });

  // Re-fetch stats once auth is resolved, and again if login state changes.
  // This avoids the race condition where onMount fires before the access
  // token is loaded from the refresh-token cookie.
  let prevAuth = null;
  $: if ($authInitialized && $isAuthenticated !== prevAuth) {
    prevAuth = $isAuthenticated;
    fetchStats();
  }

  $: topDatasets = datasets.slice(0, 5);
  $: recommendedActions = [
    datasets.length === 0 ? { label: $t('pages.home.createFirstDataset'), href: '/datasets', detail: $t('pages.home.createFirstDatasetDetail') } : null,
    { label: $t('pages.home.importSourceData'), href: '/import', detail: $t('pages.home.importSourceDataDetail') },
    { label: $t('pages.home.inspectGraph'), href: '/browse?view=graph', detail: $t('pages.home.inspectGraphDetail') },
  ].filter(Boolean);
</script>

<div class="home">
  <section class="hero-grid">
    <div class="card hero-card">
      <LinkedDataBackground color="126, 214, 208" intensity={0.95} />
      <div class="hero-inner">
        <div class="eyebrow hero-eyebrow">{$t('pages.home.heroEyebrow')}</div>
        <h2>{$t('pages.home.heroTitle')}</h2>
        <p class="hero-copy">{$t('pages.home.heroCopy')}</p>

        <form class="hero-search" on:submit|preventDefault={doSearch}>
          <input
            bind:value={searchQuery}
            placeholder={$t('pages.home.searchPlaceholder')}
            aria-label={$t('pages.home.searchPlaceholder')}
          />
          <button class="btn" type="submit" disabled={!searchQuery.trim()}>{$t('search.open')}</button>
        </form>

        <div class="capability-strip">
          {#each CAPABILITIES as capability}
            <span class="capability-pill">{capability}</span>
          {/each}
        </div>
      </div>
    </div>

    <div class="hero-side">
      <div class="card next-card">
        <div class="eyebrow">{$t('pages.home.suggestedSteps')}</div>
        <div class="next-list">
          {#each recommendedActions as action}
            <Link class="next-item" to={action.href}>
              <strong>{action.label}</strong>
              <span>{action.detail}</span>
            </Link>
          {/each}
        </div>
      </div>
    </div>
  </section>

  {#if error}
    <p class="error">{error}</p>
  {/if}

  <section class="metrics-grid">
    <div class="metric-card card">
      <span class="metric-label">{$t('pages.home.totalTriples')}</span>
      {#if statsLoading}
        <strong class="metric-skeleton"></strong>
      {:else}
        <strong>{formatNumber(stats?.total_triples)}</strong>
      {/if}
      <p>{$t('pages.home.triplesDesc')}</p>
    </div>
    <div class="metric-card card">
      <span class="metric-label">{$t('pages.home.namedGraphs')}</span>
      {#if statsLoading}
        <strong class="metric-skeleton"></strong>
      {:else}
        <strong>{formatNumber(stats?.named_graphs)}</strong>
      {/if}
      <p>{$t('pages.home.graphsDesc')}</p>
    </div>
  </section>

  <section class="dashboard-grid">
    <div class="main-column">
      <div class="card">
        <div class="section-head">
          <div>
            <div class="eyebrow">{$t('pages.home.coreWorkflows')}</div>
            <h3>{$t('pages.home.operateLifecycle')}</h3>
          </div>
          <Link class="section-link" to="/sparql">{$t('pages.home.openQueryWorkspace')}</Link>
        </div>
        <div class="workflow-grid">
          {#each WORKFLOWS as workflow}
            <Link to={workflow.href} class="workflow-card">
              <span class="workflow-kicker">
                <svelte:component this={workflow.icon} size={14} />
                {$t(workflow.kickerKey)}
              </span>
              <strong>{$t(workflow.titleKey)}</strong>
              <p>{$t(workflow.descKey)}</p>
              <span class="workflow-cta">{$t(workflow.ctaKey)} <ArrowRight size={12} /></span>
            </Link>
          {/each}
        </div>
      </div>


    </div>

    <div class="side-column">
      <div class="card">
        <div class="section-head compact-head">
          <div>
            <div class="eyebrow">{$t('pages.home.datasets')}</div>
            <h3>{$t('pages.home.managedSurfaces')}</h3>
          </div>
          <Link class="section-link" to="/datasets">{$t('pages.home.manageDatasets')}</Link>
        </div>

        {#if topDatasets.length > 0}
          <div class="dataset-list">
            {#each topDatasets as ds}
              <Link class="dataset-item" to={`/datasets/${ds.id}`}>
                <div>
                  <strong>{ds.name}</strong>
                  <span>{ds.description || $t('pages.home.noDescription')}</span>
                </div>
                <span class="dataset-badge">{ds.visibility || 'public'}</span>
              </Link>
            {/each}
          </div>
        {:else}
          <div class="empty-block compact-empty">
            <p>{$t('pages.home.noDatasets')}</p>
            <Link class="btn btn-sm btn-ghost" to="/datasets">{$t('pages.home.createDataset')}</Link>
          </div>
        {/if}
      </div>

      <div class="card insight-card">
        <div class="eyebrow">{$t('pages.home.recommendedFlow')}</div>
        <ol class="flow-list">
          <li>{$t('pages.home.flow1')}</li>
          <li>{$t('pages.home.flow2')}</li>
          <li>{$t('pages.home.flow3')}</li>
          <li>{$t('pages.home.flow4')}</li>
        </ol>
      </div>
    </div>
  </section>
</div>

<style>
  .home {
    display: flex;
    flex-direction: column;
    gap: 1rem;
  }

  .hero-grid {
    display: grid;
    grid-template-columns: minmax(0, 1.6fr) minmax(300px, 0.95fr);
    gap: 1rem;
  }

  .hero-card {
    position: relative;
    isolation: isolate;
    padding: 14px;
    background:
      radial-gradient(circle at 85% -10%, rgba(126, 214, 208, 0.45), transparent 40%),
      radial-gradient(circle at -10% 110%, rgba(255, 255, 255, 0.08), transparent 40%),
      linear-gradient(135deg, #0f2a33 0%, #1e5663 55%, #2F7A8C 100%);
    color: white;
    overflow: hidden;
    border: 1px solid rgba(255, 255, 255, 0.08);
  }
  .hero-card h2 { color: white; }

  /* Liquid-glass panel floating over the animated linked-data background. The
     14px frame on .hero-card lets the drifting nodes peek around the panel. */
  .hero-inner {
    position: relative;
    z-index: 1;
    background: rgba(255, 255, 255, 0.07);
    backdrop-filter: blur(12px) saturate(135%);
    -webkit-backdrop-filter: blur(12px) saturate(135%);
    border: 1px solid rgba(255, 255, 255, 0.16);
    border-radius: 16px;
    padding: clamp(1.25rem, 3vw, 2rem);
    box-shadow:
      inset 0 1px 0 rgba(255, 255, 255, 0.12),
      0 10px 30px rgba(0, 0, 0, 0.18);
  }

  .hero-eyebrow {
    color: rgba(255, 255, 255, 0.68);
  }

  .hero-card h2 {
    margin: 0;
    max-width: 16ch;
    font-size: clamp(2rem, 5vw, 3.4rem);
    line-height: 0.95;
    letter-spacing: -0.04em;
  }

  .hero-copy {
    max-width: 46rem;
    margin: 0.75rem 0 1rem;
    color: rgba(255, 255, 255, 0.84);
    font-size: 0.95rem;
    line-height: 1.6;
  }

  .hero-search {
    display: flex;
    gap: 0.75rem;
    align-items: center;
    margin-bottom: 0.85rem;
  }

  .hero-search input {
    background: rgba(255, 255, 255, 0.94);
  }

  .capability-strip {
    display: flex;
    flex-wrap: wrap;
    gap: 0.4rem;
  }

  .capability-pill {
    padding: 0.35rem 0.65rem;
    border-radius: 999px;
    background: rgba(255, 255, 255, 0.12);
    border: 1px solid rgba(255, 255, 255, 0.12);
    color: rgba(255, 255, 255, 0.9);
    font-size: 0.75rem;
    font-weight: 600;
  }

  .hero-side {
    display: flex;
    flex-direction: column;
    gap: 1rem;
  }

  .next-card,
  .insight-card {
    min-height: 0;
  }

  :global(.next-item span),
  .metric-card p,
  :global(.workflow-card p),
  :global(.dataset-item span),
  .empty-block p {
    color: var(--ink-700);
    line-height: 1.55;
  }

  .next-list {
    display: flex;
    flex-direction: column;
    gap: 0.7rem;
  }

  :global(.next-item) {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
    padding: 0.85rem 1rem;
    border-radius: 18px;
    background: rgba(255, 255, 255, 0.72);
    text-decoration: none;
    border: 1px solid var(--line-soft);
    transition: transform 0.16s ease, box-shadow 0.16s ease;
  }

  :global(.next-item:hover),
  :global(.workflow-card:hover),
  :global(.dataset-item:hover) {
    transform: translateY(-2px);
    box-shadow: var(--shadow-sm);
  }

  .metrics-grid {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: 0.8rem;
  }

  .metric-card {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
  }

  .metric-card strong {
    font-size: 2rem;
    line-height: 1;
    letter-spacing: -0.05em;
  }

  .metric-skeleton {
    display: block;
    width: 7rem;
    height: 2rem;
    border-radius: 6px;
    background: linear-gradient(90deg, var(--line-soft) 25%, rgba(0,0,0,0.06) 50%, var(--line-soft) 75%);
    background-size: 200% 100%;
    animation: shimmer 1.4s infinite;
  }

  @keyframes shimmer {
    0% { background-position: 200% 0; }
    100% { background-position: -200% 0; }
  }

  .metric-label {
    color: var(--ink-500);
    font-size: 0.74rem;
    font-weight: 700;
    letter-spacing: 0.14em;
    text-transform: uppercase;
  }

  .dashboard-grid {
    display: grid;
    grid-template-columns: minmax(0, 1.55fr) minmax(300px, 0.95fr);
    gap: 1rem;
  }

  .main-column,
  .side-column {
    display: flex;
    flex-direction: column;
    gap: 1rem;
  }

  .section-head {
    display: flex;
    align-items: end;
    justify-content: space-between;
    gap: 1rem;
    margin-bottom: 1rem;
  }

  .compact-head {
    align-items: center;
  }

  .section-head h3 {
    margin: 0;
    font-size: 1.2rem;
  }

  :global(.section-link) {
    color: var(--brand-600);
    text-decoration: none;
    font-size: 0.88rem;
    font-weight: 600;
  }

  :global(.section-link:hover) {
    text-decoration: underline;
  }

  .workflow-grid {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: 0.85rem;
  }

  :global(.workflow-card) {
    display: flex;
    flex-direction: column;
    gap: 0.45rem;
    padding: 1rem;
    border-radius: 20px;
    background: linear-gradient(180deg, rgba(255, 255, 255, 0.9), rgba(247, 241, 231, 0.9));
    border: 1px solid var(--line-soft);
    text-decoration: none;
    transition: transform 0.16s ease, box-shadow 0.16s ease;
  }

  .workflow-kicker {
    display: flex;
    align-items: center;
    gap: 0.35rem;
    color: var(--brand-600);
    font-size: 0.74rem;
    font-weight: 700;
    letter-spacing: 0.14em;
    text-transform: uppercase;
  }

  .workflow-cta {
    display: flex;
    align-items: center;
    gap: 0.3rem;
    margin-top: auto;
    color: var(--ink-900);
    font-size: 0.84rem;
    font-weight: 700;
  }

  .dataset-list {
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
  }

  :global(.dataset-item) {
    display: flex;
    justify-content: space-between;
    gap: 0.75rem;
    padding: 0.95rem 1rem;
    border-radius: 18px;
    border: 1px solid var(--line-soft);
    background: rgba(255, 255, 255, 0.74);
    text-decoration: none;
    transition: transform 0.16s ease, box-shadow 0.16s ease;
  }

  :global(.dataset-item div) {
    min-width: 0;
  }

  :global(.dataset-item span) {
    display: block;
    margin-top: 0.2rem;
    font-size: 0.86rem;
  }

  .dataset-badge {
    display: inline-flex;
    align-items: center;
    height: fit-content;
    padding: 0.34rem 0.65rem;
    border-radius: 999px;
    background: rgba(31, 152, 151, 0.12);
    color: var(--brand-600);
    font-size: 0.74rem;
    font-weight: 700;
    text-transform: lowercase;
    white-space: nowrap;
  }

  .flow-list {
    margin: 0;
    padding-left: 1.15rem;
    color: var(--ink-700);
    line-height: 1.7;
  }

  .empty-block {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 1rem;
    padding: 1rem;
    border: 1px dashed var(--line-strong);
    border-radius: 18px;
    background: rgba(255, 255, 255, 0.5);
  }

  .compact-empty {
    flex-direction: column;
    align-items: stretch;
  }

  @media (max-width: 1080px) {
    .hero-grid,
    .dashboard-grid,
    .metrics-grid {
      grid-template-columns: 1fr;
    }
  }

  @media (max-width: 720px) {
    .hero-search,
    .section-head,
    .empty-block {
      flex-direction: column;
      align-items: stretch;
    }

    .workflow-grid {
      grid-template-columns: 1fr;
    }

    :global(.dataset-item) {
      flex-direction: column;
    }
  }

  /* ── Dark theme: cards/items that hardcode light backgrounds ──────────────── */
  :global(html.dark .next-item),
  :global(html.dark .dataset-item) {
    background: rgba(255, 255, 255, 0.04);
    border-color: var(--line-soft);
  }
  :global(html.dark .next-item:hover),
  :global(html.dark .dataset-item:hover) {
    background: rgba(255, 255, 255, 0.07);
  }
  :global(html.dark .workflow-card) {
    background: linear-gradient(180deg, rgba(255, 255, 255, 0.05), rgba(255, 255, 255, 0.02));
    border-color: var(--line-soft);
  }
  :global(html.dark .workflow-card:hover) {
    background: linear-gradient(180deg, rgba(255, 255, 255, 0.08), rgba(255, 255, 255, 0.04));
  }
  :global(html.dark) .empty-block {
    background: rgba(255, 255, 255, 0.03);
    border-color: var(--line-strong);
  }
  :global(html.dark) .dataset-badge {
    background: rgba(126, 214, 208, 0.14);
    color: var(--brand-700);
  }
  :global(html.dark) .metric-skeleton {
    background: linear-gradient(90deg, rgba(255,255,255,0.04) 25%, rgba(255,255,255,0.08) 50%, rgba(255,255,255,0.04) 75%);
    background-size: 200% 100%;
  }
</style>
