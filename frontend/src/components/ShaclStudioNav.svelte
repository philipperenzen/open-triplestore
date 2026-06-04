<script>
  // Sub-navigation pinned above every /shacl/* page. Keeps the four Studio
  // surfaces (Overview · Shapes · Pipelines · Results) one click from each
  // other, satisfying the "one consistent SHACL workspace" goal.
  import { LayoutDashboard, FileCode, Workflow, ListChecks } from 'lucide-svelte';
  import { t } from 'svelte-i18n';
  import { Link } from '../lib/router/index.js';
  import { location } from '../lib/locationStore.js';

  const TABS = [
    { to: '/shacl',            labelKey: 'components.shaclStudioNav.tabOverview',  icon: LayoutDashboard, match: (p) => p === '/shacl' },
    { to: '/shacl/shapes',     labelKey: 'components.shaclStudioNav.tabShapes',    icon: FileCode,        match: (p) => p.startsWith('/shacl/shapes') },
    { to: '/shacl/pipelines',  labelKey: 'components.shaclStudioNav.tabPipelines', icon: Workflow,        match: (p) => p.startsWith('/shacl/pipelines') },
    { to: '/shacl/results',    labelKey: 'components.shaclStudioNav.tabResults',   icon: ListChecks,      match: (p) => p.startsWith('/shacl/results') || p.startsWith('/validation') },
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
  .studio-nav { display: flex; gap: 0.25rem; padding: 0.4rem; background: var(--surface, #fff); border: 1px solid var(--line-soft); border-radius: 12px; margin-bottom: 0.85rem; }
  :global(.studio-nav .tab) { display: inline-flex; align-items: center; gap: 0.4rem; padding: 0.45rem 0.85rem; border-radius: 8px; color: #64748b; font-weight: 600; font-size: 0.85rem; text-decoration: none; transition: background 0.12s, color 0.12s; }
  :global(.studio-nav .tab:hover) { background: #f1f5f9; color: #334155; }
  :global(.studio-nav .tab[data-active="true"]) { background: #ecfeff; color: #0e7490; }

  :global(:is([data-theme="dark"], .dark)) .studio-nav { background: var(--bg-strong); }
  :global(:is([data-theme="dark"], .dark) .studio-nav .tab) { color: var(--ink-500); }
  :global(:is([data-theme="dark"], .dark) .studio-nav .tab:hover) { background: rgba(255,255,255,0.06); color: var(--ink-800); }
  :global(:is([data-theme="dark"], .dark) .studio-nav .tab[data-active="true"]) { background: var(--brand-100); color: var(--brand-700); }
</style>
