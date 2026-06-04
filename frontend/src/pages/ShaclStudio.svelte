<script>
  // SHACL Studio — Overview / Dashboard. A light first-cut: counts + recent
  // activity + nudges. Phase 4 will turn this into a real conformance
  // dashboard with trends and per-shape failure breakdowns.
  import { onMount } from 'svelte';
  import { t } from 'svelte-i18n';
  import { listShapeGraphs, listPipelines, listDatasets } from '../lib/api.js';
  import { FileCode, Workflow, Database, ShieldCheck, Plus, ArrowRight, AlertTriangle } from 'lucide-svelte';
  import { Link, navigate } from '../lib/router/index.js';
  import ShaclStudioNav from '../components/ShaclStudioNav.svelte';
  import { isAuthenticated, authInitialized } from '../lib/stores.js';

  let sets = [];
  let pipelines = [];
  let datasets = [];
  let loading = true;

  let _guardChecked = false;
  $: if ($authInitialized && !_guardChecked) {
    _guardChecked = true;
    if (!$isAuthenticated) navigate('/login');
  }

  onMount(async () => {
    try {
      [sets, pipelines, datasets] = await Promise.all([
        listShapeGraphs().catch(() => []),
        listPipelines().catch(() => []),
        listDatasets().catch(() => []),
      ]);
    } finally {
      loading = false;
    }
  });

  $: scheduled = pipelines.filter((p) => p.schedule_cron && p.schedule_cron.length > 0).length;
  $: gating = pipelines.filter((p) => p.gate_writes).length;
  $: datasetsWithoutShapes = datasets.filter((d) => !d.shapes_graph_iri).length;
  $: recent = [...pipelines].filter((p) => p.last_run_at).sort((a, b) => (b.last_run_at || '').localeCompare(a.last_run_at || '')).slice(0, 6);

  function relativeTime(iso) {
    if (!iso) return '';
    const sec = Math.round((Date.now() - new Date(iso).getTime()) / 1000);
    if (sec < 60) return $t('pages.shaclStudio.timeJustNow');
    const min = Math.round(sec / 60); if (min < 60) return $t('pages.shaclStudio.timeMinutesAgo', { values: { count: min } });
    const hr = Math.round(min / 60); if (hr < 24) return $t('pages.shaclStudio.timeHoursAgo', { values: { count: hr } });
    const day = Math.round(hr / 24); if (day < 30) return $t('pages.shaclStudio.timeDaysAgo', { values: { count: day } });
    return $t('pages.shaclStudio.timeMonthsAgo', { values: { count: Math.round(day / 30) } });
  }
</script>

<div class="studio-page">
  <ShaclStudioNav />

  <p class="lede">{$t('pages.shaclStudio.lede')}</p>

  <div class="kpi-grid">
    <Link to="/shacl/shapes" class="kpi">
      <div class="kpi-icon kpi-icon-shapes"><FileCode size={20} /></div>
      <div class="kpi-body">
        <div class="kpi-value">{sets.length}</div>
        <div class="kpi-label">{$t('pages.shaclStudio.kpiShapeGraphs')}</div>
      </div>
    </Link>
    <Link to="/shacl/pipelines" class="kpi">
      <div class="kpi-icon kpi-icon-pipe"><Workflow size={20} /></div>
      <div class="kpi-body">
        <div class="kpi-value">{pipelines.length}</div>
        <div class="kpi-label">{$t('pages.shaclStudio.kpiPipelines')}</div>
        {#if scheduled || gating}
          <div class="kpi-sub">{scheduled ? $t('pages.shaclStudio.kpiScheduled', { values: { count: scheduled } }) : ''}{scheduled && gating ? ' · ' : ''}{gating ? $t('pages.shaclStudio.kpiGatingWrites', { values: { count: gating } }) : ''}</div>
        {/if}
      </div>
    </Link>
    <Link to="/datasets" class="kpi">
      <div class="kpi-icon kpi-icon-data"><Database size={20} /></div>
      <div class="kpi-body">
        <div class="kpi-value">{datasets.length}</div>
        <div class="kpi-label">{$t('pages.shaclStudio.kpiDatasets')}</div>
        {#if datasetsWithoutShapes > 0}
          <div class="kpi-sub warn"><AlertTriangle size={11} /> {$t('pages.shaclStudio.kpiWithoutShapes', { values: { count: datasetsWithoutShapes } })}</div>
        {/if}
      </div>
    </Link>
    <Link to="/shacl/results" class="kpi">
      <div class="kpi-icon kpi-icon-runs"><ShieldCheck size={20} /></div>
      <div class="kpi-body">
        <div class="kpi-value">{recent.length}</div>
        <div class="kpi-label">{$t('pages.shaclStudio.kpiRecentRuns')}</div>
      </div>
    </Link>
  </div>

  <div class="row">
    <section class="card panel">
      <header class="panel-head">
        <h3>{$t('pages.shaclStudio.recentRunsHeading')}</h3>
        <Link to="/shacl/results" class="see-all">{$t('pages.shaclStudio.allResults')} <ArrowRight size={12} /></Link>
      </header>
      {#if loading}
        <p class="dim">{$t('system.loading')}</p>
      {:else if recent.length === 0}
        <div class="empty">
          <p>{$t('pages.shaclStudio.noPipelineRuns')}</p>
          <Link to="/shacl/pipelines" class="btn btn-sm"><Plus size={13} /> {$t('pages.shaclStudio.createFirstPipeline')}</Link>
        </div>
      {:else}
        <ul class="recent">
          {#each recent as p}
            <li class="recent-row">
              <Link to={`/shacl/pipelines/${p.id}`} class="recent-link">
                <span class="recent-name">{p.name}</span>
                {#if p.last_conforms === false}
                  <span class="pill pill-fail">{$t('pages.shaclStudio.pillFailed')}</span>
                {:else if p.last_conforms === true}
                  <span class="pill pill-ok">{$t('pages.shaclStudio.pillPassed')}</span>
                {/if}
                <span class="recent-time">{relativeTime(p.last_run_at)}</span>
              </Link>
            </li>
          {/each}
        </ul>
      {/if}
    </section>

    <section class="card panel">
      <header class="panel-head"><h3>{$t('pages.shaclStudio.quickStartHeading')}</h3></header>
      <ol class="steps">
        <li><strong>{$t('pages.shaclStudio.step1Title')}</strong> {$t('pages.shaclStudio.step1Body1')} <strong>{$t('pages.shaclStudio.step1Shapes')}</strong> {$t('pages.shaclStudio.step1Body2')}<Link to="/shacl/shapes" class="step-link">{$t('pages.shaclStudio.step1Link')}</Link></li>
        <li><strong>{$t('pages.shaclStudio.step2Title')}</strong> {$t('pages.shaclStudio.step2Body1')} <em>{$t('pages.shaclStudio.step2Emphasis')}</em> {$t('pages.shaclStudio.step2Body2')}</li>
        <li><strong>{$t('pages.shaclStudio.step3Title')}</strong> {$t('pages.shaclStudio.step3Body')}<Link to="/shacl/pipelines" class="step-link">{$t('pages.shaclStudio.step3Link')}</Link></li>
        <li><strong>{$t('pages.shaclStudio.step4Title')}</strong> {$t('pages.shaclStudio.step4Body1')} <em>{$t('pages.shaclStudio.step4Emphasis')}</em> {$t('pages.shaclStudio.step4Body2')}</li>
        <li><strong>{$t('pages.shaclStudio.step5Title')}</strong> {$t('pages.shaclStudio.step5Body1')} <code>form-manifest</code> {$t('pages.shaclStudio.step5Body2')}</li>
      </ol>
    </section>
  </div>
</div>

<style>
  .studio-page { display: flex; flex-direction: column; }
  .lede { margin: 0 0 1rem; color: #475569; font-size: 0.92rem; max-width: 60rem; }
  .kpi-grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(220px, 1fr)); gap: 0.85rem; margin-bottom: 1rem; }
  :global(.studio-page .kpi) { display: flex; align-items: center; gap: 0.85rem; padding: 0.85rem 1rem; border-radius: 14px; background: #fff; border: 1px solid var(--line-soft); color: inherit; text-decoration: none; transition: border-color 0.12s, box-shadow 0.12s; }
  :global(.studio-page .kpi:hover) { border-color: #7ED6D0; box-shadow: var(--shadow-sm); }
  .kpi-icon { display: grid; place-items: center; width: 42px; height: 42px; border-radius: 12px; background: #f1f5f9; color: #475569; flex-shrink: 0; }
  .kpi-icon-shapes { background: linear-gradient(135deg, #ecfeff, #cffafe); color: #0e7490; }
  .kpi-icon-pipe { background: linear-gradient(135deg, #ede9fe, #ddd6fe); color: #6d28d9; }
  .kpi-icon-data { background: linear-gradient(135deg, #fef3c7, #fde68a); color: #92400e; }
  .kpi-icon-runs { background: linear-gradient(135deg, #d1fae5, #a7f3d0); color: #047857; }
  .kpi-value { font-size: 1.5rem; font-weight: 700; color: #1e293b; line-height: 1; }
  .kpi-label { font-size: 0.78rem; color: #64748b; margin-top: 0.2rem; }
  .kpi-sub { font-size: 0.72rem; color: #94a3b8; margin-top: 0.2rem; display: inline-flex; align-items: center; gap: 0.25rem; }
  .kpi-sub.warn { color: #b45309; }

  .row { display: grid; grid-template-columns: 1.4fr 1fr; gap: 0.85rem; align-items: start; }
  .panel { padding: 0.85rem 1rem !important; }
  .panel-head { display: flex; justify-content: space-between; align-items: center; margin-bottom: 0.5rem; }
  .panel-head h3 { margin: 0; font-size: 0.92rem; font-weight: 700; color: #334155; }
  :global(.studio-page .see-all) { display: inline-flex; align-items: center; gap: 0.25rem; font-size: 0.78rem; color: #0e7490; text-decoration: none; }
  :global(.studio-page .see-all:hover) { text-decoration: underline; }
  .dim { color: #94a3b8; margin: 0; font-size: 0.85rem; }
  .empty { display: flex; flex-direction: column; gap: 0.6rem; padding: 1rem 0; color: #64748b; font-size: 0.88rem; }
  .recent { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 0.3rem; }
  :global(.studio-page .recent-link) { display: flex; align-items: center; gap: 0.5rem; padding: 0.45rem 0.6rem; border-radius: 8px; text-decoration: none; color: #334155; }
  :global(.studio-page .recent-link:hover) { background: #f8fafc; }
  .recent-name { flex: 1; font-weight: 600; }
  .recent-time { font-size: 0.74rem; color: #94a3b8; }
  .pill { display: inline-flex; align-items: center; gap: 3px; font-size: 0.68rem; padding: 2px 7px; border-radius: 999px; font-weight: 600; }
  .pill-ok { background: #dcfce7; color: #15803d; }
  .pill-fail { background: #fee2e2; color: #b91c1c; }
  .steps { margin: 0; padding-left: 1.1rem; display: flex; flex-direction: column; gap: 0.5rem; font-size: 0.86rem; color: #475569; line-height: 1.45; }
  .steps li { padding-left: 0.2rem; }
  :global(.studio-page .step-link) { display: inline-flex; align-items: center; gap: 0.2rem; color: #0e7490; text-decoration: none; margin-left: 0.4rem; font-weight: 600; }
  :global(.studio-page .step-link:hover) { text-decoration: underline; }
  code { background: #f1f5f9; padding: 1px 5px; border-radius: 4px; font-size: 0.8em; }

  @media (max-width: 760px) {
    .row { grid-template-columns: 1fr; }
  }

  :global(:is([data-theme="dark"], .dark) .studio-page .kpi) { background: var(--bg-strong); }
  :global(:is([data-theme="dark"], .dark)) .kpi-icon { background: rgba(255,255,255,0.06); color: var(--ink-500); }
  :global(:is([data-theme="dark"], .dark)) .kpi-icon-shapes { background: rgba(34,211,238,0.18); color: #67e8f9; }
  :global(:is([data-theme="dark"], .dark)) .kpi-icon-pipe { background: rgba(139,92,246,0.2); color: #c4b5fd; }
  :global(:is([data-theme="dark"], .dark)) .kpi-icon-data { background: rgba(245,158,11,0.18); color: #fcd34d; }
  :global(:is([data-theme="dark"], .dark)) .kpi-icon-runs { background: rgba(16,185,129,0.18); color: #6ee7b7; }
  :global(:is([data-theme="dark"], .dark)) .kpi-value { color: var(--ink-900); }
  :global(:is([data-theme="dark"], .dark)) .kpi-sub.warn { color: #fcd34d; }
  :global(:is([data-theme="dark"], .dark)) .panel-head h3 { color: var(--ink-800); }
  :global(:is([data-theme="dark"], .dark) .studio-page .see-all) { color: var(--brand-700); }
  :global(:is([data-theme="dark"], .dark) .studio-page .recent-link) { color: var(--ink-800); }
  :global(:is([data-theme="dark"], .dark) .studio-page .recent-link:hover) { background: rgba(255,255,255,0.04); }
  :global(:is([data-theme="dark"], .dark)) .pill-ok { background: rgba(16,185,129,0.18); color: #6ee7b7; }
  :global(:is([data-theme="dark"], .dark)) .pill-fail { background: rgba(239,68,68,0.18); color: #fca5a5; }
  :global(:is([data-theme="dark"], .dark) .studio-page .step-link) { color: var(--brand-700); }
  :global(:is([data-theme="dark"], .dark)) code { background: rgba(255,255,255,0.06); color: var(--ink-800); }
  /* Low-contrast body text on dark — Quick start steps, lede, empty/dim states. */
  :global(:is([data-theme="dark"], .dark)) .lede,
  :global(:is([data-theme="dark"], .dark)) .steps { color: var(--ink-700); }
  :global(:is([data-theme="dark"], .dark)) .steps strong,
  :global(:is([data-theme="dark"], .dark)) .steps em { color: var(--ink-900); }
  :global(:is([data-theme="dark"], .dark)) .kpi-label { color: var(--ink-600); }
  :global(:is([data-theme="dark"], .dark)) .empty { color: var(--ink-600); }
  :global(:is([data-theme="dark"], .dark)) .dim { color: var(--ink-500); }
</style>
