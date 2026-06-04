<script>
  // Validation pipelines list. Phase 1 ships read-only browsing + Run-now;
  // Phase 3 adds the full create/edit form with the friendly schedule builder
  // and the gate-writes confirmation flow.
  import { onMount } from 'svelte';
  import { t } from 'svelte-i18n';
  import { listPipelines, runPipeline, listLatestPipelineRuns } from '../lib/api.js';
  import { Workflow, Play, Loader2, Clock, ShieldCheck, Zap, GitMerge, AlertTriangle, Check, X, Calendar } from 'lucide-svelte';
  import { Link, navigate } from '../lib/router/index.js';
  import ShaclStudioNav from '../components/ShaclStudioNav.svelte';
  import { isAuthenticated, authInitialized } from '../lib/stores.js';
  import { toastError, toastSuccess } from '../lib/toast.ts';

  let pipelines = [];
  let latest = {};
  let loading = true;
  let running = new Set();

  let _guardChecked = false;
  $: if ($authInitialized && !_guardChecked) {
    _guardChecked = true;
    if (!$isAuthenticated) navigate('/login');
  }

  onMount(async () => {
    try {
      pipelines = await listPipelines();
      if (pipelines.length) {
        try {
          const runs = await listLatestPipelineRuns(pipelines.map((p) => p.id));
          latest = Object.fromEntries(runs.map((r) => [r.pipeline_id, r]));
        } catch {}
      }
    } finally {
      loading = false;
    }
  });

  async function runNow(p) {
    running = new Set([...running, p.id]);
    try {
      const run = await runPipeline(p.id);
      latest = { ...latest, [p.id]: run };
      pipelines = pipelines.map((x) => x.id === p.id ? { ...x, last_run_at: run.ran_at, last_conforms: run.conforms } : x);
      toastSuccess(run.conforms ? $t('pages.pipelinesList.validationPassed') : $t('pages.pipelinesList.violationsFound', { values: { count: run.violation_count } }));
    } catch (e) {
      toastError(e.message);
    } finally {
      const next = new Set(running); next.delete(p.id); running = next;
    }
  }

  function relativeTime(iso) {
    if (!iso) return '';
    const sec = Math.round((Date.now() - new Date(iso).getTime()) / 1000);
    if (sec < 60) return $t('pages.pipelinesList.timeJustNow');
    const min = Math.round(sec / 60); if (min < 60) return $t('pages.pipelinesList.timeMinutesAgo', { values: { count: min } });
    const hr = Math.round(min / 60); if (hr < 24) return $t('pages.pipelinesList.timeHoursAgo', { values: { count: hr } });
    const day = Math.round(hr / 24); if (day < 30) return $t('pages.pipelinesList.timeDaysAgo', { values: { count: day } });
    return $t('pages.pipelinesList.timeMonthsAgo', { values: { count: Math.round(day / 30) } });
  }
</script>

<div class="pipelines-page">
  <ShaclStudioNav />

  <div class="card toolbar">
    <h2>{$t('pages.pipelinesList.heading')}</h2>
    <!-- eslint-disable-next-line svelte/no-at-html-tags -- trusted static i18n string -->
    <p class="dim">{@html $t('pages.pipelinesList.intro')}</p>
    <div class="toolbar-cta">
      <Link to="/shacl/shapes" class="btn btn-sm btn-ghost">{$t('pages.pipelinesList.shapesLibrary')}</Link>
      <Link to="/shacl/pipelines/new" class="btn">{$t('pages.pipelinesList.newPipeline')}</Link>
    </div>
  </div>

  {#if loading}
    <div class="card placeholder"><Loader2 size={24} class="spin" /><p>{$t('pages.pipelinesList.loadingPipelines')}</p></div>
  {:else if pipelines.length === 0}
    <div class="card placeholder">
      <Workflow size={36} strokeWidth={1.2} />
      <h3>{$t('pages.pipelinesList.emptyHeading')}</h3>
      <p>{$t('pages.pipelinesList.emptyDesc')}</p>
    </div>
  {:else}
    <ul class="pipe-list">
      {#each pipelines as p (p.id)}
        {@const last = latest[p.id]}
        <li class="pipe-card">
          <div class="pipe-main">
            <div class="pipe-head">
              <Workflow size={14} class="pipe-icon" />
              <Link to={`/shacl/pipelines/${p.id}`} class="pipe-name-link">{p.name}</Link>
              <span class="chip chip-vis">{p.visibility}</span>
              {#if p.trigger_on_write}
                <span class="chip chip-trigger"><Zap size={10} /> {$t('pages.pipelinesList.chipOnWrite')}</span>
              {/if}
              {#if p.schedule_cron}
                <span class="chip chip-trigger" title={p.schedule_cron}><Calendar size={10} /> {$t('pages.pipelinesList.chipScheduled')}</span>
              {/if}
              {#if p.gate_writes}
                <span class="chip chip-gate"><ShieldCheck size={10} /> {$t('pages.pipelinesList.chipGatesWrites')}</span>
              {/if}
              {#if p.run_inference}
                <span class="chip chip-inf"><GitMerge size={10} /> {$t('pages.pipelinesList.chipInference')}</span>
              {/if}
            </div>
            {#if p.description}<p class="pipe-desc">{p.description}</p>{/if}
            <div class="pipe-meta">
              <span><strong>{p.shape_graph_ids.length}</strong> {$t('pages.pipelinesList.shapeGraphsLabel', { values: { count: p.shape_graph_ids.length } })}</span>
              <span>·</span>
              <span><strong>{p.dataset_ids.length || p.graph_iris.length}</strong> {p.graph_iris.length ? $t('pages.pipelinesList.graphsInScopeLabel', { values: { count: p.graph_iris.length } }) : $t('pages.pipelinesList.datasetsInScopeLabel', { values: { count: p.dataset_ids.length } })}</span>
              <span>·</span>
              <span class="dim">{$t('pages.pipelinesList.severityThreshold', { values: { value: p.severity_threshold } })}</span>
            </div>
            {#if last}
              <div class="pipe-last">
                {#if last.conforms}
                  <span class="pill pill-ok"><Check size={11} /> {$t('pages.pipelinesList.passed')}</span>
                {:else}
                  <span class="pill pill-fail"><X size={11} /> {$t('pages.pipelinesList.failedIssues', { values: { count: last.results_count } })}</span>
                {/if}
                <span class="pipe-last-time"><Clock size={10} /> {relativeTime(last.ran_at)}</span>
                <span class="pipe-last-counts">
                  {#if last.violation_count}<span class="hc hc-v">{last.violation_count}V</span>{/if}
                  {#if last.warning_count}<span class="hc hc-w">{last.warning_count}W</span>{/if}
                  {#if last.info_count}<span class="hc hc-i">{last.info_count}I</span>{/if}
                </span>
              </div>
            {:else}
              <div class="pipe-last dim"><AlertTriangle size={11} /> {$t('pages.pipelinesList.neverRun')}</div>
            {/if}
          </div>
          <div class="pipe-actions">
            <button class="btn btn-sm" on:click={() => runNow(p)} disabled={running.has(p.id) || !p.shape_graph_ids.length}>
              {#if running.has(p.id)}<Loader2 size={12} class="spin" /> {$t('pages.pipelinesList.running')}{:else}<Play size={12} /> {$t('pages.pipelinesList.runNow')}{/if}
            </button>
          </div>
        </li>
      {/each}
    </ul>
  {/if}
</div>

<style>
  .pipelines-page { display: flex; flex-direction: column; }
  .toolbar { padding: 0.85rem 1rem !important; margin-bottom: 0.85rem; display: flex; flex-direction: column; gap: 0.25rem; position: relative; }
  .toolbar h2 { margin: 0; font-size: 1.05rem; }
  .toolbar .dim { margin: 0; color: #64748b; font-size: 0.85rem; max-width: 60rem; }
  .toolbar-cta { position: absolute; top: 0.85rem; right: 1rem; display: flex; gap: 0.4rem; }
  .placeholder { display: flex; flex-direction: column; align-items: center; gap: 0.6rem; padding: 3rem 1.5rem; color: #64748b; text-align: center; }
  .placeholder p { margin: 0; max-width: 32rem; }

  .pipe-list { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 0.6rem; }
  .pipe-card { display: flex; align-items: center; gap: 1rem; background: #fff; border: 1px solid var(--line-soft); border-radius: 12px; padding: 0.75rem 0.95rem; }
  .pipe-card:hover { border-color: #cbd5e1; }
  .pipe-main { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 0.35rem; }
  .pipe-head { display: flex; align-items: center; gap: 0.4rem; flex-wrap: wrap; }
  :global(.pipe-icon) { color: #6d28d9; }
  :global(.pipe-name-link) { font-weight: 600; color: #1e293b; text-decoration: none; }
  :global(.pipe-name-link:hover) { color: #2F7A8C; text-decoration: underline; }
  .pipe-desc { margin: 0; color: #64748b; font-size: 0.85rem; }
  .pipe-meta { font-size: 0.78rem; color: #64748b; display: flex; gap: 0.4rem; flex-wrap: wrap; }
  .pipe-meta strong { color: #1e293b; }
  .pipe-last { display: flex; align-items: center; gap: 0.5rem; font-size: 0.8rem; color: #475569; }
  .pipe-last-time { display: inline-flex; align-items: center; gap: 0.25rem; color: #94a3b8; font-size: 0.75rem; }
  .pipe-last-counts { display: inline-flex; gap: 0.25rem; }
  .hc { font-size: 0.7rem; padding: 1px 6px; border-radius: 999px; font-weight: 600; }
  .hc-v { background: #fee2e2; color: #991b1b; }
  .hc-w { background: #fef3c7; color: #92400e; }
  .hc-i { background: #dbeafe; color: #1e40af; }
  .pipe-actions { flex-shrink: 0; }

  .chip { display: inline-flex; align-items: center; gap: 0.2rem; font-size: 0.68rem; padding: 2px 7px; border-radius: 999px; background: #f1f5f9; color: #475569; font-weight: 600; text-transform: capitalize; }
  .chip-trigger { background: #ede9fe; color: #5b21b6; }
  .chip-gate { background: #fee2e2; color: #b91c1c; }
  .chip-inf { background: #fef3c7; color: #92400e; }
  .chip-vis { background: #f1f5f9; color: #64748b; }
  .pill { display: inline-flex; align-items: center; gap: 3px; font-size: 0.7rem; padding: 2px 8px; border-radius: 999px; font-weight: 600; }
  .pill-ok { background: #dcfce7; color: #15803d; }
  .pill-fail { background: #fee2e2; color: #b91c1c; }
  .dim { color: #94a3b8; }

  :global(:is([data-theme="dark"], .dark)) .pipe-card { background: var(--bg-strong); }
  :global(:is([data-theme="dark"], .dark)) .pipe-card:hover { border-color: var(--line-strong); }
  :global(:is([data-theme="dark"], .dark) .pipe-icon) { color: #c4b5fd; }
  :global(:is([data-theme="dark"], .dark) .pipe-name-link) { color: var(--ink-900); }
  :global(:is([data-theme="dark"], .dark) .pipe-name-link:hover) { color: var(--brand-700); }
  :global(:is([data-theme="dark"], .dark)) .pipe-meta strong { color: var(--ink-900); }
  :global(:is([data-theme="dark"], .dark)) .hc-v { background: rgba(239,68,68,0.18); color: #fca5a5; }
  :global(:is([data-theme="dark"], .dark)) .hc-w { background: rgba(245,158,11,0.18); color: #fcd34d; }
  :global(:is([data-theme="dark"], .dark)) .hc-i { background: rgba(59,130,246,0.2); color: #93c5fd; }
  :global(:is([data-theme="dark"], .dark)) .chip { background: rgba(255,255,255,0.06); color: var(--ink-400); }
  :global(:is([data-theme="dark"], .dark)) .chip-trigger { background: rgba(139,92,246,0.2); color: #c4b5fd; }
  :global(:is([data-theme="dark"], .dark)) .chip-gate { background: rgba(239,68,68,0.18); color: #fca5a5; }
  :global(:is([data-theme="dark"], .dark)) .chip-inf { background: rgba(245,158,11,0.18); color: #fcd34d; }
  :global(:is([data-theme="dark"], .dark)) .chip-vis { background: rgba(255,255,255,0.06); color: var(--ink-500); }
  :global(:is([data-theme="dark"], .dark)) .pill-ok { background: rgba(16,185,129,0.18); color: #6ee7b7; }
  :global(:is([data-theme="dark"], .dark)) .pill-fail { background: rgba(239,68,68,0.18); color: #fca5a5; }
</style>
