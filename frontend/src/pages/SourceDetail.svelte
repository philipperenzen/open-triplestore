<script>
  // "Sources" workspace, steps 2 and 5 — Explore and Publish & Runs.
  //
  // Explore: the schema profile of the datasource, plus a raw first-rows
  // preview (pre-clean data, admin only).
  // Runs: trigger a run, read the history with triple counts, duration and
  // provenance, and roll back in one click. A run that the SHACL write gate
  // refused shows its report; production is untouched in that case.
  import { onMount } from 'svelte';
  import { t } from 'svelte-i18n';
  import {
    Database, Loader2, Play, Undo2, Trash2, Table2, KeyRound, Link2,
    AlertTriangle, Check, Lock, ShieldCheck, ShieldAlert, Sparkles, FileCode,
  } from 'lucide-svelte';
  import {
    getSource, introspectSource, previewSourceTable, listSourceMappings,
    listSourceRuns, startSourceRun, rollbackRun, deleteRun, getMappingRml,
  } from '../lib/api.js';
  import { Link, navigate } from '../lib/router/index.js';
  import { shortenIRI } from '../lib/rdf-utils.js';
  import { isAdmin, authInitialized } from '../lib/stores.js';
  import { toastError, toastSuccess } from '../lib/toast.ts';

  export let id;

  let source = null;
  let schema = null;
  let mappings = [];
  let runs = [];
  let loading = true;
  let tab = 'runs';
  let expandedTable = null;
  let preview = null;
  let previewing = false;
  let running = false;
  let gateReport = null;
  let selectedMapping = '';
  let rml = null;

  let _guardChecked = false;
  $: if ($authInitialized && !_guardChecked) {
    _guardChecked = true;
    if (!$isAdmin) navigate('/');
  }

  onMount(load);

  async function load() {
    loading = true;
    try {
      source = await getSource(id);
      mappings = await listSourceMappings(id).catch(() => []);
      runs = await listSourceRuns(id).catch(() => []);
      if (!selectedMapping && mappings.length) selectedMapping = mappings[0].id;
    } catch (e) {
      toastError(e.message);
    } finally {
      loading = false;
    }
  }

  async function loadSchema() {
    if (schema) return;
    try {
      schema = await introspectSource(id);
    } catch (e) {
      toastError(e.message);
      schema = { tables: [] };
    }
  }

  async function showPreview(table) {
    if (expandedTable === table) {
      expandedTable = null;
      preview = null;
      return;
    }
    expandedTable = table;
    previewing = true;
    preview = null;
    try {
      preview = await previewSourceTable(id, table, 10);
    } catch (e) {
      toastError(e.message);
    } finally {
      previewing = false;
    }
  }

  async function showRml(mappingId) {
    rml = rml?.id === mappingId ? null : { id: mappingId, text: $t('pages.sourceDetail.loadingRml') };
    if (!rml) return;
    try {
      rml = { id: mappingId, text: await getMappingRml(mappingId) };
    } catch (e) {
      rml = { id: mappingId, text: e.message };
    }
  }

  async function run() {
    if (!selectedMapping) return;
    running = true;
    gateReport = null;
    try {
      await startSourceRun(id, { mapping: selectedMapping, mode: 'full' });
      toastSuccess($t('pages.sourceDetail.runSucceeded'));
      await load();
    } catch (e) {
      // A 422 is the write gate refusing the run, not a failure to run: the
      // candidate graph exists and production is unchanged. Say so.
      if (e.status === 422) {
        gateReport = e.message;
        toastError($t('pages.sourceDetail.runGated'));
        await load();
      } else {
        toastError(e.message);
      }
    } finally {
      running = false;
    }
  }

  async function rollback(runId) {
    try {
      source = await rollbackRun(runId);
      toastSuccess($t('pages.sourceDetail.rolledBack'));
      await load();
    } catch (e) {
      toastError(e.message);
    }
  }

  async function removeRun(runId) {
    try {
      await deleteRun(runId);
      toastSuccess($t('pages.sourceDetail.runDeleted'));
      await load();
    } catch (e) {
      toastError(e.message);
    }
  }

  const isProduction = (r) => source?.production?.run === r.id;
  const duration = (ms) => (ms < 1000 ? `${ms} ms` : `${(ms / 1000).toFixed(1)} s`);
  const when = (iso) => (iso ? new Date(iso).toLocaleString() : '—');
</script>

<div class="src-detail">
  {#if loading}
    <div class="card placeholder"><Loader2 size={24} class="spin" /><p>{$t('pages.sourceDetail.loading')}</p></div>
  {:else if !source}
    <div class="card placeholder"><p>{$t('pages.sourceDetail.notFound')}</p></div>
  {:else}
    <div class="card head">
      <div class="crumb"><Link to="/sources">{$t('pages.sources.heading')}</Link> / {source.name}</div>
      <h2><Database size={18} /> {source.name}</h2>
      <div class="chips">
        <span class="chip">{source.dialect}</span>
        {#if source.readOnly}<span class="chip chip-ok"><Lock size={10} /> {$t('pages.sources.chipReadOnly')}</span>{/if}
        {#if source.allowlisted}
          <span class="chip chip-ok"><ShieldCheck size={10} /> {$t('pages.sources.chipAllowlisted')}</span>
        {:else}
          <span class="chip chip-warn"><ShieldAlert size={10} /> {$t('pages.sources.chipNotAllowlisted')}</span>
        {/if}
        {#if source.allowModelAssist}<span class="chip chip-assist"><Sparkles size={10} /> {$t('pages.sources.chipModelAssist')}</span>{/if}
      </div>
      <dl class="facts">
        <div><dt>{$t('pages.sourceDetail.location')}</dt><dd>{source.host ? `${source.host}${source.port ? `:${source.port}` : ''} · ` : ''}{source.database}</dd></div>
        {#if source.credential}
          <div><dt>{$t('pages.sourceDetail.credential')}</dt><dd><code>{source.credential}</code> <span class="dim">{$t('pages.sourceDetail.credentialNote')}</span></dd></div>
        {/if}
        <div><dt>{$t('pages.sourceDetail.timeout')}</dt><dd>{source.statementTimeoutMs} ms</dd></div>
        {#if source.dataset}<div><dt>{$t('pages.sourceDetail.dataset')}</dt><dd><Link to={`/datasets/${source.dataset}`}>{source.dataset}</Link></dd></div>{/if}
        <div><dt>{$t('pages.sourceDetail.serving')}</dt>
          <dd>{#if source.production}<code>{source.production.graph}</code>{:else}<span class="dim">{$t('pages.sources.neverRun')}</span>{/if}</dd>
        </div>
      </dl>
    </div>

    <div class="tabs">
      <button class:active={tab === 'runs'} on:click={() => (tab = 'runs')}>{$t('pages.sourceDetail.tabRuns')}</button>
      <button class:active={tab === 'mappings'} on:click={() => (tab = 'mappings')}>{$t('pages.sourceDetail.tabMappings')}</button>
      <button class:active={tab === 'schema'} on:click={() => { tab = 'schema'; loadSchema(); }}>{$t('pages.sourceDetail.tabSchema')}</button>
    </div>

    {#if tab === 'runs'}
      <div class="card">
        <div class="run-bar">
          <select bind:value={selectedMapping} disabled={!mappings.length}>
            {#each mappings as m (m.id)}<option value={m.id}>{m.title} (v{m.version})</option>{/each}
          </select>
          <button class="btn" on:click={run} disabled={running || !mappings.length}>
            {#if running}<Loader2 size={14} class="spin" />{:else}<Play size={14} />{/if}
            {$t('pages.sourceDetail.runNow')}
          </button>
        </div>
        {#if !mappings.length}
          <p class="dim">{$t('pages.sourceDetail.noMappings')}</p>
        {/if}
        {#if gateReport}
          <div class="gate"><AlertTriangle size={14} /> <div>{gateReport}</div></div>
        {/if}
      </div>

      {#if runs.length === 0}
        <div class="card placeholder"><p>{$t('pages.sourceDetail.noRuns')}</p></div>
      {:else}
        <ul class="run-list">
          {#each runs as r (r.id)}
            <li class="run-card" class:prod={isProduction(r)}>
              <div class="run-head">
                <span class="status status-{r.status}">
                  {#if r.status === 'succeeded'}<Check size={11} />{:else}<AlertTriangle size={11} />{/if}
                  {$t(`pages.sourceDetail.status_${r.status}`)}
                </span>
                {#if isProduction(r)}<span class="chip chip-ok">{$t('pages.sourceDetail.inProduction')}</span>{/if}
                <code class="graph">{shortenIRI(r.graph)}</code>
                <span class="dim">{when(r.startedAt)}</span>
              </div>
              <div class="run-meta">
                <span>{$t('pages.sourceDetail.mappingVersion', { values: { id: r.mapping.id, version: r.mapping.version } })}</span>
                <span>{r.rowsExtracted.toLocaleString()} {$t('pages.sourceDetail.rows')}</span>
                <span>{r.triplesProduced.toLocaleString()} {$t('pages.sourceDetail.triples')}</span>
                <span>{$t('pages.sourceDetail.nowHolding', { values: { count: r.graphTriples.toLocaleString() } })}</span>
                <span>{duration(r.durationMs)}</span>
                {#if r.shacl}
                  <span class:bad={!r.shacl.conforms}>
                    {r.shacl.conforms
                      ? $t('pages.sourceDetail.shaclClean')
                      : $t('pages.sourceDetail.shaclViolations', { values: { count: r.shacl.violations } })}
                  </span>
                {/if}
              </div>
              {#if r.error}<div class="run-error">{r.error}</div>{/if}
              <div class="run-actions">
                <a class="btn btn-sm btn-ghost" href={`/api/runs/${r.id}/provenance`} target="_blank" rel="noopener">
                  <FileCode size={12} /> {$t('pages.sourceDetail.provenance')}
                </a>
                {#if isProduction(r) && source.previous}
                  <button class="btn btn-sm btn-ghost" on:click={() => rollback(r.id)}>
                    <Undo2 size={12} /> {$t('pages.sourceDetail.rollback')}
                  </button>
                {/if}
                {#if !isProduction(r)}
                  <button class="btn btn-sm btn-ghost danger" on:click={() => removeRun(r.id)}>
                    <Trash2 size={12} /> {$t('pages.sourceDetail.deleteRun')}
                  </button>
                {/if}
              </div>
            </li>
          {/each}
        </ul>
      {/if}

    {:else if tab === 'mappings'}
      {#if mappings.length === 0}
        <div class="card placeholder"><p>{$t('pages.sourceDetail.noMappings')}</p></div>
      {:else}
        <ul class="map-list">
          {#each mappings as m (m.id)}
            <li class="card">
              <div class="map-head">
                <strong>{m.title}</strong>
                <span class="chip">v{m.version}</span>
                <span class="chip">{m.state}</span>
                <span class="dim">{$t('pages.sourceDetail.triplesMaps', { values: { count: m.triplesMaps } })}</span>
                <button class="btn btn-sm btn-ghost" on:click={() => showRml(m.id)}>
                  <FileCode size={12} /> {rml?.id === m.id ? $t('pages.sourceDetail.hideRml') : $t('pages.sourceDetail.showRml')}
                </button>
              </div>
              {#if m.shapesGraph}
                <div class="dim small">{$t('pages.sourceDetail.gatedBy')} <code>{shortenIRI(m.shapesGraph)}</code></div>
              {/if}
              {#if m.joins?.length}
                <div class="joins">
                  {#each m.joins as j}
                    <span class="join"><Link2 size={11} /> {shortenIRI(j.child)} → {shortenIRI(j.parent)}</span>
                  {/each}
                </div>
              {/if}
              {#if rml?.id === m.id}<pre class="rml">{rml.text}</pre>{/if}
            </li>
          {/each}
        </ul>
      {/if}

    {:else}
      {#if !schema}
        <div class="card placeholder"><Loader2 size={24} class="spin" /></div>
      {:else if schema.tables.length === 0}
        <div class="card placeholder"><p>{$t('pages.sourceDetail.noTables')}</p></div>
      {:else}
        <ul class="tbl-list">
          {#each schema.tables as tb (tb.name)}
            <li class="card">
              <button class="tbl-head" on:click={() => showPreview(tb.name)}>
                <Table2 size={14} />
                <strong>{tb.name}</strong>
                <span class="chip">{tb.kind}</span>
                <span class="dim">{tb.columns.length} {$t('pages.sourceDetail.columns')}</span>
                {#if tb.rowEstimate != null}<span class="dim">≈{tb.rowEstimate.toLocaleString()} {$t('pages.sourceDetail.rows')}</span>{/if}
              </button>
              <div class="cols">
                {#each tb.columns as c (c.name)}
                  <span class="col" class:pk={c.primaryKey} title={`${c.nativeType}${c.nullable ? '' : ' NOT NULL'}${c.comment ? ` — ${c.comment}` : ''}`}>
                    {#if c.primaryKey}<KeyRound size={10} />{/if}{c.name}
                    <em>{c.genericType}</em>
                  </span>
                {/each}
              </div>
              {#each tb.foreignKeys as fk}
                <div class="fk small dim"><Link2 size={11} /> {fk.columns.join(', ')} → {fk.refTable}({fk.refColumns.join(', ')})</div>
              {/each}
              {#if expandedTable === tb.name}
                {#if previewing}
                  <div class="placeholder"><Loader2 size={18} class="spin" /></div>
                {:else if preview}
                  <div class="preview-note dim small">{$t('pages.sourceDetail.previewNote')}</div>
                  <div class="scroll">
                    <table class="preview">
                      <thead><tr>{#each tb.columns as c}<th>{c.name}</th>{/each}</tr></thead>
                      <tbody>
                        {#each preview.rows as row}
                          <tr>{#each tb.columns as c}<td>{row[c.name]?.lexical ?? ''}</td>{/each}</tr>
                        {/each}
                      </tbody>
                    </table>
                  </div>
                {/if}
              {/if}
            </li>
          {/each}
        </ul>
      {/if}
    {/if}
  {/if}
</div>

<style>
  .src-detail { display: flex; flex-direction: column; gap: 1rem; }
  .crumb { font-size: .8rem; opacity: .75; margin-bottom: .35rem; }
  .head h2 { display: flex; align-items: center; gap: .5rem; margin: 0 0 .5rem; }
  .chips { display: flex; flex-wrap: wrap; gap: .4rem; }
  .facts { display: grid; grid-template-columns: repeat(auto-fit, minmax(240px, 1fr)); gap: .6rem 1.5rem; margin: .9rem 0 0; }
  .facts div { display: flex; flex-direction: column; gap: .15rem; }
  .facts dt { font-size: .75rem; text-transform: uppercase; letter-spacing: .03em; opacity: .6; }
  .facts dd { margin: 0; font-size: .87rem; word-break: break-all; }
  .tabs { display: flex; gap: .25rem; }
  .tabs button { padding: .4rem .8rem; border: 1px solid var(--border, #ccc); border-radius: 6px; background: transparent; color: inherit; cursor: pointer; font-size: .85rem; }
  .tabs button.active { background: color-mix(in srgb, currentColor 10%, transparent); font-weight: 600; }
  .run-bar { display: flex; gap: .5rem; align-items: center; flex-wrap: wrap; }
  .run-bar select { padding: .4rem .6rem; border-radius: 6px; border: 1px solid var(--border, #ccc); background: inherit; color: inherit; }
  .gate { display: flex; gap: .5rem; margin-top: .75rem; padding: .6rem .8rem; border-radius: 6px; background: color-mix(in srgb, crimson 12%, transparent); font-size: .85rem; }
  .run-list, .map-list, .tbl-list { list-style: none; padding: 0; margin: 0; display: flex; flex-direction: column; gap: .6rem; }
  .run-card { border: 1px solid var(--border, #ddd); border-radius: 8px; padding: .7rem .9rem; }
  .run-card.prod { border-color: color-mix(in srgb, green 45%, var(--border, #ddd)); }
  .run-head, .run-meta, .run-actions, .map-head, .joins { display: flex; flex-wrap: wrap; align-items: center; gap: .5rem; }
  .run-meta { font-size: .8rem; opacity: .85; margin-top: .35rem; }
  .run-actions { margin-top: .5rem; }
  .run-error { margin-top: .4rem; font-size: .8rem; color: crimson; }
  .status { font-size: .72rem; padding: .1rem .45rem; border-radius: 999px; display: inline-flex; align-items: center; gap: .25rem; }
  .status-succeeded { background: color-mix(in srgb, green 16%, transparent); }
  .status-rejected { background: color-mix(in srgb, orange 20%, transparent); }
  .status-failed { background: color-mix(in srgb, crimson 16%, transparent); }
  .graph { font-size: .76rem; opacity: .8; }
  .chip { font-size: .7rem; padding: .1rem .4rem; border-radius: 999px; border: 1px solid var(--border, #ccc); display: inline-flex; align-items: center; gap: .25rem; }
  .chip-ok { background: color-mix(in srgb, green 14%, transparent); }
  .chip-warn { background: color-mix(in srgb, orange 18%, transparent); }
  .chip-assist { background: color-mix(in srgb, rebeccapurple 16%, transparent); }
  .tbl-head { display: flex; align-items: center; gap: .5rem; width: 100%; background: none; border: 0; color: inherit; cursor: pointer; padding: 0; text-align: left; }
  .cols { display: flex; flex-wrap: wrap; gap: .3rem; margin-top: .5rem; }
  .col { font-size: .75rem; padding: .1rem .4rem; border-radius: 4px; border: 1px solid var(--border, #ddd); display: inline-flex; align-items: center; gap: .25rem; }
  .col.pk { border-color: color-mix(in srgb, goldenrod 60%, var(--border, #ddd)); }
  .col em { opacity: .55; font-style: normal; }
  .fk, .joins { margin-top: .4rem; }
  .join { font-size: .75rem; display: inline-flex; align-items: center; gap: .25rem; }
  .small { font-size: .78rem; }
  .scroll { overflow-x: auto; margin-top: .5rem; }
  .preview { border-collapse: collapse; font-size: .78rem; }
  .preview th, .preview td { border: 1px solid var(--border, #ddd); padding: .2rem .45rem; text-align: left; white-space: nowrap; }
  .preview-note { margin-top: .5rem; }
  .rml { margin-top: .6rem; max-height: 22rem; overflow: auto; font-size: .75rem; padding: .6rem; border-radius: 6px; background: color-mix(in srgb, currentColor 6%, transparent); }
  .placeholder { text-align: center; padding: 1.5rem; }
  .dim { opacity: .7; }
  .bad { color: crimson; }
  .danger { color: crimson; }
</style>
