<script>
  import { onMount } from 'svelte';
  import {
    listDatasets, validateDataset, updateDatasetShacl, getDataset, getOrganisation,
    listAccessibleShapeGraphs, listOrganisations,
    getLatestValidationRun, getValidationHistory, getValidationRun, listLatestValidationRuns,
    listPublicUsers,
  } from '../lib/api.js';
  import { t } from 'svelte-i18n';
  import { Check, X as XIcon, AlertTriangle, Loader2, Play, ShieldCheck, FileWarning, Info, PlayCircle, Database, Lock, History, FileCode, Clock } from 'lucide-svelte';
  import { navigate } from '../lib/router/index.js';
  import Avatar from '../components/Avatar.svelte';
  import Select from '../components/Select.svelte';
  import { isAuthenticated, user } from '../lib/stores.js';
  import PageHeader from '../components/PageHeader.svelte';
  import ShapesEditor from '../components/ShapesEditor.svelte';
  import IssueResults from '../components/IssueResults.svelte';
  import { toastError } from '../lib/toast.ts';

  let organisations = [];
  let accessibleShapeGraphs = [];
  let userMap = {}; // user id -> display name, for resolving dataset owners

  let backDatasetId = null;
  let backOrgId = null;
  let backContextName = null;

  let datasets = [];
  // datasetStatus[id] = { loading, result(report|null), summary(counts|null), error, ranAt }
  let datasetStatus = {};
  let selectedDataset = null;
  let activeTab = 'results';
  let error = '';
  let batchRunning = false;

  let historyRuns = [];
  let historyLoading = false;
  let viewingRunId = null;

  function sevKey(s) { return (s || 'violation').toLowerCase(); }

  function relativeTime(iso) {
    if (!iso) return '';
    const then = new Date(iso).getTime();
    if (isNaN(then)) return '';
    const sec = Math.round((Date.now() - then) / 1000);
    if (sec < 60) return $t('pages.validation.timeJustNow');
    const min = Math.round(sec / 60); if (min < 60) return $t('pages.validation.timeMinutesAgo', { values: { count: min } });
    const hr = Math.round(min / 60); if (hr < 24) return $t('pages.validation.timeHoursAgo', { values: { count: hr } });
    const day = Math.round(hr / 24); if (day < 30) return $t('pages.validation.timeDaysAgo', { values: { count: day } });
    const mo = Math.round(day / 30); if (mo < 12) return $t('pages.validation.timeMonthsAgo', { values: { count: mo } });
    return $t('pages.validation.timeYearsAgo', { values: { count: Math.round(mo / 12) } });
  }

  function summarize(report) {
    const c = { results_count: report.results_count, violation_count: 0, warning_count: 0, info_count: 0, conforms: report.conforms };
    for (const r of report.results || []) {
      const k = sevKey(r.severity);
      if (k === 'warning') c.warning_count++;
      else if (k === 'info') c.info_count++;
      else c.violation_count++;
    }
    return c;
  }
  function summaryFromRun(run) {
    return { results_count: run.results_count, violation_count: run.violation_count, warning_count: run.warning_count, info_count: run.info_count, conforms: run.conforms };
  }

  onMount(async () => {
    const params = new URLSearchParams(window.location.search);
    backDatasetId = params.get('dataset') || null;
    backOrgId = params.get('org') || null;
    if (backDatasetId) {
      getDataset(backDatasetId).then(d => { backContextName = d?.name ?? backDatasetId; }).catch(() => { backContextName = backDatasetId; });
    } else if (backOrgId) {
      getOrganisation(backOrgId).then(o => { backContextName = o?.name ?? backOrgId; }).catch(() => { backContextName = backOrgId; });
    }
    try {
      try { organisations = await listOrganisations(); } catch {}
      try {
        const pub = await listPublicUsers();
        userMap = Object.fromEntries((pub || []).map(u => [String(u.id), u.display_name || u.username]));
      } catch {}
      // Always resolve the current user's own datasets, even if not public.
      if ($user?.id) userMap[String($user.id)] = $user.display_name || $user.username;
      try { const r = await listAccessibleShapeGraphs(); accessibleShapeGraphs = r?.shape_graphs || []; } catch {}
      datasets = await listDatasets();
      if (backDatasetId) {
        datasets = datasets.filter(d => d.id === backDatasetId);
      } else if (backOrgId) {
        datasets = datasets.filter(d => d.owner_type === 'organisation' && String(d.owner_id) === String(backOrgId));
      }
      datasetStatus = Object.fromEntries(datasets.map(d => [d.id, { loading: false, result: null, summary: null, error: null, ranAt: null }]));
      // Seed persisted status so the page survives reloads.
      try {
        const runs = await listLatestValidationRuns(datasets.map(d => d.id));
        for (const run of runs) {
          const st = datasetStatus[run.dataset_id];
          if (st) datasetStatus[run.dataset_id] = { ...st, summary: summaryFromRun(run), ranAt: run.run_timestamp };
        }
        datasetStatus = datasetStatus;
      } catch {}
    } catch (e) {
      error = e.message;
    }
  });

  async function runValidation(dsId) {
    datasetStatus[dsId] = { ...datasetStatus[dsId], loading: true, error: null };
    datasetStatus = datasetStatus;
    try {
      const res = await validateDataset(dsId, {});
      const report = res.report;
      datasetStatus[dsId] = { loading: false, result: report, summary: summarize(report), error: null, ranAt: res.ran_at };
      selectedDataset = dsId;
      viewingRunId = null;
      if (activeTab === 'history') loadHistory(dsId);
    } catch (e) {
      datasetStatus[dsId] = { ...datasetStatus[dsId], loading: false, error: e.message };
    }
    datasetStatus = datasetStatus;
  }

  async function validateAll() {
    batchRunning = true;
    try {
      for (const d of datasets.filter(d => d.shapes_graph_iri)) {
        await runValidation(d.id);
      }
    } finally {
      batchRunning = false;
    }
  }

  async function selectDataset(dsId) {
    selectedDataset = dsId;
    activeTab = 'results';
    viewingRunId = null;
    const st = datasetStatus[dsId];
    if (st && !st.result && st.summary) await loadLatest(dsId);
  }

  async function loadLatest(dsId) {
    try {
      const run = await getLatestValidationRun(dsId);
      if (run) {
        datasetStatus[dsId] = { ...datasetStatus[dsId], result: run.report, summary: summaryFromRun(run), ranAt: run.run_timestamp, error: null };
        datasetStatus = datasetStatus;
      }
    } catch {}
  }

  function setTab(tab) {
    activeTab = tab;
    if (tab === 'history' && selectedDataset) loadHistory(selectedDataset);
  }

  async function loadHistory(dsId) {
    historyLoading = true;
    historyRuns = [];
    try { historyRuns = await getValidationHistory(dsId, 50); } catch (e) { toastError(e.message); } finally { historyLoading = false; }
  }

  async function viewRun(dsId, runId) {
    try {
      const run = await getValidationRun(dsId, runId);
      datasetStatus[dsId] = { ...datasetStatus[dsId], result: run.report, summary: summaryFromRun(run), ranAt: run.run_timestamp };
      datasetStatus = datasetStatus;
      viewingRunId = runId;
      activeTab = 'results';
    } catch (e) { toastError(e.message); }
  }

  async function toggleShaclOnWrite(ds) {
    try {
      await updateDatasetShacl(ds.id, { shacl_on_write: !ds.shacl_on_write, shapes_graph_iri: ds.shapes_graph_iri ?? null });
      ds.shacl_on_write = !ds.shacl_on_write;
      datasets = datasets;
    } catch (e) { error = e.message; }
  }

  async function linkShapes(ds, iri) {
    if (!iri) return;
    try {
      await updateDatasetShacl(ds.id, { shacl_on_write: false, shapes_graph_iri: iri });
      ds.shapes_graph_iri = iri;
      datasets = datasets;
    } catch {}
  }

  $: selectedDs = datasets.find(d => d.id === selectedDataset);
  $: activeResult = selectedDataset ? datasetStatus[selectedDataset]?.result : null;
  $: activeStatus = selectedDataset ? datasetStatus[selectedDataset] : null;

  // Global summary across datasets (uses persisted summaries + freshly run reports).
  $: summary = (() => {
    const s = { conforms: 0, violations: 0, warnings: 0, infos: 0, validated: 0, total: datasets.length, noShapes: 0 };
    for (const d of datasets) {
      if (!d.shapes_graph_iri) s.noShapes += 1;
      const st = datasetStatus[d.id];
      const src = st?.summary;
      if (!src) continue;
      s.validated += 1;
      if (src.conforms) s.conforms += 1;
      s.violations += src.violation_count || 0;
      s.warnings += src.warning_count || 0;
      s.infos += src.info_count || 0;
    }
    return s;
  })();
</script>

<div class="validation-page">
  {#if !$isAuthenticated}
    <div class="auth-gate">
      <Lock size={40} class="auth-gate-icon" />
      <h2>{$t('pages.validation.authGateTitle')}</h2>
      <p>{$t('pages.validation.authGateDesc')}</p>
      <button class="btn" on:click={() => navigate('/login')}>{$t('pages.validation.signIn')}</button>
    </div>
  {:else}

  <PageHeader
    title={$t('pages.validation.title')}
    breadcrumbs={backOrgId
      ? [{ label: $t('pages.validation.breadcrumbOrganisations'), href: '/organisations' }, { label: backContextName ?? '…', href: '/organisations/' + backOrgId }, { label: $t('pages.validation.title') }]
      : backDatasetId
        ? [{ label: $t('pages.validation.breadcrumbDatasets'), href: '/datasets' }, { label: backContextName ?? '…', href: '/datasets/' + backDatasetId }, { label: $t('pages.validation.title') }]
        : [{ label: $t('pages.validation.breadcrumbDatasets'), href: '/datasets' }, { label: $t('pages.validation.title') }]}
  />

  {#if error}<div class="error">{error}</div>{/if}

  <!-- Summary hero (across all datasets) -->
  <div class="summary card">
    <div class="summary-metric summary-primary">
      <div class="metric-icon"><ShieldCheck size={18} /></div>
      <div>
        <div class="metric-value">{summary.validated} / {summary.total}</div>
        <div class="metric-label">{$t('pages.validation.datasetsValidated')}</div>
      </div>
    </div>
    <div class="summary-metric metric-ok">
      <div class="metric-icon"><Check size={18} /></div>
      <div><div class="metric-value">{summary.conforms}</div><div class="metric-label">{$t('pages.validation.metricConform')}</div></div>
    </div>
    <div class="summary-metric metric-violation">
      <div class="metric-icon"><XIcon size={18} /></div>
      <div><div class="metric-value">{summary.violations}</div><div class="metric-label">{$t('pages.validation.metricViolations')}</div></div>
    </div>
    <div class="summary-metric metric-warning">
      <div class="metric-icon"><AlertTriangle size={18} /></div>
      <div><div class="metric-value">{summary.warnings}</div><div class="metric-label">{$t('pages.validation.metricWarnings')}</div></div>
    </div>
    <div class="summary-metric metric-info">
      <div class="metric-icon"><Info size={18} /></div>
      <div><div class="metric-value">{summary.infos}</div><div class="metric-label">{$t('pages.validation.metricInfo')}</div></div>
    </div>
    <div class="summary-actions">
      <button class="btn" on:click={validateAll} disabled={batchRunning || datasets.every(d => !d.shapes_graph_iri)}>
        {#if batchRunning}<Loader2 size={14} class="spin" />{:else}<PlayCircle size={14} />{/if}
        {$t('pages.validation.validateAll')}
      </button>
    </div>
  </div>

  <div class="split">
    <aside class="card ds-list">
      <div class="ds-list-head">
        <h3>{datasets.length} {datasets.length === 1 ? $t('pages.validation.datasetSingular') : $t('pages.validation.datasetPlural')}</h3>
        {#if summary.noShapes > 0}<span class="hint">{$t('pages.validation.withoutShapes', { values: { count: summary.noShapes } })}</span>{/if}
      </div>
      <div class="ds-scroll">
        {#each datasets as ds}
          {@const status = datasetStatus[ds.id] || {}}
          {@const src = status.summary}
          {@const ownerName = ds.owner_type === 'organisation'
            ? (organisations.find(o => o.id === ds.owner_id)?.name ?? String(ds.owner_id))
            : (userMap[String(ds.owner_id)] ?? String(ds.owner_id))}
          {@const ownerHasImage = ds.owner_type === 'organisation'
            ? !!organisations.find(o => o.id === ds.owner_id)?.image_key
            : true}
          <div class="dataset-row" class:selected={selectedDataset === ds.id}
               on:click={() => selectDataset(ds.id)}
               on:keydown={(e) => e.key === 'Enter' && selectDataset(ds.id)}
               role="button" tabindex="0">
            <div class="row-main">
              <div class="row-title">
                <Database size={13} />
                <span class="ds-name" title={ds.name}>{ds.name}</span>
                <span class="owner-chip" title="{ds.owner_type === 'organisation' ? $t('pages.validation.ownerOrganisation') : $t('pages.validation.ownerUser')}: {ownerName}">
                  <Avatar kind={ds.owner_type === 'organisation' ? 'organisation' : 'user'} id={String(ds.owner_id)} name={ownerName} hasImage={ownerHasImage} size={16} />
                  <span class="owner-chip-name">{ownerName}</span>
                </span>
              </div>
              <div class="row-status">
                {#if status.loading}
                  <span class="pill pill-loading"><Loader2 size={11} class="spin" /> {$t('pages.validation.running')}</span>
                {:else if src}
                  {#if src.conforms}
                    <span class="pill pill-ok"><Check size={11} /> {$t('pages.validation.conforms')}</span>
                  {:else}
                    <span class="pill pill-fail"><FileWarning size={11} /> {$t('pages.validation.issueCount', { values: { count: src.results_count } })}</span>
                  {/if}
                {:else if status.error}
                  <span class="pill pill-error"><AlertTriangle size={11} /> {$t('system.error')}</span>
                {:else if !ds.shapes_graph_iri}
                  <span class="pill pill-muted">{$t('pages.validation.noShapes')}</span>
                  {#if accessibleShapeGraphs.length > 0}
                    <div class="inline-shapes-picker" on:click|stopPropagation role="presentation">
                      <Select
                        size="sm"
                        value=""
                        placeholder={$t('pages.validation.linkShapesPlaceholder')}
                        options={accessibleShapeGraphs.map((sg) => ({ value: sg.shapes_graph_iri, label: sg.dataset_name }))}
                        on:change={(e) => { if (e.detail) linkShapes(ds, e.detail); }}
                      />
                    </div>
                  {/if}
                {:else}
                  <span class="pill pill-muted">{$t('pages.validation.notRunPill')}</span>
                {/if}
                {#if status.ranAt && !status.loading}
                  <span class="ran-at" title={status.ranAt}><Clock size={10} /> {relativeTime(status.ranAt)}</span>
                {/if}
              </div>
            </div>
            <div class="row-actions">
              <button class="btn btn-sm btn-ghost" on:click|stopPropagation={() => runValidation(ds.id)}
                disabled={status.loading || !ds.shapes_graph_iri}
                title={!ds.shapes_graph_iri ? $t('pages.validation.configureShapesFirst') : $t('pages.validation.runValidation')}>
                {#if status.loading}<Loader2 size={12} class="spin" />{:else}<Play size={12} />{/if}
              </button>
              <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
              <label class="toggle" on:click|stopPropagation on:keydown|stopPropagation>
                <input type="checkbox" checked={ds.shacl_on_write} on:change|stopPropagation={() => toggleShaclOnWrite(ds)} />
                <span class="toggle-track"><span class="toggle-thumb"></span></span>
                <span class="toggle-text" title={$t('pages.validation.validateOnEveryWrite')}>{$t('pages.validation.onWriteToggle')}</span>
              </label>
            </div>
            {#if status.error}<p class="ds-error">{status.error}</p>{/if}
          </div>
        {/each}
      </div>
    </aside>

    <section class="card results-pane">
      {#if !selectedDs}
        <div class="placeholder">
          <ShieldCheck size={44} strokeWidth={1.1} />
          <h3>{$t('pages.validation.noDatasetSelected')}</h3>
          <p>{$t('pages.validation.noDatasetSelectedDesc')}</p>
        </div>
      {:else}
        <div class="pane-tabs">
          <div class="tab-row">
            <button class="tab" class:active={activeTab === 'results'} on:click={() => setTab('results')}><ShieldCheck size={14} /> {$t('pages.validation.tabResults')}</button>
            <button class="tab" class:active={activeTab === 'shapes'} on:click={() => setTab('shapes')}><FileCode size={14} /> {$t('pages.validation.tabShapes')}</button>
            <button class="tab" class:active={activeTab === 'history'} on:click={() => setTab('history')}><History size={14} /> {$t('pages.validation.tabHistory')}</button>
          </div>
          <button class="btn btn-sm" on:click={() => runValidation(selectedDataset)} disabled={activeStatus?.loading || !selectedDs.shapes_graph_iri}>
            {#if activeStatus?.loading}<Loader2 size={13} class="spin" />{:else}<Play size={13} />{/if} {$t('pages.validation.runValidation')}
          </button>
        </div>

        {#if activeTab === 'results'}
          {#if activeStatus?.loading}
            <div class="placeholder"><Loader2 size={32} class="spin" /><p>{$t('pages.validation.runningValidation')}</p></div>
          {:else if activeStatus?.error}
            <div class="results-banner err">
              <AlertTriangle size={18} />
              <div><strong>{$t('pages.validation.validationError')}</strong><small>{activeStatus.error}</small></div>
            </div>
          {:else if activeResult}
            <div class="results-banner" class:ok={activeResult.conforms}>
              {#if activeResult.conforms}
                <Check size={18} />
                <div><strong>{$t('pages.validation.dataConforms')}</strong><small>{$t('pages.validation.conformsBannerDesc', { values: { name: selectedDs.name } })}</small></div>
              {:else}
                <FileWarning size={18} />
                <div>
                  <strong>{$t('pages.validation.issuesFoundBanner', { values: { count: activeResult.results_count } })}</strong>
                  <small>{selectedDs.name}{viewingRunId ? $t('pages.validation.viewingPastRun') : ''}{activeStatus?.ranAt ? ` · ${relativeTime(activeStatus.ranAt)}` : ''}</small>
                </div>
              {/if}
            </div>
            {#if !activeResult.conforms}
              <IssueResults results={activeResult.results} datasetName={selectedDs.name} />
            {/if}
          {:else}
            <div class="placeholder">
              <ShieldCheck size={40} strokeWidth={1.1} />
              <h3>{$t('pages.validation.notValidatedYet')}</h3>
              {#if selectedDs.shapes_graph_iri}
                <p>{$t('pages.validation.notValidatedDesc', { values: { name: selectedDs.name } })}</p>
                <button class="btn" on:click={() => runValidation(selectedDataset)}><Play size={14} /> {$t('pages.validation.runValidation')}</button>
              {:else}
                <p>{$t('pages.validation.noShapesConfiguredDesc', { values: { name: selectedDs.name } })}</p>
                <button class="btn" on:click={() => setTab('shapes')}><FileCode size={14} /> {$t('pages.validation.editShapes')}</button>
              {/if}
            </div>
          {/if}
        {:else if activeTab === 'shapes'}
          <div class="shapes-wrap">
            <ShapesEditor datasetId={selectedDataset} height="calc(100vh - 360px)" />
          </div>
        {:else if activeTab === 'history'}
          <div class="history-wrap">
            {#if historyLoading}
              <div class="placeholder"><Loader2 size={28} class="spin" /><p>{$t('pages.validation.loadingHistory')}</p></div>
            {:else if historyRuns.length === 0}
              <div class="placeholder"><History size={36} strokeWidth={1.1} /><h3>{$t('pages.validation.noRunsYet')}</h3><p>{$t('pages.validation.noRunsYetDesc', { values: { name: selectedDs.name } })}</p></div>
            {:else}
              <ul class="history-list">
                {#each historyRuns as run}
                  <li class="history-row" class:current={viewingRunId === run.id}>
                    <button class="history-main" on:click={() => viewRun(selectedDataset, run.id)}>
                      {#if run.conforms}
                        <span class="pill pill-ok"><Check size={11} /> {$t('pages.validation.conforms')}</span>
                      {:else}
                        <span class="pill pill-fail"><FileWarning size={11} /> {$t('pages.validation.issueCount', { values: { count: run.results_count } })}</span>
                      {/if}
                      <span class="history-counts">
                        {#if run.violation_count}<span class="hc hc-v">{$t('pages.validation.violationCount', { values: { count: run.violation_count } })}</span>{/if}
                        {#if run.warning_count}<span class="hc hc-w">{$t('pages.validation.warningCount', { values: { count: run.warning_count } })}</span>{/if}
                        {#if run.info_count}<span class="hc hc-i">{$t('pages.validation.infoCount', { values: { count: run.info_count } })}</span>{/if}
                      </span>
                      <span class="history-time" title={run.run_timestamp}><Clock size={11} /> {relativeTime(run.run_timestamp)}</span>
                    </button>
                  </li>
                {/each}
              </ul>
            {/if}
          </div>
        {/if}
      {/if}
    </section>
  </div>
  {/if}
</div>

<style>
  .validation-page { display: flex; flex-direction: column; gap: 1rem; }

  .auth-gate { display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 1rem; padding: 4rem 2rem; text-align: center; background: var(--surface); border-radius: var(--radius); border: 1px solid var(--border); }
  .auth-gate h2 { font-size: 1.3rem; font-weight: 600; margin: 0; }
  .auth-gate p { color: var(--ink-600); max-width: 30rem; margin: 0; }
  :global(.auth-gate-icon) { color: var(--ink-400); }
  .owner-chip { display: inline-flex; align-items: center; gap: 0.3rem; margin-left: 0.4rem; font-size: 0.72rem; color: var(--ink-500); background: #f1f5f9; padding: 1px 6px 1px 2px; border-radius: 10px; }
  .owner-chip-name { max-width: 8rem; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }

  .error { color: #dc2626; background: #fef2f2; border: 1px solid #fecaca; padding: 0.6rem 0.8rem; border-radius: 10px; font-size: 0.85rem; }

  .summary { display: grid; grid-template-columns: repeat(5, minmax(120px, 1fr)) auto; gap: 0.75rem; align-items: center; padding: 0.9rem 1.1rem !important; }
  .summary-metric { display: flex; align-items: center; gap: 0.55rem; padding: 0.35rem 0.25rem; }
  .metric-icon { display: grid; place-items: center; width: 34px; height: 34px; border-radius: 10px; background: #f1f5f9; color: #475569; }
  .metric-value { font-size: 1.25rem; font-weight: 700; color: #1e293b; line-height: 1.1; }
  .metric-label { font-size: 0.7rem; color: #94a3b8; text-transform: uppercase; letter-spacing: 0.08em; }
  .metric-ok .metric-icon { background: #dcfce7; color: #15803d; }
  .metric-violation .metric-icon { background: #fee2e2; color: #b91c1c; }
  .metric-warning .metric-icon { background: #fef3c7; color: #b45309; }
  .metric-info .metric-icon { background: #dbeafe; color: #1d4ed8; }
  .summary-primary .metric-icon { background: linear-gradient(135deg, #7ED6D0, #2F7A8C); color: white; }
  .summary-actions { justify-self: end; }

  .split { display: grid; grid-template-columns: minmax(280px, 320px) minmax(0, 1fr); gap: 1rem; align-items: start; }

  .ds-list { padding: 0.85rem !important; display: flex; flex-direction: column; gap: 0.6rem; max-height: calc(100vh - 18rem); }
  .ds-list-head { display: flex; justify-content: space-between; align-items: baseline; }
  .ds-list h3 { margin: 0; font-size: 0.78rem; font-weight: 700; text-transform: uppercase; letter-spacing: 0.08em; color: #64748b; }
  .hint { font-size: 0.72rem; color: #94a3b8; }
  .ds-scroll { display: flex; flex-direction: column; gap: 0.4rem; overflow: auto; }

  .dataset-row { border: 1px solid var(--line-soft); border-radius: 10px; padding: 0.55rem 0.7rem; background: #fff; cursor: pointer; transition: background 0.12s, border-color 0.12s; }
  .dataset-row:hover { border-color: #7ED6D0; background: #f0fdfa; }
  .dataset-row.selected { border-color: #2F7A8C; background: linear-gradient(135deg, rgba(126,214,208,0.15), rgba(255,255,255,0.6)); box-shadow: inset 0 0 0 1px rgba(47,122,140,0.25); }

  .row-main { display: flex; align-items: center; justify-content: space-between; gap: 0.5rem; margin-bottom: 0.35rem; }
  .row-title { display: flex; align-items: center; gap: 0.35rem; min-width: 0; color: #475569; }
  .ds-name { font-weight: 600; font-size: 0.88rem; color: #1e293b; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .row-status { display: flex; align-items: center; gap: 0.35rem; flex-wrap: wrap; }
  .ran-at { display: inline-flex; align-items: center; gap: 0.2rem; font-size: 0.68rem; color: #94a3b8; }

  .pill { display: inline-flex; align-items: center; gap: 3px; font-size: 0.68rem; padding: 2px 7px; border-radius: 999px; font-weight: 600; white-space: nowrap; }
  .pill-ok { background: #dcfce7; color: #15803d; }
  .pill-fail { background: #fee2e2; color: #b91c1c; }
  .pill-loading { background: #dbeafe; color: #1d4ed8; }
  .pill-error { background: #fef3c7; color: #92400e; }
  .pill-muted { background: #f1f5f9; color: #64748b; }

  .row-actions { display: flex; align-items: center; gap: 0.35rem; }
  .ds-error { color: #b91c1c; font-size: 0.75rem; margin: 0.3rem 0 0; }

  .toggle { position: relative; display: inline-flex; align-items: center; gap: 0.3rem; cursor: pointer; user-select: none; }
  .toggle input { position: absolute; opacity: 0; width: 0; height: 0; }
  .toggle-track { width: 28px; height: 16px; background: #cbd5e1; border-radius: 999px; transition: background 0.2s; position: relative; flex-shrink: 0; }
  .toggle input:checked + .toggle-track { background: #2F7A8C; }
  .toggle-thumb { position: absolute; top: 2px; left: 2px; width: 12px; height: 12px; background: #fff; border-radius: 50%; transition: transform 0.2s; box-shadow: 0 1px 2px rgba(0,0,0,0.2); }
  .toggle input:checked + .toggle-track .toggle-thumb { transform: translateX(12px); }
  .toggle-text { font-size: 0.7rem; color: #64748b; }

  .results-pane { padding: 0 !important; overflow: hidden; display: flex; flex-direction: column; }
  .placeholder { display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 0.6rem; padding: 3rem 1.5rem; color: #64748b; text-align: center; }
  .placeholder h3 { margin: 0; }
  .placeholder p { margin: 0; max-width: 380px; font-size: 0.88rem; color: #94a3b8; }

  .pane-tabs { display: flex; align-items: center; justify-content: space-between; gap: 0.5rem; padding: 0.6rem 1.1rem; border-bottom: 1px solid var(--line-soft); }
  .tab-row { display: flex; gap: 0.25rem; }
  .tab { display: inline-flex; align-items: center; gap: 0.35rem; padding: 0.4rem 0.8rem; border: none; background: transparent; border-radius: 8px; cursor: pointer; font-size: 0.85rem; color: #64748b; font-weight: 600; }
  .tab:hover { background: #f1f5f9; color: #334155; }
  .tab.active { background: #ecfeff; color: #0e7490; }

  .results-banner { display: flex; align-items: center; gap: 0.75rem; padding: 0.9rem 1.1rem; background: linear-gradient(90deg, #fef2f2, #ffffff); border-bottom: 1px solid var(--line-soft); color: #991b1b; }
  .results-banner.ok { background: linear-gradient(90deg, #ecfdf5, #ffffff); color: #065f46; }
  .results-banner.err { background: linear-gradient(90deg, #fffbeb, #ffffff); color: #92400e; }
  .results-banner > div:first-of-type { flex: 1; min-width: 0; display: flex; flex-direction: column; }
  .results-banner strong { display: block; font-size: 0.95rem; }
  .results-banner small { color: inherit; opacity: 0.75; font-size: 0.78rem; }

  .shapes-wrap, .history-wrap { padding: 1rem 1.1rem; }

  .history-list { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 0.4rem; }
  .history-row { border: 1px solid var(--line-soft); border-radius: 10px; background: #fff; }
  .history-row.current { border-color: #2F7A8C; box-shadow: inset 0 0 0 1px rgba(47,122,140,0.2); }
  .history-main { display: flex; align-items: center; gap: 0.6rem; width: 100%; text-align: left; background: transparent; border: none; cursor: pointer; padding: 0.6rem 0.8rem; }
  .history-main:hover { background: #f8fafc; }
  .history-counts { display: flex; gap: 0.4rem; flex-wrap: wrap; flex: 1; }
  .hc { font-size: 0.72rem; padding: 1px 7px; border-radius: 999px; }
  .hc-v { background: #fee2e2; color: #991b1b; }
  .hc-w { background: #fef3c7; color: #92400e; }
  .hc-i { background: #dbeafe; color: #1e40af; }
  .history-time { display: inline-flex; align-items: center; gap: 0.25rem; font-size: 0.74rem; color: #94a3b8; }

  .inline-shapes-picker { margin-left: 0.25rem; display: inline-flex; }

  .btn-ghost { background: transparent; color: #2F7A8C; border: 1px solid var(--line-soft); }
  .btn-ghost:hover { background: #f0fdfa; border-color: #7ED6D0; }
  :global(.spin) { animation: spin 0.9s linear infinite; }
  @keyframes spin { to { transform: rotate(360deg); } }

  @media (max-width: 960px) {
    .summary { grid-template-columns: repeat(2, 1fr); }
    .summary-actions { grid-column: span 2; justify-self: stretch; }
    .summary-actions .btn { width: 100%; }
    .split { grid-template-columns: 1fr; }
    .ds-list { max-height: none; }
  }

  /* ---- Dark mode overrides (scoped rules out-specify global theme.css) ---- */
  :global(:is([data-theme="dark"], .dark)) .owner-chip { background: rgba(255,255,255,0.06); color: var(--ink-500); }
  :global(:is([data-theme="dark"], .dark)) .error { color: #fca5a5; background: rgba(220,38,38,0.12); border-color: rgba(220,38,38,0.35); }

  :global(:is([data-theme="dark"], .dark)) .metric-icon { background: rgba(255,255,255,0.06); color: var(--ink-600); }
  :global(:is([data-theme="dark"], .dark)) .metric-value { color: var(--ink-900); }
  :global(:is([data-theme="dark"], .dark)) .metric-label { color: var(--ink-500); }
  :global(:is([data-theme="dark"], .dark)) .metric-ok .metric-icon { background: rgba(16,185,129,0.18); color: #6ee7b7; }
  :global(:is([data-theme="dark"], .dark)) .metric-violation .metric-icon { background: rgba(239,68,68,0.18); color: #fca5a5; }
  :global(:is([data-theme="dark"], .dark)) .metric-warning .metric-icon { background: rgba(245,158,11,0.18); color: #fcd34d; }
  :global(:is([data-theme="dark"], .dark)) .metric-info .metric-icon { background: rgba(59,130,246,0.2); color: #93c5fd; }

  :global(:is([data-theme="dark"], .dark)) .ds-list h3 { color: var(--ink-500); }
  :global(:is([data-theme="dark"], .dark)) .hint { color: var(--ink-500); }
  :global(:is([data-theme="dark"], .dark)) .dataset-row { background: var(--bg-soft); }
  :global(:is([data-theme="dark"], .dark)) .dataset-row:hover { border-color: var(--brand-300); background: var(--brand-100); }
  :global(:is([data-theme="dark"], .dark)) .dataset-row.selected { border-color: var(--brand-300); background: var(--bg-accent-soft); box-shadow: inset 0 0 0 1px var(--brand-200); }
  :global(:is([data-theme="dark"], .dark)) .row-title { color: var(--ink-700); }
  :global(:is([data-theme="dark"], .dark)) .ds-name { color: var(--ink-900); }
  :global(:is([data-theme="dark"], .dark)) .ran-at { color: var(--ink-500); }

  :global(:is([data-theme="dark"], .dark)) .pill-ok { background: rgba(16,185,129,0.18); color: #6ee7b7; }
  :global(:is([data-theme="dark"], .dark)) .pill-fail { background: rgba(239,68,68,0.18); color: #fca5a5; }
  :global(:is([data-theme="dark"], .dark)) .pill-loading { background: rgba(59,130,246,0.2); color: #93c5fd; }
  :global(:is([data-theme="dark"], .dark)) .pill-error { background: rgba(245,158,11,0.18); color: #fcd34d; }
  :global(:is([data-theme="dark"], .dark)) .pill-muted { background: rgba(255,255,255,0.06); color: var(--ink-500); }
  :global(:is([data-theme="dark"], .dark)) .ds-error { color: #fca5a5; }

  :global(:is([data-theme="dark"], .dark)) .toggle-track { background: var(--line-strong); }
  :global(:is([data-theme="dark"], .dark)) .toggle-text { color: var(--ink-500); }

  :global(:is([data-theme="dark"], .dark)) .placeholder { color: var(--ink-500); }
  :global(:is([data-theme="dark"], .dark)) .placeholder p { color: var(--ink-600); }

  :global(:is([data-theme="dark"], .dark)) .tab { color: var(--ink-500); }
  :global(:is([data-theme="dark"], .dark)) .tab:hover { background: rgba(255,255,255,0.06); color: var(--ink-800); }
  :global(:is([data-theme="dark"], .dark)) .tab.active { background: var(--brand-100); color: var(--brand-700); }

  :global(:is([data-theme="dark"], .dark)) .results-banner { background: linear-gradient(90deg, rgba(220,38,38,0.14), transparent); color: #fca5a5; }
  :global(:is([data-theme="dark"], .dark)) .results-banner.ok { background: linear-gradient(90deg, rgba(16,185,129,0.14), transparent); color: #6ee7b7; }
  :global(:is([data-theme="dark"], .dark)) .results-banner.err { background: linear-gradient(90deg, rgba(245,158,11,0.14), transparent); color: #fcd34d; }

  :global(:is([data-theme="dark"], .dark)) .history-row { background: var(--bg-soft); }
  :global(:is([data-theme="dark"], .dark)) .history-main:hover { background: rgba(255,255,255,0.04); }
  :global(:is([data-theme="dark"], .dark)) .hc-v { background: rgba(239,68,68,0.18); color: #fca5a5; }
  :global(:is([data-theme="dark"], .dark)) .hc-w { background: rgba(245,158,11,0.18); color: #fcd34d; }
  :global(:is([data-theme="dark"], .dark)) .hc-i { background: rgba(59,130,246,0.2); color: #93c5fd; }
  :global(:is([data-theme="dark"], .dark)) .history-time { color: var(--ink-500); }

  :global(:is([data-theme="dark"], .dark)) .btn-ghost { color: var(--brand-700); }
  :global(:is([data-theme="dark"], .dark)) .btn-ghost:hover { background: var(--brand-100); border-color: var(--brand-300); }
</style>
