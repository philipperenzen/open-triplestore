<script>
  // Sub-navigation pinned above every Studio page. Keeps the workspace's
  // surfaces one click from each other, in the order the work runs: you look
  // at the Overview, author Shapes, pick the Datasets to validate, arrange
  // Pipelines, and read Results.
  //
  // Datasets points at /validation, the validation overview — the same list the
  // Overview's "Datasets to validate" card opens. It used to leave for the
  // global catalogue at /datasets, which is not part of the workspace and
  // carries no way back into it.
  import { LayoutDashboard, FileCode, Workflow, ListChecks, Database } from 'lucide-svelte';
  import { t } from 'svelte-i18n';
  import { Link } from '../lib/router/index.js';
  import { location } from '../lib/locationStore.js';

  const TABS = [
    { to: '/shacl',            labelKey: 'components.shaclStudioNav.tabOverview',  icon: LayoutDashboard, match: (p) => p === '/shacl' },
    { to: '/shacl/shapes',     labelKey: 'components.shaclStudioNav.tabShapes',    icon: FileCode,        match: (p) => p.startsWith('/shacl/shapes') },
    { to: '/validation',       labelKey: 'components.shaclStudioNav.tabDatasets',  icon: Database,        match: (p) => p.startsWith('/validation') },
    { to: '/shacl/pipelines',  labelKey: 'components.shaclStudioNav.tabPipelines', icon: Workflow,        match: (p) => p.startsWith('/shacl/pipelines') },
    // Results no longer claims /validation: it did while nothing on that page
    // rendered this bar, so the claim was invisible — and now that the page
    // does, it would light the wrong tab.
    { to: '/shacl/results',    labelKey: 'components.shaclStudioNav.tabResults',   icon: ListChecks,      match: (p) => p.startsWith('/shacl/results') },
  ];

  $: path = $location.pathname;
</script>

<nav class="studio-nav" aria-label={$t('components.shaclStudioNav.navLabel')}>
  {#each TABS as tab}
    {@const Icon = tab.icon}
    <Link to={tab.to} class="tab" data-active={tab.match(path) ? 'true' : 'false'}>
      <Icon size={14} />
      <span>{$t(tab.labelKey)}</span>
    </Link>
  {/each}
</nav>

<style>
  /* Five tabs no longer fit a narrow main column on one line, so the bar wraps
     rather than overflowing its card. */
  .studio-nav { display: flex; flex-wrap: wrap; gap: 0.25rem; padding: 0.4rem; background: var(--surface, #fff); border: 1px solid var(--line-soft); border-radius: 12px; margin-bottom: 0.85rem; }
  :global(.studio-nav .tab) { display: inline-flex; align-items: center; gap: 0.4rem; padding: 0.45rem 0.85rem; border-radius: 8px; color: #64748b; font-weight: 600; font-size: 0.85rem; text-decoration: none; white-space: nowrap; transition: background 0.12s, color 0.12s; }
  :global(.studio-nav .tab:hover) { background: #f1f5f9; color: #334155; }
  :global(.studio-nav .tab[data-active="true"]) { background: #ecfeff; color: #0e7490; }

  /* Once a row wraps, tabs share it evenly instead of leaving a ragged edge. */
  @media (max-width: 900px) {
    :global(.studio-nav .tab) { flex: 1 1 auto; justify-content: center; padding: 0.45rem 0.6rem; }
  }

  :global(:is([data-theme="dark"], .dark)) .studio-nav { background: var(--bg-strong); }
  :global(:is([data-theme="dark"], .dark) .studio-nav .tab) { color: var(--ink-500); }
  :global(:is([data-theme="dark"], .dark) .studio-nav .tab:hover) { background: rgba(255,255,255,0.06); color: var(--ink-800); }
  :global(:is([data-theme="dark"], .dark) .studio-nav .tab[data-active="true"]) { background: var(--brand-100); color: var(--brand-700); }
</style>
