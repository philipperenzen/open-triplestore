<script>
  // Validation pipeline editor (create + edit).
  //
  // The schedule is built via a friendly preset picker (Manual / Hourly /
  // Daily / Weekly / Custom cron) that emits the standard 5-field cron string
  // the backend evaluates.
  //
  // The "Gate writes" toggle is loud on purpose: enabling it means writes that
  // violate this pipeline's threshold are *rejected* (HTTP 422 + report).
  import { onMount } from 'svelte';
  import { t as i18nT } from 'svelte-i18n';
  import {
    getPipeline, createPipeline, updatePipeline, deletePipeline, runPipeline,
    listShapeGraphs, listDatasets, browseGraphs,
  } from '../lib/api.js';
  import {
    Workflow, ArrowLeft, Save, Trash2, Play, Loader2, ShieldCheck, Database, FileCode,
    Calendar, Zap, AlertTriangle, GitMerge, Network, FlaskConical,
  } from 'lucide-svelte';
  import { Link, navigate } from '../lib/router/index.js';
  import { shortenIRI } from '../lib/rdf-utils.js';
  import ShaclStudioNav from '../components/ShaclStudioNav.svelte';
  import { isAuthenticated, authInitialized } from '../lib/stores.js';
  import { toastError, toastSuccess } from '../lib/toast.ts';
  import Select from '../components/Select.svelte';

  /** Pipeline id when editing; empty string when creating. */
  export let id = '';

  let isNew = !id || id === 'new';
  let loading = true;
  let saving = false;
  let running = false;

  let name = '';
  let description = '';
  let visibility = 'private';
  let ownerType = 'user';
  let ownerId = '';

  // What to validate — the pipeline's targets, split by kind for fast toggling.
  // On save these collapse into a single `targets: [{kind,id}]` array (and are
  // mirrored into the legacy dataset_ids/graph_iris for back-compat consumers).
  let datasetTargetIds = new Set();   // kind: dataset  (id = dataset id)
  let graphTargetIris = new Set();    // kind: graph    (id = graph IRI)
  let metaShapeGraphIds = new Set();    // kind: shapegraph (id = shape-graph id) — meta-validate the shapes themselves
  let targetClasses = '';             // comma separated; advanced, informational
  let shapeGraphIds = new Set();        // the shape graphs used AS validators

  let graphFilter = '';

  let severityThreshold = 'violation';
  let runInference = false;
  let maxResults = '';

  let triggerOnWrite = false;
  let gateWrites = false;

  let scheduleMode = 'manual';       // manual | hourly | daily | weekly | custom
  let scheduleHour = 9;
  let scheduleMinute = 0;
  let scheduleDay = 1;               // 0=Sun..6=Sat
  let scheduleCustom = '';

  let retention = 50;
  let inferredTarget = 'in_place';
  let inferredTargetGraph = '';
  let resultsTarget = 'none';
  let resultsTargetGraph = '';

  // Picker data
  let allShapeGraphs = [];
  let allDatasets = [];
  let allGraphs = [];

  $: filteredGraphs = graphFilter.trim()
    ? allGraphs.filter((g) => (g.iri + ' ' + g.name).toLowerCase().includes(graphFilter.trim().toLowerCase()))
    : allGraphs;

  let _guardChecked = false;
  $: if ($authInitialized && !_guardChecked) {
    _guardChecked = true;
    if (!$isAuthenticated) navigate('/login');
  }

  onMount(async () => {
    try {
      const [shapeGraphs, datasets, graphs] = await Promise.all([
        listShapeGraphs().catch(() => []),
        listDatasets().catch(() => []),
        browseGraphs().catch(() => []),
      ]);
      allShapeGraphs = shapeGraphs || [];
      allDatasets = datasets || [];
      allGraphs = (Array.isArray(graphs) ? graphs : (graphs?.graphs || []))
        .map((e) => e.iri ?? e.graph ?? e.graph_iri ?? null)
        .filter(Boolean)
        .map((iri) => ({ iri, name: shortenIRI(iri) }));
      if (!isNew) {
        const p = await getPipeline(id);
        name = p.name;
        description = p.description || '';
        visibility = p.visibility;
        ownerType = p.owner_type;
        ownerId = p.owner_id;
        // Seed target sets from both the new `targets` array and the legacy
        // dataset_ids/graph_iris (Sets dedupe, so mirroring stays idempotent).
        const tg = p.targets || [];
        datasetTargetIds = new Set([
          ...(p.dataset_ids || []),
          ...tg.filter((t) => t.kind === 'dataset').map((t) => t.id),
        ]);
        graphTargetIris = new Set([
          ...(p.graph_iris || []),
          ...tg.filter((t) => t.kind === 'graph').map((t) => t.id),
        ]);
        metaShapeGraphIds = new Set(tg.filter((t) => t.kind === 'shapegraph').map((t) => t.id));
        targetClasses = (p.target_classes || []).join(', ');
        shapeGraphIds = new Set(p.shape_graph_ids || []);
        severityThreshold = p.severity_threshold;
        runInference = p.run_inference;
        maxResults = p.max_results == null ? '' : String(p.max_results);
        triggerOnWrite = p.trigger_on_write;
        gateWrites = p.gate_writes;
        retention = p.retention || 50;
        inferredTarget = p.inferred_target || 'in_place';
        inferredTargetGraph = p.inferred_target_graph || '';
        resultsTarget = p.results_target || 'none';
        resultsTargetGraph = p.results_target_graph || '';
        decomposeCron(p.schedule_cron);
      }
    } catch (e) {
      toastError(e.message);
    } finally {
      loading = false;
    }
  });

  function decomposeCron(cron) {
    if (!cron) { scheduleMode = 'manual'; return; }
    const f = cron.split(/\s+/);
    if (f.length !== 5) { scheduleMode = 'custom'; scheduleCustom = cron; return; }
    const [mi, hr, dom, mo, dow] = f;
    if (mi === '0' && hr === '*' && dom === '*' && mo === '*' && dow === '*') { scheduleMode = 'hourly'; return; }
    if (/^\d+$/.test(mi) && /^\d+$/.test(hr) && dom === '*' && mo === '*' && dow === '*') {
      scheduleMode = 'daily'; scheduleHour = +hr; scheduleMinute = +mi; return;
    }
    if (/^\d+$/.test(mi) && /^\d+$/.test(hr) && dom === '*' && mo === '*' && /^\d+$/.test(dow)) {
      scheduleMode = 'weekly'; scheduleHour = +hr; scheduleMinute = +mi; scheduleDay = +dow; return;
    }
    scheduleMode = 'custom'; scheduleCustom = cron;
  }

  function buildCron() {
    switch (scheduleMode) {
      case 'manual': return null;
      case 'hourly': return '0 * * * *';
      case 'daily':  return `${scheduleMinute} ${scheduleHour} * * *`;
      case 'weekly': return `${scheduleMinute} ${scheduleHour} * * ${scheduleDay}`;
      case 'custom': return (scheduleCustom || '').trim() || null;
    }
    return null;
  }

  $: cronPreview = buildCron();
  $: friendlyCron = describeSchedule(scheduleMode, scheduleHour, scheduleMinute, scheduleDay, scheduleCustom);

  $: scopeSummary = (() => {
    const parts = [];
    if (datasetTargetIds.size) parts.push($i18nT('pages.pipelineEditor.scopeDatasets', { values: { count: datasetTargetIds.size } }));
    if (graphTargetIris.size) parts.push($i18nT('pages.pipelineEditor.scopeGraphs', { values: { count: graphTargetIris.size } }));
    if (metaShapeGraphIds.size) parts.push($i18nT('pages.pipelineEditor.scopeMeta', { values: { count: metaShapeGraphIds.size } }));
    return parts.length ? parts.join(' · ') : $i18nT('pages.pipelineEditor.scopeNothing');
  })();

  function describeSchedule(mode, h, m, d, custom) {
    const days = [
      $i18nT('pages.pipelineEditor.daySunday'), $i18nT('pages.pipelineEditor.dayMonday'),
      $i18nT('pages.pipelineEditor.dayTuesday'), $i18nT('pages.pipelineEditor.dayWednesday'),
      $i18nT('pages.pipelineEditor.dayThursday'), $i18nT('pages.pipelineEditor.dayFriday'),
      $i18nT('pages.pipelineEditor.daySaturday'),
    ];
    const pad = (n) => String(n).padStart(2, '0');
    switch (mode) {
      case 'manual': return $i18nT('pages.pipelineEditor.scheduleManualDesc');
      case 'hourly': return $i18nT('pages.pipelineEditor.scheduleHourlyDesc');
      case 'daily': return $i18nT('pages.pipelineEditor.scheduleDailyDesc', { values: { time: `${pad(h)}:${pad(m)}` } });
      case 'weekly': return $i18nT('pages.pipelineEditor.scheduleWeeklyDesc', { values: { day: days[d] || '?', time: `${pad(h)}:${pad(m)}` } });
      case 'custom': return custom ? $i18nT('pages.pipelineEditor.scheduleCustomCron', { values: { cron: custom } }) : $i18nT('pages.pipelineEditor.scheduleCustomDesc');
    }
    return '';
  }

  function toggle(set, value) { if (set.has(value)) set.delete(value); else set.add(value); return new Set(set); }

  function parseCsvLines(s) {
    return s.split(/[\n,]/).map((x) => x.trim()).filter(Boolean);
  }

  $: targets = [
    ...[...datasetTargetIds].map((tid) => ({ kind: 'dataset', id: tid })),
    ...[...graphTargetIris].map((tid) => ({ kind: 'graph', id: tid })),
    ...[...metaShapeGraphIds].map((tid) => ({ kind: 'shapegraph', id: tid })),
  ];

  async function save() {
    if (!name.trim()) { toastError($i18nT('pages.pipelineEditor.errorNameRequired')); return; }
    if (!targets.length) { toastError($i18nT('pages.pipelineEditor.errorNoTargets')); return; }
    // Explicit shape graphs aren't required when meta-validating shapes (SHACL-SHACL
    // is supplied automatically) or when relying on shapes bound to the targets.
    if (!shapeGraphIds.size && !metaShapeGraphIds.size && !datasetTargetIds.size && !graphTargetIris.size) {
      toastError($i18nT('pages.pipelineEditor.errorNoShapeGraphs')); return;
    }
    saving = true;
    try {
      const body = {
        name: name.trim(),
        description: description.trim() || undefined,
        visibility,
        owner_type: ownerType,
        owner_id: ownerType === 'organisation' ? ownerId : undefined,
        targets,
        // Mirror Dataset/Graph targets into the legacy fields so older readers
        // and the write-gate's legacy path keep resolving the same scope.
        dataset_ids: [...datasetTargetIds],
        graph_iris: [...graphTargetIris],
        target_classes: parseCsvLines(targetClasses),
        shape_graph_ids: [...shapeGraphIds],
        severity_threshold: severityThreshold,
        run_inference: runInference,
        max_results: maxResults === '' ? null : parseInt(maxResults, 10),
        trigger_on_write: triggerOnWrite,
        schedule_cron: cronPreview,
        gate_writes: gateWrites,
        retention,
        inferred_target: inferredTarget,
        inferred_target_graph: inferredTargetGraph || null,
        results_target: resultsTarget,
        results_target_graph: resultsTargetGraph || null,
      };
      const result = isNew ? await createPipeline(body) : await updatePipeline(id, body);
      toastSuccess(isNew ? $i18nT('pages.pipelineEditor.toastCreated') : $i18nT('pages.pipelineEditor.toastSaved'));
      navigate(`/shacl/pipelines/${result.id}`);
    } catch (e) {
      toastError(e.message);
    } finally {
      saving = false;
    }
  }

  async function runNow() {
    if (isNew) return;
    running = true;
    try {
      const run = await runPipeline(id);
      toastSuccess(run.conforms ? $i18nT('pages.pipelineEditor.toastValidationPassed') : $i18nT('pages.pipelineEditor.toastViolationsFound', { values: { count: run.violation_count } }));
    } catch (e) {
      toastError(e.message);
    } finally {
      running = false;
    }
  }

  async function doDelete() {
    if (isNew || !confirm($i18nT('pages.pipelineEditor.confirmDelete', { values: { name } }))) return;
    try {
      await deletePipeline(id);
      navigate('/shacl/pipelines');
    } catch (e) { toastError(e.message); }
  }

  function gateConfirm() {
    if (gateWrites) return; // turning OFF needs no confirmation
    if (!confirm($i18nT('pages.pipelineEditor.confirmGateWrites'))) {
      return;
    }
  }
</script>

<div class="edit-page">
  <ShaclStudioNav />

  {#if loading}
    <div class="card placeholder"><Loader2 size={24} class="spin" /><p>{$i18nT('pages.pipelineEditor.loadingPipeline')}</p></div>
  {:else}
    <div class="card header-card">
      <Link to="/shacl/pipelines" class="back"><ArrowLeft size={13} /> {$i18nT('pages.pipelineEditor.pipelinesLink')}</Link>
      <div class="header-row">
        <div class="header-main">
          <Workflow size={18} class="hicon" />
          <input class="name-input" placeholder={$i18nT('pages.pipelineEditor.namePlaceholder')} bind:value={name} />
        </div>
        <div class="header-actions">
          {#if !isNew}
            <button class="btn btn-sm btn-ghost" on:click={runNow} disabled={running}>
              {#if running}<Loader2 size={12} class="spin" />{:else}<Play size={12} />{/if} {$i18nT('pages.pipelineEditor.runNow')}
            </button>
            <button class="btn btn-sm btn-ghost icon-danger" on:click={doDelete}><Trash2 size={12} /> {$i18nT('system.delete')}</button>
          {/if}
          <button class="btn" on:click={save} disabled={saving}>
            {#if saving}<Loader2 size={13} class="spin" /> {$i18nT('pages.pipelineEditor.saving')}{:else}<Save size={13} /> {isNew ? $i18nT('pages.pipelineEditor.createPipeline') : $i18nT('system.save')}{/if}
          </button>
        </div>
      </div>
      <textarea class="desc-input" placeholder={$i18nT('pages.pipelineEditor.descriptionPlaceholder')} rows="2" bind:value={description}></textarea>
    </div>

    <div class="grid">
      <!-- What to validate (targets) -->
      <section class="card panel">
        <header class="panel-head"><Database size={14} /><h3>{$i18nT('pages.pipelineEditor.whatToValidate')}</h3></header>
        <p class="hint">{$i18nT('pages.pipelineEditor.whatToValidateHint')}</p>

        <div class="target-group">
          <span class="group-label"><Database size={11} /> {$i18nT('pages.pipelineEditor.datasets')}</span>
          <div class="picker">
            {#each allDatasets as ds}
              <label class="picker-row">
                <input type="checkbox" checked={datasetTargetIds.has(ds.id)} on:change={() => (datasetTargetIds = toggle(datasetTargetIds, ds.id))} />
                <Database size={11} />
                <span>{ds.name}</span>
                {#if ds.visibility !== 'public'}<span class="dim">{ds.visibility}</span>{/if}
              </label>
            {/each}
            {#if allDatasets.length === 0}<p class="empty">{$i18nT('pages.pipelineEditor.noDatasets')}</p>{/if}
          </div>
        </div>

        <div class="target-group">
          <span class="group-label"><Network size={11} /> {$i18nT('pages.pipelineEditor.namedGraphs')}</span>
          {#if allGraphs.length > 8}
            <input class="filter-input" placeholder={$i18nT('pages.pipelineEditor.filterGraphs')} bind:value={graphFilter} />
          {/if}
          <div class="picker">
            {#each filteredGraphs as g (g.iri)}
              <label class="picker-row">
                <input type="checkbox" checked={graphTargetIris.has(g.iri)} on:change={() => (graphTargetIris = toggle(graphTargetIris, g.iri))} />
                <Network size={11} />
                <span title={g.iri}>{g.name}</span>
              </label>
            {/each}
            {#if allGraphs.length === 0}<p class="empty">{$i18nT('pages.pipelineEditor.noNamedGraphs')}</p>{/if}
            {#if allGraphs.length > 0 && filteredGraphs.length === 0}<p class="empty">{$i18nT('pages.pipelineEditor.noGraphsMatchFilter')}</p>{/if}
          </div>
        </div>

        <div class="target-group">
          <span class="group-label"><FlaskConical size={11} /> {$i18nT('pages.pipelineEditor.shapeGraphs')} <span class="group-note">{$i18nT('pages.pipelineEditor.shapeGraphsMetaNote')}</span></span>
          <div class="picker">
            {#each allShapeGraphs as set}
              <label class="picker-row">
                <input type="checkbox" checked={metaShapeGraphIds.has(set.id)} on:change={() => (metaShapeGraphIds = toggle(metaShapeGraphIds, set.id))} />
                <FileCode size={11} />
                <span>{set.name}</span>
                <span class="dim">{$i18nT('pages.pipelineEditor.shapeCount', { values: { count: set.shape_count } })}</span>
              </label>
            {/each}
            {#if allShapeGraphs.length === 0}<p class="empty">{$i18nT('pages.pipelineEditor.noShapeGraphs')}</p>{/if}
          </div>
        </div>

        <details class="advanced">
          <summary>{$i18nT('pages.pipelineEditor.advancedTargetClasses')}</summary>
          <label>
            <span>{$i18nT('pages.pipelineEditor.targetClassesLabel')}</span>
            <input bind:value={targetClasses} placeholder="ex:Bridge, ex:Road" />
          </label>
        </details>
      </section>

      <!-- Validators -->
      <section class="card panel">
        <header class="panel-head"><FileCode size={14} /><h3>{$i18nT('pages.pipelineEditor.validateWith')}</h3></header>
        <p class="hint">{$i18nT('pages.pipelineEditor.validateWithHint')}</p>
        <div class="picker">
          {#each allShapeGraphs as set}
            <label class="picker-row">
              <input type="checkbox" checked={shapeGraphIds.has(set.id)} on:change={() => (shapeGraphIds = toggle(shapeGraphIds, set.id))} />
              <FileCode size={11} />
              <span>{set.name}</span>
              <span class="dim">{$i18nT('pages.pipelineEditor.shapeCount', { values: { count: set.shape_count } })}</span>
            </label>
          {/each}
          {#if allShapeGraphs.length === 0}
            <p class="empty">{$i18nT('pages.pipelineEditor.noShapeGraphsLibrary')} <Link to="/shacl/shapes">{$i18nT('pages.pipelineEditor.createOne')}</Link></p>
          {/if}
        </div>
      </section>

      <!-- Options -->
      <section class="card panel">
        <header class="panel-head"><ShieldCheck size={14} /><h3>{$i18nT('pages.pipelineEditor.options')}</h3></header>
        <label>
          <span>{$i18nT('pages.pipelineEditor.severityThreshold')}</span>
          <Select bind:value={severityThreshold} options={[
            { value: 'violation', label: $i18nT('pages.pipelineEditor.severityViolation') },
            { value: 'warning', label: $i18nT('pages.pipelineEditor.severityWarning') },
            { value: 'info', label: $i18nT('pages.pipelineEditor.severityInfo') },
          ]} />
        </label>
        <label class="check">
          <input type="checkbox" bind:checked={runInference} />
          <span><GitMerge size={11} /> {$i18nT('pages.pipelineEditor.runInference')}</span>
        </label>
        <label>
          <span>{$i18nT('pages.pipelineEditor.retention')}</span>
          <input type="number" min="1" max="500" bind:value={retention} />
        </label>
        <label>
          <span>{$i18nT('pages.pipelineEditor.inferredTriples')}</span>
          <Select bind:value={inferredTarget} options={[
            { value: 'in_place', label: $i18nT('pages.pipelineEditor.inferredInPlace') },
            { value: 'new_graph', label: $i18nT('pages.pipelineEditor.inferredNewGraph') },
            { value: 'new_version', label: $i18nT('pages.pipelineEditor.inferredNewVersion') },
          ]} />
        </label>
        {#if inferredTarget === 'new_graph'}
          <label>
            <span>{$i18nT('pages.pipelineEditor.inferredGraphIri')}</span>
            <input type="text" placeholder="urn:system:inferred:…" bind:value={inferredTargetGraph} />
          </label>
        {/if}
        <label>
          <span>{$i18nT('pages.pipelineEditor.validationResults')}</span>
          <Select bind:value={resultsTarget} options={[
            { value: 'none', label: $i18nT('pages.pipelineEditor.resultsNone') },
            { value: 'in_place', label: $i18nT('pages.pipelineEditor.resultsReportGraph') },
            { value: 'new_graph', label: $i18nT('pages.pipelineEditor.resultsNewGraph') },
            { value: 'new_version', label: $i18nT('pages.pipelineEditor.resultsNewVersion') },
          ]} />
        </label>
        {#if resultsTarget === 'new_graph'}
          <label>
            <span>{$i18nT('pages.pipelineEditor.resultsGraphIri')}</span>
            <input type="text" placeholder="urn:system:reports:…" bind:value={resultsTargetGraph} />
          </label>
        {/if}
        <label>
          <span>{$i18nT('pages.pipelineEditor.maxResults')}</span>
          <input type="number" min="1" bind:value={maxResults} placeholder="" />
        </label>
        <label>
          <span>{$i18nT('pages.pipelineEditor.visibility')}</span>
          <Select bind:value={visibility} options={[
            { value: 'private', label: $i18nT('pages.pipelineEditor.visibilityPrivate') },
            { value: 'members', label: $i18nT('pages.pipelineEditor.visibilityMembers') },
            { value: 'public', label: $i18nT('pages.pipelineEditor.visibilityPublic') },
          ]} />
        </label>
      </section>

      <!-- Triggers -->
      <section class="card panel">
        <header class="panel-head"><Calendar size={14} /><h3>{$i18nT('pages.pipelineEditor.triggers')}</h3></header>

        <label class="check">
          <input type="checkbox" bind:checked={triggerOnWrite} />
          <span><Zap size={11} /> {$i18nT('pages.pipelineEditor.runOnWrite')}</span>
        </label>

        <fieldset class="schedule">
          <legend>{$i18nT('pages.pipelineEditor.schedule')}</legend>
          <div class="radio-row">
            <label><input type="radio" value="manual" bind:group={scheduleMode} /> {$i18nT('pages.pipelineEditor.scheduleManual')}</label>
            <label><input type="radio" value="hourly" bind:group={scheduleMode} /> {$i18nT('pages.pipelineEditor.scheduleHourly')}</label>
            <label><input type="radio" value="daily" bind:group={scheduleMode} /> {$i18nT('pages.pipelineEditor.scheduleDaily')}</label>
            <label><input type="radio" value="weekly" bind:group={scheduleMode} /> {$i18nT('pages.pipelineEditor.scheduleWeekly')}</label>
            <label><input type="radio" value="custom" bind:group={scheduleMode} /> {$i18nT('pages.pipelineEditor.scheduleCustom')}</label>
          </div>
          {#if scheduleMode === 'daily' || scheduleMode === 'weekly'}
            <div class="schedule-pickers">
              {#if scheduleMode === 'weekly'}
                <label>
                  <span>{$i18nT('pages.pipelineEditor.day')}</span>
                  <Select bind:value={scheduleDay} options={[
                    { value: 0, label: $i18nT('pages.pipelineEditor.daySunday') }, { value: 1, label: $i18nT('pages.pipelineEditor.dayMonday') },
                    { value: 2, label: $i18nT('pages.pipelineEditor.dayTuesday') }, { value: 3, label: $i18nT('pages.pipelineEditor.dayWednesday') },
                    { value: 4, label: $i18nT('pages.pipelineEditor.dayThursday') }, { value: 5, label: $i18nT('pages.pipelineEditor.dayFriday') },
                    { value: 6, label: $i18nT('pages.pipelineEditor.daySaturday') },
                  ]} />
                </label>
              {/if}
              <label><span>{$i18nT('pages.pipelineEditor.hourUtc')}</span><input type="number" min="0" max="23" bind:value={scheduleHour} /></label>
              <label><span>{$i18nT('pages.pipelineEditor.minute')}</span><input type="number" min="0" max="59" bind:value={scheduleMinute} /></label>
            </div>
          {:else if scheduleMode === 'custom'}
            <label>
              <span>{$i18nT('pages.pipelineEditor.cronExpression')}</span>
              <input bind:value={scheduleCustom} placeholder="0 */6 * * *" />
            </label>
          {/if}
          <p class="schedule-preview">
            <Calendar size={11} /> {friendlyCron}
            {#if cronPreview}<code>{cronPreview}</code>{/if}
          </p>
        </fieldset>

        <label class="check gate-toggle" class:active={gateWrites}>
          <input type="checkbox" bind:checked={gateWrites} on:change={gateConfirm} />
          <span>
            <ShieldCheck size={11} /> <strong>{$i18nT('pages.pipelineEditor.gateWrites')}</strong> {$i18nT('pages.pipelineEditor.gateWritesDesc')}
          </span>
        </label>
        {#if gateWrites}
          <div class="gate-warn">
            <AlertTriangle size={14} />
            <div>
              <strong>{$i18nT('pages.pipelineEditor.gatingActive')}</strong> {$i18nT('pages.pipelineEditor.gatingActiveDescBefore', { values: { threshold: severityThreshold } })} <em>{$i18nT('pages.pipelineEditor.gatingRejected')}</em> {$i18nT('pages.pipelineEditor.gatingActiveDescAfter')}
            </div>
          </div>
        {/if}
      </section>
    </div>

    <!-- Summary -->
    <section class="card summary">
      <div class="summary-head">
        <h4>{$i18nT('pages.pipelineEditor.pipelineSummary')}</h4>
        {#if triggerOnWrite || gateWrites}
          <div class="summary-flags">
            {#if triggerOnWrite}<span class="chip chip-trigger"><Zap size={11} /> {$i18nT('pages.pipelineEditor.chipRunsOnWrite')}</span>{/if}
            {#if gateWrites}<span class="chip chip-gate"><ShieldCheck size={11} /> {$i18nT('pages.pipelineEditor.chipGatesWrites')}</span>{/if}
          </div>
        {/if}
      </div>
      <div class="summary-grid">
        <div class="stat">
          <span class="stat-ic ic-scope"><Database size={15} /></span>
          <div class="stat-body">
            <span class="stat-label">{$i18nT('pages.pipelineEditor.scope')}</span>
            <span class="stat-value" title={scopeSummary}>{scopeSummary}</span>
          </div>
        </div>
        <div class="stat">
          <span class="stat-ic ic-shapes"><FileCode size={15} /></span>
          <div class="stat-body">
            <span class="stat-label">{$i18nT('pages.pipelineEditor.shapeGraphs')}</span>
            <span class="stat-value">{$i18nT('pages.pipelineEditor.summaryShapeGraphCount', { values: { count: shapeGraphIds.size } })}</span>
          </div>
        </div>
        <div class="stat">
          <span class="stat-ic ic-threshold"><ShieldCheck size={15} /></span>
          <div class="stat-body">
            <span class="stat-label">{$i18nT('pages.pipelineEditor.threshold')}</span>
            <span class="stat-value">{severityThreshold}</span>
          </div>
        </div>
        <div class="stat">
          <span class="stat-ic ic-schedule"><Calendar size={15} /></span>
          <div class="stat-body">
            <span class="stat-label">{$i18nT('pages.pipelineEditor.schedule')}</span>
            <span class="stat-value" title={friendlyCron}>{friendlyCron}</span>
          </div>
        </div>
      </div>
    </section>
  {/if}
</div>

<style>
  .edit-page { display: flex; flex-direction: column; gap: 0.85rem; }
  .placeholder { display: flex; align-items: center; justify-content: center; gap: 0.5rem; padding: 3rem; color: #94a3b8; }

  .header-card { padding: 0.75rem 1rem !important; }
  :global(.edit-page .back) { display: inline-flex; align-items: center; gap: 0.3rem; font-size: 0.78rem; color: #2F7A8C; text-decoration: none; margin-bottom: 0.45rem; }
  :global(.edit-page .back:hover) { text-decoration: underline; }
  .header-row { display: flex; align-items: center; gap: 0.6rem; }
  .header-main { display: flex; align-items: center; gap: 0.5rem; flex: 1; min-width: 0; }
  :global(.hicon) { color: #6d28d9; flex-shrink: 0; }
  .name-input { flex: 1; padding: 0.4rem 0.5rem; border: 1px solid var(--line-soft); border-radius: 8px; font-size: 1.05rem; font-weight: 600; }
  .header-actions { display: flex; gap: 0.35rem; flex-shrink: 0; }
  .icon-danger { color: #b91c1c; }
  .desc-input { width: 100%; margin-top: 0.5rem; padding: 0.4rem 0.55rem; font-size: 0.88rem; border: 1px solid var(--line-soft); border-radius: 8px; font-family: inherit; resize: vertical; }

  .grid { display: grid; grid-template-columns: 1fr 1fr; gap: 0.85rem; align-items: start; }
  .panel { padding: 0.85rem 1rem !important; }
  .panel-head { display: flex; align-items: center; gap: 0.35rem; margin-bottom: 0.4rem; color: #475569; }
  .panel-head h3 { margin: 0; font-size: 0.9rem; font-weight: 700; color: #334155; }
  .hint { margin: 0 0 0.55rem; color: #64748b; font-size: 0.82rem; }
  .panel label { display: flex; flex-direction: column; gap: 0.2rem; font-size: 0.82rem; color: #475569; font-weight: 600; margin-top: 0.55rem; }
  .panel label.check { flex-direction: row; align-items: center; gap: 0.5rem; font-weight: 500; }
  .panel input[type="number"], .panel input:not([type]) { padding: 0.4rem 0.55rem; font-size: 0.86rem; border: 1px solid var(--line-soft); border-radius: 8px; font-family: inherit; }

  .picker { display: flex; flex-direction: column; gap: 0.2rem; max-height: 180px; overflow: auto; border: 1px solid var(--line-soft); border-radius: 8px; padding: 0.4rem; background: #fafbfc; }
  /* Scoped under .panel label to win over the column-layout .panel label rule. */
  .panel label.picker-row { display: flex; flex-direction: row; align-items: center; gap: 0.45rem; margin-top: 0; padding: 0.3rem 0.45rem; border-radius: 6px; cursor: pointer; font-weight: 500; }
  .picker-row:hover { background: #f1f5f9; }
  .picker-row input[type="checkbox"] { margin: 0; flex-shrink: 0; }
  .picker-row > :global(svg) { flex-shrink: 0; color: #64748b; }
  .picker-row > span:not(.dim) { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .empty { color: #94a3b8; font-size: 0.82rem; margin: 0.4rem; }
  .dim { color: #94a3b8; font-size: 0.74rem; margin-left: auto; }

  .target-group { margin-top: 0.55rem; }
  .target-group:first-of-type { margin-top: 0; }
  .group-label { display: flex; align-items: center; gap: 0.3rem; font-size: 0.72rem; font-weight: 700; text-transform: uppercase; letter-spacing: 0.04em; color: #64748b; margin-bottom: 0.25rem; }
  .group-label > :global(svg) { color: #94a3b8; }
  .group-note { text-transform: none; letter-spacing: 0; font-weight: 500; color: #94a3b8; }
  .filter-input { width: 100%; margin-bottom: 0.3rem; padding: 0.32rem 0.5rem; font-size: 0.82rem; border: 1px solid var(--line-soft); border-radius: 8px; font-family: inherit; }
  .target-group .picker { max-height: 140px; }

  .advanced { margin-top: 0.6rem; font-size: 0.85rem; }
  .advanced summary { cursor: pointer; color: #475569; font-weight: 600; }

  .schedule { border: 1px solid var(--line-soft); border-radius: 10px; padding: 0.5rem 0.6rem; margin-top: 0.5rem; }
  .schedule legend { font-size: 0.75rem; font-weight: 700; color: #64748b; padding: 0 0.3rem; }
  .radio-row { display: flex; flex-wrap: wrap; gap: 0.5rem 1rem; }
  .radio-row label { display: inline-flex; align-items: center; gap: 0.3rem; font-weight: 500; font-size: 0.82rem; flex-direction: row; margin-top: 0; }
  .schedule-pickers { display: flex; gap: 0.5rem; margin-top: 0.4rem; flex-wrap: wrap; }
  .schedule-pickers label { font-size: 0.75rem; }
  .schedule-pickers input { width: 5rem; }
  .schedule-preview { margin: 0.55rem 0 0; font-size: 0.8rem; color: #475569; display: flex; align-items: center; gap: 0.4rem; flex-wrap: wrap; }
  .schedule-preview code { background: #f1f5f9; padding: 1px 6px; border-radius: 4px; font-size: 0.8em; color: #334155; }

  .gate-toggle { margin-top: 0.7rem; padding: 0.4rem 0.55rem; border: 1px solid var(--line-soft); border-radius: 8px; background: #fafbfc; transition: background 0.12s, border-color 0.12s; }
  .gate-toggle.active { background: #fef2f2; border-color: #fecaca; }
  .gate-toggle strong { color: #b91c1c; }
  .gate-warn { display: flex; gap: 0.5rem; align-items: flex-start; padding: 0.6rem 0.7rem; background: #fef3c7; border: 1px solid #fde68a; border-radius: 8px; margin-top: 0.5rem; color: #92400e; font-size: 0.82rem; }
  .gate-warn strong { color: #92400e; }
  .gate-warn em { color: #b91c1c; font-style: normal; font-weight: 700; }

  .summary { padding: 0.85rem 1rem !important; }
  .summary-head { display: flex; align-items: center; justify-content: space-between; gap: 0.6rem; flex-wrap: wrap; margin-bottom: 0.7rem; }
  .summary h4 { margin: 0; font-size: 0.85rem; color: #334155; }
  .summary-flags { display: flex; gap: 0.4rem; flex-wrap: wrap; }

  .summary-grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(150px, 1fr)); gap: 0.6rem; }
  .stat { display: flex; align-items: center; gap: 0.55rem; padding: 0.5rem 0.6rem; border: 1px solid var(--line-soft); border-radius: 10px; background: #fafbfc; min-width: 0; }
  .stat-ic { display: grid; place-items: center; width: 30px; height: 30px; border-radius: 8px; flex-shrink: 0; }
  .ic-scope { background: #e0f2fe; color: #0369a1; }
  .ic-shapes { background: #ede9fe; color: #6d28d9; }
  .ic-threshold { background: #dcfce7; color: #15803d; }
  .ic-schedule { background: #fef3c7; color: #b45309; }
  .stat-body { display: flex; flex-direction: column; gap: 0.05rem; min-width: 0; }
  .stat-label { font-size: 0.66rem; text-transform: uppercase; letter-spacing: 0.05em; font-weight: 700; color: #94a3b8; }
  .stat-value { font-size: 0.86rem; font-weight: 600; color: #1e293b; text-transform: capitalize; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }

  .chip { display: inline-flex; align-items: center; gap: 0.25rem; font-size: 0.7rem; padding: 3px 9px; border-radius: 999px; background: #f1f5f9; color: #475569; font-weight: 600; }
  .chip-trigger { background: #ede9fe; color: #5b21b6; }
  .chip-gate { background: #fee2e2; color: #b91c1c; }

  @media (max-width: 880px) { .grid { grid-template-columns: 1fr; } }

  :global(:is([data-theme="dark"], .dark) .edit-page .back) { color: var(--brand-700); }
  :global(:is([data-theme="dark"], .dark) .hicon) { color: #c4b5fd; }
  :global(:is([data-theme="dark"], .dark)) .icon-danger { color: #fca5a5; }
  :global(:is([data-theme="dark"], .dark)) .panel-head h3 { color: var(--ink-800); }
  :global(:is([data-theme="dark"], .dark)) .picker { background: var(--bg-soft); }
  :global(:is([data-theme="dark"], .dark)) .picker-row:hover { background: rgba(255,255,255,0.06); }
  :global(:is([data-theme="dark"], .dark)) .group-label { color: var(--ink-400); }
  :global(:is([data-theme="dark"], .dark)) .group-note { color: var(--ink-300); }
  :global(:is([data-theme="dark"], .dark)) .schedule-preview code { background: rgba(255,255,255,0.06); color: var(--ink-800); }
  :global(:is([data-theme="dark"], .dark)) .gate-toggle { background: var(--bg-soft); }
  :global(:is([data-theme="dark"], .dark)) .gate-toggle.active { background: rgba(220,38,38,0.12); border-color: rgba(220,38,38,0.35); }
  :global(:is([data-theme="dark"], .dark)) .gate-toggle strong { color: #fca5a5; }
  :global(:is([data-theme="dark"], .dark)) .gate-warn { background: rgba(245,158,11,0.12); border-color: rgba(245,158,11,0.35); color: #fcd34d; }
  :global(:is([data-theme="dark"], .dark)) .gate-warn strong { color: #fcd34d; }
  :global(:is([data-theme="dark"], .dark)) .gate-warn em { color: #fca5a5; }
  :global(:is([data-theme="dark"], .dark)) .summary h4 { color: var(--ink-800); }
  :global(:is([data-theme="dark"], .dark)) .stat { background: var(--bg-soft); border-color: var(--line-strong); }
  :global(:is([data-theme="dark"], .dark)) .stat-value { color: var(--ink-900); }
  :global(:is([data-theme="dark"], .dark)) .stat-label { color: var(--ink-400); }
  :global(:is([data-theme="dark"], .dark)) .ic-scope { background: rgba(56,189,248,0.18); color: #7dd3fc; }
  :global(:is([data-theme="dark"], .dark)) .ic-shapes { background: rgba(139,92,246,0.2); color: #c4b5fd; }
  :global(:is([data-theme="dark"], .dark)) .ic-threshold { background: rgba(34,197,94,0.18); color: #86efac; }
  :global(:is([data-theme="dark"], .dark)) .ic-schedule { background: rgba(245,158,11,0.18); color: #fcd34d; }
  :global(:is([data-theme="dark"], .dark)) .chip { background: rgba(255,255,255,0.06); color: var(--ink-400); }
  :global(:is([data-theme="dark"], .dark)) .chip-trigger { background: rgba(139,92,246,0.2); color: #c4b5fd; }
  :global(:is([data-theme="dark"], .dark)) .chip-gate { background: rgba(239,68,68,0.18); color: #fca5a5; }
</style>
