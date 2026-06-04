<script>
  // SHACL Studio — Results dashboard. Combines pipeline runs and legacy
  // dataset validation runs into one timeline, with an org-wide pass-rate KPI
  // and click-through into the full report (rendered via IssueResults).
  import { onMount } from 'svelte';
  import {
    listPipelines, listLatestPipelineRuns, getPipelineRun, runPipeline,
    listDatasets, listLatestValidationRuns, getLatestValidationRun, validateDataset,
    myDatasetUsage,
  } from '../lib/api.js';
  import { ShieldCheck, Check, X as XIcon, AlertTriangle, Clock, Workflow, Database, Loader2, ChevronRight, Play, RotateCw, FlaskConical, FileWarning, Info } from 'lucide-svelte';
  import { Link, navigate } from '../lib/router/index.js';
  import ShaclStudioNav from '../components/ShaclStudioNav.svelte';
  import IssueResults from '../components/IssueResults.svelte';
  import { isAuthenticated, authInitialized, user } from '../lib/stores.js';
  import { t as i18nT } from 'svelte-i18n';

  let pipelines = [];
  let datasets = [];
  let pipelineLatest = {};          // pipeline_id → run summary
  let datasetLatest = {};           // dataset_id → run summary
  let usageByDataset = {};          // dataset_id → { use_count, last_used } (my own footprint)

  // Drill-down state — when set, the right pane shows the full report.
  let selected = null;              // { kind: 'pipeline'|'dataset', id, name, run, test? }
  let selectedReport = null;
  let selectedLoading = false;

  // Active KPI filter: null | 'passed' | 'failed' | 'violations' | 'never'.
  let filter = null;
  // Key of the row whose run/test is currently in flight (one at a time).
  let busyKey = null;
  let runError = '';

  let loading = true;

  let _guardChecked = false;
  $: if ($authInitialized && !_guardChecked) {
    _guardChecked = true;
    if (!$isAuthenticated) navigate('/login');
  }

  onMount(async () => {
    try {
      let usage;
      [pipelines, datasets, usage] = await Promise.all([
        listPipelines().catch(() => []),
        listDatasets().catch(() => []),
        myDatasetUsage().catch(() => []),
      ]);
      usageByDataset = Object.fromEntries((usage || []).map((u) => [u.dataset_id, u]));
      if (pipelines.length) {
        try {
          const runs = await listLatestPipelineRuns(pipelines.map((p) => p.id));
          pipelineLatest = Object.fromEntries(runs.map((r) => [r.pipeline_id, r]));
        } catch {}
      }
      if (datasets.length) {
        try {
          const runs = await listLatestValidationRuns(datasets.map((d) => d.id));
          datasetLatest = Object.fromEntries(runs.map((r) => [r.dataset_id, r]));
        } catch {}
      }
    } finally {
      loading = false;
    }
  });

  // Every accessible pipeline + every dataset that is validatable (has shapes)
  // or already has a recorded run. `run` is null when it has never run.
  $: items = (() => {
    const out = [];
    for (const p of pipelines) {
      const meta = (p.targets || []).some((t) => t.kind === 'shapegraph');
      out.push({ kind: 'pipeline', id: p.id, name: p.name, owner_type: p.owner_type, owner_id: p.owner_id, run: pipelineLatest[p.id] || null, runnable: true, datasetIds: p.dataset_ids || [], meta });
    }
    for (const d of datasets) {
      const run = datasetLatest[d.id] || null;
      if (!d.shapes_graph_iri && !run) continue;
      out.push({ kind: 'dataset', id: d.id, name: d.name, owner_type: d.owner_type, owner_id: d.owner_id, run, runnable: !!d.shapes_graph_iri, datasetIds: [d.id] });
    }
    return out;
  })();

  const keyOf = (t) => `${t.kind}:${t.id}`;
  const ranAtOf = (t) => t.run ? (t.run.ran_at || t.run.run_timestamp || '') : '';
  // My usage signal for an item: frequency = highest use_count across its
  // datasets; recency = most recent last_used. Pipelines inherit from the
  // datasets they validate.
  const useCountOf = (t) => (t.datasetIds || []).reduce((m, id) => Math.max(m, usageByDataset[id]?.use_count || 0), 0);
  const lastUsedOf = (t) => (t.datasetIds || []).reduce((m, id) => {
    const lu = usageByDataset[id]?.last_used || '';
    return lu > m ? lu : m;
  }, '');

  // Filtered + ranked list. Ranking puts failing runs first (errors rank
  // higher), then runs I own, then by violation count, then by recency.
  $: visible = (() => {
    const f = filter;
    const list = items.filter((t) => {
      if (f === 'never') return !t.run;
      if (!t.run) return false;                 // run-based views hide never-run
      if (f === 'passed') return t.run.conforms;
      if (f === 'failed') return !t.run.conforms;
      if (f === 'violations') return (t.run.violation_count || 0) > 0;
      return true;                              // no filter → anything with a run
    });
    const mid = $user?.id;
    const mine = (t) => t.owner_type === 'user' && mid && t.owner_id === mid;
    const tier = (t) => !t.run ? 2 : (t.run.conforms ? 1 : 0);  // failed, passed, never
    list.sort((a, b) => {
      if (tier(a) !== tier(b)) return tier(a) - tier(b);
      const am = mine(a), bm = mine(b);
      if (am !== bm) return am ? -1 : 1;
      // "Use a lot / recently use" — surface the datasets I touch most.
      const au = useCountOf(a), bu = useCountOf(b);
      if (au !== bu) return bu - au;
      const al = lastUsedOf(a), bl = lastUsedOf(b);
      if (al !== bl) return bl.localeCompare(al);
      const av = a.run?.violation_count || 0, bv = b.run?.violation_count || 0;
      if (av !== bv) return bv - av;
      return ranAtOf(b).localeCompare(ranAtOf(a));
    });
    return list;
  })();

  // KPI summary across all items with a run.
  $: summary = (() => {
    const s = { total: 0, passed: 0, failed: 0, violations: 0, warnings: 0, infos: 0, never: 0 };
    for (const t of items) {
      if (!t.run) { s.never++; continue; }
      s.total++;
      if (t.run.conforms) s.passed++; else s.failed++;
      s.violations += t.run.violation_count || 0;
      s.warnings += t.run.warning_count || 0;
      s.infos += t.run.info_count || 0;
    }
    s.passRate = s.total ? Math.round((s.passed / s.total) * 100) : 0;
    return s;
  })();

  function toggleFilter(f) { filter = filter === f ? null : f; }

  $: FILTER_LABEL = { passed: $i18nT('pages.shaclResults.filterPassed'), failed: $i18nT('pages.shaclResults.filterFailed'), violations: $i18nT('pages.shaclResults.filterWithViolations'), never: $i18nT('pages.shaclResults.filterNeverRun') };

  function relativeTime(iso) {
    if (!iso) return '';
    const sec = Math.round((Date.now() - new Date(iso).getTime()) / 1000);
    if (sec < 60) return $i18nT('pages.shaclResults.justNow');
    const min = Math.round(sec / 60); if (min < 60) return $i18nT('pages.shaclResults.minutesAgo', { values: { count: min } });
    const hr = Math.round(min / 60); if (hr < 24) return $i18nT('pages.shaclResults.hoursAgo', { values: { count: hr } });
    const day = Math.round(hr / 24); if (day < 30) return $i18nT('pages.shaclResults.daysAgo', { values: { count: day } });
    return $i18nT('pages.shaclResults.monthsAgo', { values: { count: Math.round(day / 30) } });
  }

  async function drillInto(item) {
    if (!item.run) return;            // never-run rows have nothing to show
    selected = item;
    selectedReport = null;
    selectedLoading = true;
    try {
      if (item.kind === 'pipeline') {
        const run = await getPipelineRun(item.id, item.run.id);
        selectedReport = run.report;
      } else {
        const run = await getLatestValidationRun(item.id);
        selectedReport = run?.report;
      }
    } catch (_) {
      selectedReport = null;
    } finally {
      selectedLoading = false;
    }
  }

  // Direct run / re-run — records an official run and refreshes the KPIs.
  async function doRun(t) {
    if (busyKey) return;
    busyKey = keyOf(t);
    runError = '';
    try {
      if (t.kind === 'pipeline') {
        const run = await runPipeline(t.id);
        pipelineLatest = { ...pipelineLatest, [t.id]: run };
      } else {
        await validateDataset(t.id);
        const runs = await listLatestValidationRuns([t.id]);
        if (runs && runs[0]) datasetLatest = { ...datasetLatest, [t.id]: runs[0] };
      }
    } catch (e) {
      runError = e.message || $i18nT('pages.shaclResults.runFailed');
    } finally {
      busyKey = null;
    }
  }

  // Test run — validates but is NOT recorded; shows the report inline only.
  async function testRun(t) {
    if (busyKey) return;
    busyKey = keyOf(t);
    runError = '';
    selected = { ...t, test: true, run: { ran_at: new Date().toISOString() } };
    selectedReport = null;
    selectedLoading = true;
    try {
      if (t.kind === 'pipeline') {
        const run = await runPipeline(t.id, { test: true });
        selectedReport = run.report;
      } else {
        const res = await validateDataset(t.id, {}, { test: true });
        selectedReport = res.report;
      }
    } catch (e) {
      runError = e.message || $i18nT('pages.shaclResults.testRunFailed');
      selectedReport = null;
    } finally {
      busyKey = null;
      selectedLoading = false;
    }
  }
</script>

<div class="results-page">
  <ShaclStudioNav />

  <!-- KPIs across all accessible pipelines + datasets — click to filter -->
  <div class="card kpis">
    <button type="button" class="kpi-cell" class:active={filter === null} on:click={() => (filter = null)} title={$i18nT('pages.shaclResults.showAllRuns')}>
      <div class="kpi-icon kpi-primary"><ShieldCheck size={18} /></div>
      <div>
        <div class="kpi-value">{summary.passRate}%</div>
        <div class="kpi-label">{$i18nT('pages.shaclResults.passRate')}</div>
      </div>
    </button>
    <button type="button" class="kpi-cell" class:active={filter === 'passed'} on:click={() => toggleFilter('passed')} title={$i18nT('pages.shaclResults.showOnlyPassing')}>
      <div class="kpi-icon kpi-ok"><Check size={18} /></div>
      <div><div class="kpi-value">{summary.passed}</div><div class="kpi-label">{$i18nT('pages.shaclResults.filterPassed')}</div></div>
    </button>
    <button type="button" class="kpi-cell" class:active={filter === 'failed'} on:click={() => toggleFilter('failed')} title={$i18nT('pages.shaclResults.showOnlyFailing')}>
      <div class="kpi-icon kpi-fail"><XIcon size={18} /></div>
      <div><div class="kpi-value">{summary.failed}</div><div class="kpi-label">{$i18nT('pages.shaclResults.filterFailed')}</div></div>
    </button>
    <button type="button" class="kpi-cell" class:active={filter === 'violations'} on:click={() => toggleFilter('violations')} title={$i18nT('pages.shaclResults.showWithViolations')}>
      <div class="kpi-icon kpi-warn"><AlertTriangle size={18} /></div>
      <div><div class="kpi-value">{summary.violations}</div><div class="kpi-label">{$i18nT('pages.shaclResults.totalViolations')}</div></div>
    </button>
    {#if summary.never > 0}
      <button type="button" class="kpi-cell" class:active={filter === 'never'} on:click={() => toggleFilter('never')} title={$i18nT('pages.shaclResults.showNeverRan')}>
        <div class="kpi-icon kpi-muted"><Clock size={18} /></div>
        <div><div class="kpi-value">{summary.never}</div><div class="kpi-label">{$i18nT('pages.shaclResults.filterNeverRun')}</div></div>
      </button>
    {/if}
  </div>

  {#if runError}
    <div class="run-error"><AlertTriangle size={14} /> {runError}</div>
  {/if}

  <div class="layout">
    <!-- Timeline of latest runs across pipelines + datasets -->
    <section class="card timeline">
      <header class="panel-head">
        <h3>{filter ? FILTER_LABEL[filter] : $i18nT('pages.shaclResults.recentRuns')}</h3>
        <span class="dim">
          {visible.length} {visible.length === 1 ? $i18nT('pages.shaclResults.item') : $i18nT('pages.shaclResults.items')}
          {#if filter}<button type="button" class="clear-filter" on:click={() => (filter = null)}>{$i18nT('pages.shaclResults.clear')}</button>{/if}
        </span>
      </header>
      {#if loading}
        <div class="placeholder"><Loader2 size={22} class="spin" /><p>{$i18nT('pages.shaclResults.loadingRuns')}</p></div>
      {:else if visible.length === 0}
        <div class="placeholder">
          <ShieldCheck size={36} strokeWidth={1.2} />
          {#if filter}
            <h4>{$i18nT('pages.shaclResults.nothingMatches')}</h4>
            <p>{$i18nT('pages.shaclResults.noItemsInView', { values: { view: FILTER_LABEL[filter] } })} <button type="button" class="clear-filter" on:click={() => (filter = null)}>{$i18nT('pages.shaclResults.showAll')}</button></p>
          {:else}
            <h4>{$i18nT('pages.shaclResults.noRunsYet')}</h4>
            <p>{$i18nT('pages.shaclResults.noRunsYetDesc')}</p>
            <div class="placeholder-actions">
              <Link to="/shacl/pipelines" class="btn btn-sm"><Workflow size={13} /> {$i18nT('pages.shaclResults.pipelines')}</Link>
              <Link to="/datasets" class="btn btn-sm btn-ghost"><Database size={13} /> {$i18nT('pages.shaclResults.datasets')}</Link>
            </div>
          {/if}
        </div>
      {:else}
        <ul class="run-list">
          {#each visible as t (keyOf(t))}
            {@const busy = busyKey === keyOf(t)}
            <!-- svelte-ignore a11y_no_noninteractive_element_to_interactive_role -->
            <li class="run-row" class:selected={selected && selected.kind === t.kind && selected.id === t.id} class:clickable={!!t.run} on:click={() => drillInto(t)} role="button" tabindex="0" on:keydown={(e) => e.key === 'Enter' && drillInto(t)}>
              <div class="run-kind">
                {#if t.kind === 'pipeline'}<Workflow size={13} class="kicon" />{:else}<Database size={13} class="dicon" />{/if}
                <span class="run-name">{t.name}</span>
                {#if t.meta}<span class="meta-tag" title={$i18nT('pages.shaclResults.metaValidationTooltip')}><FlaskConical size={9} /> {$i18nT('pages.shaclResults.meta')}</span>{/if}
              </div>
              <div class="run-status">
                {#if !t.run}
                  <span class="pill pill-muted"><Clock size={11} /> {$i18nT('pages.shaclResults.neverRun')}</span>
                {:else if t.run.conforms}
                  <span class="pill pill-ok"><Check size={11} /> {$i18nT('pages.shaclResults.passed')}</span>
                {:else}
                  <span class="pill pill-fail"><FileWarning size={11} /> {t.run.results_count === 1 ? $i18nT('pages.shaclResults.issueCountSingular', { values: { count: t.run.results_count } }) : $i18nT('pages.shaclResults.issueCountPlural', { values: { count: t.run.results_count } })}</span>
                {/if}
                {#if t.run}
                  <span class="counts">
                    {#if t.run.violation_count}<span class="hc hc-v">{t.run.violation_count}V</span>{/if}
                    {#if t.run.warning_count}<span class="hc hc-w">{t.run.warning_count}W</span>{/if}
                    {#if t.run.info_count}<span class="hc hc-i">{t.run.info_count}I</span>{/if}
                  </span>
                {/if}
              </div>
              <div class="run-time">
                {#if t.run}<Clock size={10} /> {relativeTime(ranAtOf(t))}{/if}
              </div>
              <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
              <div class="run-actions" on:click|stopPropagation>
                {#if t.runnable}
                  <button class="act" title={$i18nT('pages.shaclResults.testRunTooltip')} on:click={() => testRun(t)} disabled={!!busyKey}>
                    {#if busy}<Loader2 size={13} class="spin" />{:else}<FlaskConical size={13} />{/if}
                  </button>
                  <button class="act act-run" title={t.run ? $i18nT('pages.shaclResults.reRunTooltip') : $i18nT('pages.shaclResults.runNowTooltip')} on:click={() => doRun(t)} disabled={!!busyKey}>
                    {#if t.run}<RotateCw size={13} />{:else}<Play size={13} />{/if}
                  </button>
                {/if}
                {#if t.run}<ChevronRight size={12} class="chev" />{/if}
              </div>
            </li>
          {/each}
        </ul>
      {/if}
    </section>

    <!-- Report drill-down -->
    <section class="card report-pane">
      {#if !selected}
        <div class="placeholder">
          <ShieldCheck size={36} strokeWidth={1.2} />
          <h4>{$i18nT('pages.shaclResults.pickRun')}</h4>
          <p>{$i18nT('pages.shaclResults.pickRunDesc')}</p>
        </div>
      {:else}
        <header class="report-head">
          <div class="report-title">
            {#if selected.kind === 'pipeline'}<Workflow size={14} class="kicon" />{:else}<Database size={14} class="dicon" />{/if}
            <strong>{selected.name}</strong>
            {#if selected.meta}<span class="meta-tag"><FlaskConical size={10} /> {$i18nT('pages.shaclResults.meta')}</span>{/if}
            {#if selected.test}<span class="test-tag"><FlaskConical size={10} /> {$i18nT('pages.shaclResults.testRunNotRecorded')}</span>{/if}
            <span class="dim">· {relativeTime(selected.run.ran_at || selected.run.run_timestamp)}</span>
          </div>
          <div class="report-actions">
            {#if selected.kind === 'pipeline'}
              <Link to={`/shacl/pipelines/${selected.id}`} class="btn btn-xs btn-ghost"><Play size={11} /> {$i18nT('pages.shaclResults.openPipeline')}</Link>
            {:else}
              <Link to={`/datasets/${selected.id}`} class="btn btn-xs btn-ghost"><Database size={11} /> {$i18nT('pages.shaclResults.openDataset')}</Link>
            {/if}
          </div>
        </header>
        {#if selectedLoading}
          <div class="placeholder"><Loader2 size={20} class="spin" /></div>
        {:else if selectedReport && selectedReport.conforms}
          <div class="report-ok"><Check size={20} /> <div><strong>{$i18nT('pages.shaclResults.dataConforms')}</strong><br><small>{$i18nT('pages.shaclResults.noIssuesForShapes')}</small></div></div>
        {:else if selectedReport && (selectedReport.results || []).length}
          <IssueResults results={selectedReport.results} datasetName={selected.name} />
        {:else}
          <div class="placeholder"><Info /> <p>{$i18nT('pages.shaclResults.noReportPayload')}</p></div>
        {/if}
      {/if}
    </section>
  </div>
</div>

<style>
  .results-page { display: flex; flex-direction: column; gap: 0.85rem; }

  .kpis { display: grid; grid-template-columns: repeat(auto-fit, minmax(160px, 1fr)); gap: 0.5rem; padding: 0.85rem 1rem !important; }
  .kpi-cell { display: flex; align-items: center; gap: 0.55rem; padding: 0.4rem 0.5rem; background: none; border: 1px solid transparent; border-radius: 10px; cursor: pointer; text-align: left; font: inherit; color: inherit; transition: background 0.12s, border-color 0.12s; }
  .kpi-cell:hover { background: #f8fafc; }
  .kpi-cell.active { background: var(--bg-accent-soft, #ecfeff); border-color: #7ED6D0; }
  .run-error { display: flex; align-items: center; gap: 0.4rem; padding: 0.5rem 0.85rem; border-radius: 10px; background: #fee2e2; color: #b91c1c; font-size: 0.82rem; }
  .clear-filter { background: none; border: none; color: #2F7A8C; cursor: pointer; font: inherit; font-size: inherit; text-decoration: underline; padding: 0 0.15rem; }
  .kpi-icon { display: grid; place-items: center; width: 36px; height: 36px; border-radius: 10px; background: #f1f5f9; color: #475569; }
  .kpi-primary { background: linear-gradient(135deg, #7ED6D0, #2F7A8C); color: white; }
  .kpi-ok { background: #dcfce7; color: #15803d; }
  .kpi-fail { background: #fee2e2; color: #b91c1c; }
  .kpi-warn { background: #fef3c7; color: #b45309; }
  .kpi-muted { background: #f1f5f9; color: #64748b; }
  .kpi-value { font-size: 1.25rem; font-weight: 700; color: #1e293b; line-height: 1; }
  .kpi-label { font-size: 0.7rem; color: #64748b; text-transform: uppercase; letter-spacing: 0.06em; margin-top: 0.15rem; }

  .layout { display: grid; grid-template-columns: minmax(280px, 360px) minmax(0, 1fr); gap: 0.85rem; align-items: start; }
  .panel-head { display: flex; align-items: center; justify-content: space-between; padding: 0.6rem 1rem; border-bottom: 1px solid var(--line-soft); }
  .panel-head h3 { margin: 0; font-size: 0.9rem; font-weight: 700; color: #334155; }
  .timeline { padding: 0 !important; max-height: calc(100vh - 18rem); overflow: hidden; display: flex; flex-direction: column; }
  .run-list { list-style: none; margin: 0; padding: 0; overflow: auto; flex: 1; }
  .run-row { display: grid; grid-template-columns: minmax(0, 1fr) auto auto auto; gap: 0.4rem; padding: 0.55rem 0.85rem; border-top: 1px solid #f1f5f9; align-items: center; }
  .run-row.clickable { cursor: pointer; }
  .run-row:first-child { border-top: none; }
  .run-row:hover { background: #f8fafc; }
  .run-actions { display: inline-flex; align-items: center; gap: 0.2rem; }
  .act { display: grid; place-items: center; width: 26px; height: 26px; border-radius: 7px; border: 1px solid var(--line-soft); background: #fff; color: #475569; cursor: pointer; transition: background 0.12s, color 0.12s, border-color 0.12s; }
  .act:hover:not(:disabled) { background: #f1f5f9; color: #1e293b; }
  .act.act-run:hover:not(:disabled) { border-color: #2F7A8C; color: #2F7A8C; }
  .act:disabled { opacity: 0.45; cursor: not-allowed; }
  :global(.run-actions .chev) { color: #cbd5e1; }
  .test-tag { display: inline-flex; align-items: center; gap: 3px; font-size: 0.66rem; font-weight: 700; padding: 2px 7px; border-radius: 999px; background: #ede9fe; color: #6d28d9; }
  .meta-tag { display: inline-flex; align-items: center; gap: 2px; font-size: 0.6rem; font-weight: 700; padding: 1px 6px; border-radius: 999px; background: #cffafe; color: #0e7490; text-transform: uppercase; letter-spacing: 0.03em; flex-shrink: 0; }
  .pill-muted { background: #f1f5f9; color: #64748b; }
  .run-row.selected { background: linear-gradient(90deg, #ecfeff, #ffffff); border-left: 3px solid #2F7A8C; padding-left: calc(0.85rem - 3px); }
  .run-kind { display: flex; align-items: center; gap: 0.4rem; min-width: 0; }
  .run-name { font-weight: 600; color: #1e293b; font-size: 0.88rem; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .run-status { display: flex; align-items: center; gap: 0.35rem; }
  .run-time { display: inline-flex; align-items: center; gap: 0.25rem; font-size: 0.74rem; color: #94a3b8; }
  :global(.kicon) { color: #6d28d9; }
  :global(.dicon) { color: #2F7A8C; }

  .pill { display: inline-flex; align-items: center; gap: 3px; font-size: 0.68rem; padding: 2px 7px; border-radius: 999px; font-weight: 600; white-space: nowrap; }
  .pill-ok { background: #dcfce7; color: #15803d; }
  .pill-fail { background: #fee2e2; color: #b91c1c; }
  .counts { display: inline-flex; gap: 0.2rem; }
  .hc { font-size: 0.66rem; padding: 1px 5px; border-radius: 999px; font-weight: 700; }
  .hc-v { background: #fee2e2; color: #991b1b; }
  .hc-w { background: #fef3c7; color: #92400e; }
  .hc-i { background: #dbeafe; color: #1e40af; }

  .report-pane { padding: 0 !important; min-height: 360px; }
  .report-head { display: flex; align-items: center; justify-content: space-between; padding: 0.6rem 1rem; border-bottom: 1px solid var(--line-soft); }
  .report-title { display: flex; align-items: center; gap: 0.45rem; min-width: 0; }
  .report-title .dim { color: #94a3b8; font-size: 0.78rem; }
  .report-actions { display: flex; gap: 0.3rem; }

  .report-ok { display: flex; align-items: center; gap: 0.6rem; padding: 0.9rem 1.1rem; background: linear-gradient(90deg, #ecfdf5, #ffffff); color: #065f46; border-bottom: 1px solid var(--line-soft); }
  .placeholder { display: flex; flex-direction: column; align-items: center; gap: 0.5rem; padding: 3rem 1.5rem; color: #64748b; text-align: center; }
  .placeholder h4 { margin: 0; color: #334155; }
  .placeholder p { margin: 0; max-width: 32rem; font-size: 0.88rem; color: #94a3b8; }
  .placeholder-actions { display: flex; gap: 0.4rem; margin-top: 0.4rem; }
  .dim { color: #94a3b8; font-size: 0.78rem; }

  @media (max-width: 880px) {
    .layout { grid-template-columns: 1fr; }
    .timeline { max-height: 50vh; }
  }

  /* ---- Dark mode overrides (scoped rules out-specify global theme.css) ---- */
  :global(:is([data-theme="dark"], .dark)) .kpi-icon { background: rgba(255,255,255,0.06); color: var(--ink-600); }
  :global(:is([data-theme="dark"], .dark)) .kpi-ok { background: rgba(16,185,129,0.18); color: #6ee7b7; }
  :global(:is([data-theme="dark"], .dark)) .kpi-fail { background: rgba(239,68,68,0.18); color: #fca5a5; }
  :global(:is([data-theme="dark"], .dark)) .kpi-warn { background: rgba(245,158,11,0.18); color: #fcd34d; }
  :global(:is([data-theme="dark"], .dark)) .kpi-muted { background: rgba(255,255,255,0.06); color: var(--ink-500); }
  :global(:is([data-theme="dark"], .dark)) .kpi-value { color: var(--ink-900); }
  :global(:is([data-theme="dark"], .dark)) .kpi-label { color: var(--ink-500); }
  :global(:is([data-theme="dark"], .dark)) .panel-head h3,
  :global(:is([data-theme="dark"], .dark)) .placeholder h4 { color: var(--ink-800); }
  :global(:is([data-theme="dark"], .dark)) .run-row { border-top-color: var(--line-soft); }
  :global(:is([data-theme="dark"], .dark)) .run-row:hover { background: rgba(255,255,255,0.04); }
  :global(:is([data-theme="dark"], .dark)) .run-row.selected { background: linear-gradient(90deg, var(--bg-accent-soft), transparent); border-left-color: var(--brand-300); }
  :global(:is([data-theme="dark"], .dark)) .run-name { color: var(--ink-900); }
  :global(:is([data-theme="dark"], .dark)) .run-time { color: var(--ink-500); }
  :global(:is([data-theme="dark"], .dark) .kicon) { color: #c4b5fd; }
  :global(:is([data-theme="dark"], .dark) .dicon) { color: var(--brand-700); }
  :global(:is([data-theme="dark"], .dark)) .pill-ok { background: rgba(16,185,129,0.18); color: #6ee7b7; }
  :global(:is([data-theme="dark"], .dark)) .pill-fail { background: rgba(239,68,68,0.18); color: #fca5a5; }
  :global(:is([data-theme="dark"], .dark)) .hc-v { background: rgba(239,68,68,0.18); color: #fca5a5; }
  :global(:is([data-theme="dark"], .dark)) .hc-w { background: rgba(245,158,11,0.18); color: #fcd34d; }
  :global(:is([data-theme="dark"], .dark)) .hc-i { background: rgba(59,130,246,0.2); color: #93c5fd; }
  :global(:is([data-theme="dark"], .dark)) .report-ok { background: linear-gradient(90deg, rgba(16,185,129,0.14), transparent); color: #6ee7b7; }
  :global(:is([data-theme="dark"], .dark)) .placeholder { color: var(--ink-500); }
  :global(:is([data-theme="dark"], .dark)) .kpi-cell:hover { background: rgba(255,255,255,0.04); }
  :global(:is([data-theme="dark"], .dark)) .kpi-cell.active { background: var(--bg-accent-soft); border-color: var(--brand-400); }
  :global(:is([data-theme="dark"], .dark)) .run-error { background: rgba(239,68,68,0.18); color: #fca5a5; }
  :global(:is([data-theme="dark"], .dark)) .clear-filter { color: var(--brand-700); }
  :global(:is([data-theme="dark"], .dark)) .act { background: rgba(255,255,255,0.06); border-color: var(--line-strong); color: var(--ink-700); }
  :global(:is([data-theme="dark"], .dark)) .act:hover:not(:disabled) { background: rgba(255,255,255,0.10); color: var(--ink-900); }
  :global(:is([data-theme="dark"], .dark)) .act.act-run:hover:not(:disabled) { border-color: var(--brand-400); color: var(--brand-700); }
  :global(:is([data-theme="dark"], .dark)) .pill-muted { background: rgba(255,255,255,0.06); color: var(--ink-500); }
  :global(:is([data-theme="dark"], .dark)) .test-tag { background: rgba(124,58,237,0.22); color: #c4b5fd; }
  :global(:is([data-theme="dark"], .dark)) .meta-tag { background: rgba(6,182,212,0.2); color: #67e8f9; }
  :global(:is([data-theme="dark"], .dark) .run-actions .chev) { color: var(--ink-400); }
</style>
