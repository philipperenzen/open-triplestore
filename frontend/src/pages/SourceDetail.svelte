<script>
  // "Sources" workspace, steps 2–5 — Explore, Map, Dry-run, Publish & Runs.
  //
  // Explore: the schema, the profile the store computed (counts and shapes,
  // never rows — it is what the offline proposer reads), a raw first-rows
  // preview (pre-clean data, admin only), and drift against the profile a
  // mapping was written against, with the re-map tickets it opens.
  // Map: three views of one RML graph — a matrix, the Turtle itself, and a
  // YARRRML composer — plus the one-time converter for a legacy bundle.
  // Dry-run: a sample, validated, split into mapping defects and data issues,
  // with every attempt kept as a round.
  // Runs: trigger a run, read the history, roll back in one click.
  import { onMount } from 'svelte';
  import { t } from 'svelte-i18n';
  import {
    Database, Loader2, Play, Undo2, Trash2, Table2, KeyRound, Link2, AlertTriangle, Check, Lock,
    ShieldCheck, ShieldAlert, Sparkles, FileCode, Activity, FlaskConical, GitCompareArrows, Save,
    Ticket, X, ChevronDown, ChevronRight,
  } from 'lucide-svelte';
  import {
    getSource, introspectSource, previewSourceTable, listSourceMappings, listSourceRuns,
    startSourceRun, rollbackRun, deleteRun, getMappingRml, profileSource, getSourceProfile,
    driftSource, listSourceTickets, closeTicket, dryRunSource, createMapping, updateMapping,
    convertLegacyMapping,
  } from '../lib/api.js';
  import { Link, navigate } from '../lib/router/index.js';
  import { shortenIRI } from '../lib/rdf-utils.js';
  import { parseSourceProfile } from '../lib/sourceProfile.ts';
  import { parseRmlMatrix } from '../lib/rmlMatrix.ts';
  import { isAdmin, authInitialized } from '../lib/stores.js';
  import { toastError, toastSuccess } from '../lib/toast.ts';
  import SparqlEditorCM from '../components/SparqlEditorCM.svelte';

  export let id;

  let source = null;
  let mappings = [];
  let runs = [];
  let loading = true;
  let tab = 'explore';

  // ── Explore ──
  let schema = null;
  let expandedTable = null;
  let preview = null;
  let previewing = false;
  let profile = null;
  let profileLoaded = false;
  let profiling = false;
  let advanced = false;
  let drift = null;
  let drifting = false;
  let tickets = [];

  // ── Map ──
  let selectedMapping = '';
  let mapView = 'matrix';
  let rmlText = '';
  let rmlLoadedFor = '';
  let matrix = [];
  let yarrrmlText = '';
  let legacyText = '';
  let legacyEmptyAsNull = false;
  let legacyWarnings = [];
  let newMapping = { id: '', title: '' };
  let saving = false;
  const YARRRML_EXAMPLE = 'prefixes:\n  ex: http://example.org/\nmappings:\n  product:\n    table: products\n    s: ex:p_$(id)\n    po:\n      - [a, ex:Product]\n      - [ex:name, $(name)]';
  const LEGACY_EXAMPLE = 'prefixes: { ex: "http://example.org/" }\nentities:\n  - source_table: products\n    primary_key: product_id\n    subject_iri: "ex:product_{product_id}"\n    rdf_type: "ex:Product"\n    properties:\n      - { column: price, predicate: "ex:hasPrice", datatype: "xsd:decimal" }';

  // ── Dry-run ──
  let dr = { mapping: '', table: '', sampleSize: 20 };
  let drResult = null;
  let dryRunning = false;
  let rounds = [];
  let openEntity = null;

  // ── Runs ──
  let running = false;
  let gateReport = null;

  let _guardChecked = false;
  $: if ($authInitialized && !_guardChecked) {
    _guardChecked = true;
    if (!$isAdmin) navigate('/');
  }

  $: selected = mappings.find((m) => m.id === selectedMapping) || null;
  $: matrix = parseRmlMatrix(rmlText);

  /** Switch tabs, loading what the tab needs the first time it is opened. */
  function show(next) {
    tab = next;
    if (next === 'explore') {
      loadSchema();
      if (!profileLoaded) loadProfile();
    }
    if (next === 'map' && selectedMapping && rmlLoadedFor !== selectedMapping) loadRml(selectedMapping);
  }

  onMount(async () => {
    await load();
    show('explore');
  });

  async function load() {
    loading = true;
    try {
      source = await getSource(id);
      mappings = await listSourceMappings(id).catch(() => []);
      runs = await listSourceRuns(id).catch(() => []);
      tickets = await listSourceTickets(id).catch(() => []);
      if (!selectedMapping && mappings.length) selectedMapping = mappings[0].id;
      if (!dr.mapping && mappings.length) dr.mapping = mappings[0].id;
    } catch (e) {
      toastError(e.message);
    } finally {
      loading = false;
    }
  }

  // ───────────────────────────── Explore ─────────────────────────────

  async function loadSchema() {
    if (schema) return;
    try {
      schema = await introspectSource(id);
    } catch (e) {
      toastError(e.message);
      schema = { tables: [] };
    }
  }

  async function loadProfile() {
    profileLoaded = true;
    try {
      const turtle = await getSourceProfile(id);
      profile = parseSourceProfile(typeof turtle === 'string' ? turtle : '');
    } catch (e) {
      // 404 is "not profiled yet", which the screen says in its own words.
      profile = null;
      if (e.status && e.status !== 404) toastError(e.message);
    }
  }

  async function profileNow() {
    profiling = true;
    try {
      const summary = await profileSource(id);
      toastSuccess($t('pages.sourceDetail.profiled', { values: { version: summary.version } }));
      profileLoaded = false;
      await loadProfile();
      drift = null;
    } catch (e) {
      toastError(e.message);
    } finally {
      profiling = false;
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

  async function checkDrift() {
    drifting = true;
    try {
      drift = await driftSource(id, selectedMapping ? { mapping: selectedMapping } : {});
      tickets = await listSourceTickets(id).catch(() => tickets);
    } catch (e) {
      drift = null;
      toastError(e.message);
    } finally {
      drifting = false;
    }
  }

  async function closeOne(ticketId) {
    try {
      await closeTicket(ticketId);
      toastSuccess($t('pages.sourceDetail.ticketClosed'));
      tickets = await listSourceTickets(id).catch(() => tickets);
    } catch (e) {
      toastError(e.message);
    }
  }

  const changesOf = (tbl) => [
    [$t('pages.sourceDetail.driftNewCols'), tbl.newColumns],
    [$t('pages.sourceDetail.driftRemovedCols'), tbl.removedColumns],
    [$t('pages.sourceDetail.driftTypeChanges'), tbl.typeChanges.map((c) => `${c.column}: ${c.from} → ${c.to}`)],
    [$t('pages.sourceDetail.driftShifts'), tbl.distributionShifts.map((s) => `${s.column} (KL ${s.klDivergence.toFixed(2)})`)],
    [$t('pages.sourceDetail.driftCodeLists'), tbl.codeListChanges.map((c) => `${c.column}: ${c.change}`)],
  ].filter(([, items]) => items.length);

  // ───────────────────────────── Map ─────────────────────────────

  async function loadRml(mappingId) {
    rmlLoadedFor = mappingId;
    try {
      const text = await getMappingRml(mappingId);
      rmlText = typeof text === 'string' ? text : '';
    } catch (e) {
      rmlText = '';
      toastError(e.message);
    }
  }

  async function saveVersion() {
    if (!selectedMapping || !rmlText.trim()) return;
    saving = true;
    try {
      const m = await updateMapping(selectedMapping, { rml: rmlText });
      toastSuccess($t('pages.sourceDetail.savedVersion', { values: { version: m.version } }));
      await load();
      rmlLoadedFor = '';
    } catch (e) {
      toastError(e.message);
    } finally {
      saving = false;
    }
  }

  async function saveYarrrml() {
    if (!yarrrmlText.trim()) return;
    saving = true;
    try {
      if (selectedMapping) {
        const m = await updateMapping(selectedMapping, { yarrrml: yarrrmlText });
        toastSuccess($t('pages.sourceDetail.savedVersion', { values: { version: m.version } }));
      } else {
        await registerNew({ yarrrml: yarrrmlText });
      }
      await load();
      rmlLoadedFor = '';
      mapView = 'turtle';
    } catch (e) {
      toastError(e.message);
    } finally {
      saving = false;
    }
  }

  async function registerNew(body) {
    const mid = newMapping.id.trim();
    if (!mid) { toastError($t('pages.sourceDetail.needMappingId')); return; }
    const created = await createMapping({ id: mid, title: newMapping.title.trim() || mid, source: id, ...body });
    toastSuccess($t('pages.sourceDetail.mappingRegistered', { values: { id: created.id } }));
    newMapping = { id: '', title: '' };
    selectedMapping = created.id;
    dr.mapping = created.id;
  }

  async function registerFromTurtle() {
    if (!rmlText.trim()) return;
    saving = true;
    try {
      await registerNew({ rml: rmlText });
      await load();
      rmlLoadedFor = '';
    } catch (e) {
      toastError(e.message);
    } finally {
      saving = false;
    }
  }

  async function setState(state) {
    if (!selectedMapping) return;
    try {
      await updateMapping(selectedMapping, { state });
      toastSuccess($t('pages.sourceDetail.stateChanged', { values: { state } }));
      await load();
    } catch (e) {
      toastError(e.message);
    }
  }

  async function convertLegacy() {
    if (!legacyText.trim()) return;
    saving = true;
    try {
      const out = await convertLegacyMapping({
        format: 'sql2rdf', source: id, document: legacyText, emptyAsNull: legacyEmptyAsNull,
      });
      rmlText = out.rml;
      rmlLoadedFor = '__converted__';
      legacyWarnings = out.warnings || [];
      toastSuccess($t('pages.sourceDetail.converted', { values: { count: out.triplesMaps } }));
      mapView = 'turtle';
    } catch (e) {
      toastError(e.message);
    } finally {
      saving = false;
    }
  }

  const kindLabel = (kind) => $t(`pages.sourceDetail.kind_${kind}`);

  // ───────────────────────────── Dry-run ─────────────────────────────

  async function runDryRun() {
    dryRunning = true;
    try {
      const body = { sampleSize: Number(dr.sampleSize) || 20 };
      if (dr.table.trim()) body.table = dr.table.trim();
      if (dr.mapping === '__editor__') {
        body.rml = rmlText;
        const m = selected;
        if (m?.shapesGraph) body.shapesGraph = m.shapesGraph;
        if (m?.model) { body.model = m.model; body.modelVersion = m.modelVersion; }
      } else {
        body.mapping = dr.mapping;
      }
      drResult = await dryRunSource(id, body);
      openEntity = null;
      rounds = [
        {
          n: rounds.length + 1,
          at: new Date().toLocaleTimeString(),
          mapping: dr.mapping === '__editor__' ? $t('pages.sourceDetail.useEditor') : dr.mapping,
          defects: drResult.classification.mappingDefects.length,
          issues: drResult.classification.dataIssues.length,
          triples: drResult.triples,
          result: drResult,
        },
        ...rounds,
      ];
    } catch (e) {
      toastError(e.message);
    } finally {
      dryRunning = false;
    }
  }

  const pct = (share) => `${Math.round((share || 0) * 100)}%`;

  // ───────────────────────────── Runs ─────────────────────────────

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
  const fmt = (n) => (n == null ? '—' : Number.isInteger(n) ? n.toLocaleString() : n.toFixed(2));
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

    <div class="tabs" role="tablist">
      <button role="tab" class:active={tab === 'explore'} on:click={() => show('explore')}><Activity size={13} /> {$t('pages.sourceDetail.tabExplore')}</button>
      <button role="tab" class:active={tab === 'map'} on:click={() => show('map')}><FileCode size={13} /> {$t('pages.sourceDetail.tabMap')}</button>
      <button role="tab" class:active={tab === 'dryrun'} on:click={() => show('dryrun')}><FlaskConical size={13} /> {$t('pages.sourceDetail.tabDryRun')}</button>
      <button role="tab" class:active={tab === 'runs'} on:click={() => show('runs')}><Play size={13} /> {$t('pages.sourceDetail.tabRuns')}</button>
    </div>

    <!-- ═══════════════════════ Explore ═══════════════════════ -->
    {#if tab === 'explore'}
      <div class="card">
        <div class="bar">
          <h3><Activity size={15} /> {$t('pages.sourceDetail.profileHeading')}</h3>
          {#if profile?.activity?.version}
            <span class="chip">{$t('pages.sourceDetail.profileVersion', { values: { version: profile.activity.version } })}</span>
            <span class="dim small">{when(profile.activity.startedAt)}</span>
          {/if}
          <span class="grow"></span>
          <label class="switch small"><input type="checkbox" bind:checked={advanced} /> {$t('pages.sourceDetail.advanced')}</label>
          <button class="btn btn-sm" on:click={profileNow} disabled={profiling}>
            {#if profiling}<Loader2 size={13} class="spin" />{:else}<Activity size={13} />{/if}
            {profiling ? $t('pages.sourceDetail.profiling') : $t('pages.sourceDetail.profileNow')}
          </button>
        </div>
        {#if !profile}
          <p class="dim">{$t('pages.sourceDetail.noProfile')}</p>
        {/if}
      </div>

      {#if profile}
        <ul class="tbl-list">
          {#each profile.tables as tb (tb.iri)}
            <li class="card">
              <button class="tbl-head" on:click={() => showPreview(tb.name)} aria-expanded={expandedTable === tb.name}>
                {#if expandedTable === tb.name}<ChevronDown size={14} />{:else}<ChevronRight size={14} />{/if}
                <Table2 size={14} />
                <strong>{tb.name}</strong>
                {#if tb.isView}<span class="chip">view</span>{/if}
                {#if tb.rows != null}<span class="dim">{tb.rows.toLocaleString()} {$t('pages.sourceDetail.rows')}</span>{/if}
                <span class="dim">{tb.columns.length} {$t('pages.sourceDetail.columns')}</span>
                <span class="dim">{$t('pages.sourceDetail.codeLists', { values: { count: tb.columns.filter((c) => c.topValues.length).length } })}</span>
                {#if advanced}
                  <span class="dim mono" title={tb.structuralHash}>{$t('pages.sourceDetail.structuralHash')} {tb.structuralHash.slice(0, 10)}</span>
                  <span class="dim">{$t('pages.sourceDetail.sampledRows', { values: { count: tb.sampledRows } })}</span>
                {/if}
              </button>

              <div class="scroll">
                <table class="stats">
                  <thead>
                    <tr>
                      <th>{$t('pages.sourceDetail.column')}</th>
                      <th>{$t('pages.sourceDetail.type')}</th>
                      <th>{$t('pages.sourceDetail.pattern')}</th>
                      {#if advanced}
                        <th>{$t('pages.sourceDetail.distinct')}</th>
                        <th>NULL</th>
                        <th>{$t('pages.sourceDetail.ratio')}</th>
                        <th>{$t('pages.sourceDetail.meanLength')}</th>
                        <th>min / max / mean / p50 / p99</th>
                      {/if}
                      <th>{$t('pages.sourceDetail.topValues')}</th>
                    </tr>
                  </thead>
                  <tbody>
                    {#each tb.columns as c (c.iri)}
                      <tr>
                        <td class="mono">
                          {#if tb.primaryKey.includes(c.name)}<KeyRound size={10} />{/if}{c.name}
                          {#if c.required}<span class="chip chip-tiny">{$t('pages.sourceDetail.colRequired')}</span>{/if}
                        </td>
                        <td>{c.datatype ?? '—'} <span class="dim">{c.nativeType}</span></td>
                        <td>{#if c.pattern}{c.pattern}{#if c.patternConfidence != null} <span class="dim">{pct(c.patternConfidence)}</span>{/if}{:else}—{/if}</td>
                        {#if advanced}
                          <td>{fmt(c.distinct)}</td>
                          <td>{fmt(c.nulls)}</td>
                          <td>{c.cardinalityRatio == null ? '—' : c.cardinalityRatio.toFixed(2)}</td>
                          <td>{fmt(c.meanLength)}</td>
                          <td>{#if c.numeric}{fmt(c.numeric.min)} / {fmt(c.numeric.max)} / {fmt(c.numeric.mean)} / {fmt(c.numeric.p50)} / {fmt(c.numeric.p99)}{:else}—{/if}</td>
                        {/if}
                        <td class="values">
                          {#each c.topValues.slice(0, advanced ? 50 : 8) as v (v.rank)}
                            <span class="chip chip-tiny" title={`${v.count}×`}>{v.value} <em>{v.count}</em></span>
                          {/each}
                          {#if !advanced && c.topValues.length > 8}<span class="dim small">+{c.topValues.length - 8}</span>{/if}
                        </td>
                      </tr>
                    {/each}
                  </tbody>
                </table>
              </div>

              {#if tb.foreignKeys.length}
                <div class="fk small dim">
                  <Link2 size={11} /> {$t('pages.sourceDetail.fkGraph')}:
                  {#each tb.foreignKeys as fk}
                    <span class="edge">{tb.name}.{fk.columns.join(', ')} → {fk.refTable}({fk.refColumns.join(', ')})</span>
                  {/each}
                </div>
              {/if}

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
      {:else if schema?.tables?.length}
        <!-- No profile yet: the catalogue, so a table can still be previewed. -->
        <ul class="tbl-list">
          {#each schema.tables as tb (tb.name)}
            <li class="card">
              <button class="tbl-head" on:click={() => showPreview(tb.name)}>
                <Table2 size={14} /><strong>{tb.name}</strong>
                <span class="chip">{tb.kind}</span>
                <span class="dim">{tb.columns.length} {$t('pages.sourceDetail.columns')}</span>
                {#if tb.rowEstimate != null}<span class="dim">≈{tb.rowEstimate.toLocaleString()} {$t('pages.sourceDetail.rows')}</span>{/if}
              </button>
              <div class="cols">
                {#each tb.columns as c (c.name)}
                  <span class="col" class:pk={c.primaryKey} title={`${c.nativeType}${c.nullable ? '' : ' NOT NULL'}${c.comment ? ` — ${c.comment}` : ''}`}>
                    {#if c.primaryKey}<KeyRound size={10} />{/if}{c.name} <em>{c.genericType}</em>
                  </span>
                {/each}
              </div>
              {#if expandedTable === tb.name && preview}
                <div class="preview-note dim small">{$t('pages.sourceDetail.previewNote')}</div>
                <div class="scroll">
                  <table class="preview">
                    <thead><tr>{#each tb.columns as c}<th>{c.name}</th>{/each}</tr></thead>
                    <tbody>{#each preview.rows as row}<tr>{#each tb.columns as c}<td>{row[c.name]?.lexical ?? ''}</td>{/each}</tr>{/each}</tbody>
                  </table>
                </div>
              {/if}
            </li>
          {/each}
        </ul>
      {:else if schema}
        <div class="card placeholder"><p>{$t('pages.sourceDetail.noTables')}</p></div>
      {/if}

      <div class="card">
        <div class="bar">
          <h3><GitCompareArrows size={15} /> {$t('pages.sourceDetail.driftHeading')}</h3>
          {#if mappings.length}
            <select bind:value={selectedMapping}>
              {#each mappings as m (m.id)}<option value={m.id}>{m.title} (v{m.version}{m.profileVersion ? ` · profile v${m.profileVersion}` : ''})</option>{/each}
            </select>
          {/if}
          <span class="grow"></span>
          <button class="btn btn-sm" on:click={checkDrift} disabled={drifting || !profile}>
            {#if drifting}<Loader2 size={13} class="spin" />{:else}<GitCompareArrows size={13} />{/if}
            {drifting ? $t('pages.sourceDetail.drifting') : $t('pages.sourceDetail.checkDrift')}
          </button>
        </div>
        {#if drift}
          {#if !drift.affectedTables.length && !drift.modelVersionBump}
            <p class="ok"><Check size={13} /> {$t('pages.sourceDetail.driftNone', { values: { baseline: drift.baseline, candidate: drift.candidate } })}</p>
          {:else}
            <p class="small dim">v{drift.baseline} → v{drift.candidate} · KL ≥ {drift.klThreshold}</p>
            {#if drift.modelVersionBump}
              <p class="warn"><AlertTriangle size={13} /> {$t('pages.sourceDetail.modelBump', { values: { model: drift.modelVersionBump.model, from: drift.modelVersionBump.mappingVersion, to: drift.modelVersionBump.latestVersion } })}</p>
            {/if}
            {#each drift.tables.filter((x) => x.affected) as tbl (tbl.table)}
              <div class="drift-table">
                <strong>{tbl.table}</strong>
                {#each changesOf(tbl) as [label, items]}
                  <div class="small"><span class="dim">{label}:</span> {items.join(', ')}</div>
                {/each}
              </div>
            {/each}
            {#if drift.newTables.length}<div class="small"><span class="dim">{$t('pages.sourceDetail.driftNewTables')}:</span> {drift.newTables.join(', ')}</div>{/if}
            {#if drift.removedTables.length}<div class="small"><span class="dim">{$t('pages.sourceDetail.driftRemovedTables')}:</span> {drift.removedTables.join(', ')}</div>{/if}
          {/if}
        {/if}

        <h4><Ticket size={13} /> {$t('pages.sourceDetail.tickets')}</h4>
        {#if !tickets.length}
          <p class="dim small">{$t('pages.sourceDetail.noTickets')}</p>
        {:else}
          <ul class="ticket-list">
            {#each tickets as tk (tk.id)}
              <li class:closed={tk.status === 'closed'}>
                <span class="chip" class:chip-warn={tk.status === 'open'}>{tk.status}</span>
                <span>{tk.reason}</span>
                <span class="dim small">{tk.affectedTables.join(', ')}</span>
                {#if tk.mapping}<code class="small">{tk.mapping.replace('urn:mapping:', '')}</code>{/if}
                <span class="dim small">{when(tk.updatedAt)}</span>
                {#if tk.status === 'open'}
                  <button class="btn btn-sm btn-ghost" on:click={() => closeOne(tk.id)}><X size={12} /> {$t('pages.sourceDetail.closeTicket')}</button>
                {/if}
              </li>
            {/each}
          </ul>
        {/if}
      </div>

    <!-- ═══════════════════════ Map ═══════════════════════ -->
    {:else if tab === 'map'}
      <div class="card">
        <div class="bar">
          {#if mappings.length}
            <select bind:value={selectedMapping} on:change={() => loadRml(selectedMapping)}>
              {#each mappings as m (m.id)}<option value={m.id}>{m.title} (v{m.version} · {m.state})</option>{/each}
            </select>
          {:else}
            <span class="dim">{$t('pages.sourceDetail.noMappings')}</span>
          {/if}
          {#if selected}
            {#if selected.shapesGraph}<span class="dim small">{$t('pages.sourceDetail.gatedBy')} <code>{shortenIRI(selected.shapesGraph)}</code></span>{/if}
            <span class="grow"></span>
            <span class="small dim">{$t('pages.sourceDetail.state')}:</span>
            {#each ['draft', 'proposed', 'approved'] as st}
              <button class="btn btn-sm" class:btn-ghost={selected.state !== st} disabled={selected.state === st} on:click={() => setState(st)}>{$t(`pages.sourceDetail.state_${st}`)}</button>
            {/each}
          {/if}
        </div>
        <div class="subtabs">
          {#each ['matrix', 'turtle', 'yarrrml', 'legacy'] as v}
            <button class:active={mapView === v} on:click={() => (mapView = v)}>{$t(`pages.sourceDetail.view_${v}`)}</button>
          {/each}
        </div>
      </div>

      {#if mapView === 'matrix'}
        {#if !matrix.length}
          <div class="card placeholder"><p class="dim">{$t('pages.sourceDetail.noMatrix')}</p></div>
        {:else}
          {#each matrix as m (m.iri)}
            <div class="card">
              <div class="map-head">
                <strong>{m.name}</strong>
                <span class="dim small">{$t('pages.sourceDetail.logicalSource')} <code>{m.table ?? m.query ?? '—'}</code></span>
                <span class="dim small">{$t('pages.sourceDetail.subjectLabel')} <code>{m.subject}</code></span>
                {#each m.classes as c}<span class="chip">{shortenIRI(c)}</span>{/each}
              </div>
              <div class="scroll">
                <table class="matrix">
                  <thead><tr><th>{$t('pages.sourceDetail.matrixPredicate')}</th><th>{$t('pages.sourceDetail.matrixObject')}</th><th>{$t('pages.sourceDetail.matrixDatatype')}</th><th>{$t('pages.sourceDetail.matrixGate')}</th></tr></thead>
                  <tbody>
                    {#each m.rows as r}
                      <tr>
                        <td><code>{shortenIRI(r.predicate)}</code></td>
                        <td><span class="chip chip-tiny">{kindLabel(r.kind)}</span> <code>{r.kind === 'constant' ? shortenIRI(r.detail) : r.detail}</code>
                          {#if r.joins?.length}<span class="dim small">({r.joins.map((j) => `${j.child} = ${j.parent}`).join(', ')})</span>{/if}</td>
                        <td>{r.datatype ?? (r.language ? `@${r.language}` : '—')}</td>
                        <td class="dim small">{$t('pages.sourceDetail.matrixNoProposal')}</td>
                      </tr>
                    {/each}
                  </tbody>
                </table>
              </div>
            </div>
          {/each}
        {/if}

      {:else if mapView === 'turtle'}
        <div class="card">
          {#if legacyWarnings.length}
            <ul class="warnings">{#each legacyWarnings as w}<li><AlertTriangle size={12} /> {w}</li>{/each}</ul>
          {/if}
          <SparqlEditorCM mode="turtle" query={rmlText} height="380px" showFormat={false} on:change={(e) => (rmlText = e.detail)} />
          <div class="actions">
            {#if selectedMapping}
              <button class="btn" on:click={saveVersion} disabled={saving || !rmlText.trim()}><Save size={13} /> {$t('pages.sourceDetail.saveVersion')}</button>
            {/if}
            <span class="grow"></span>
            <input class="mini" bind:value={newMapping.id} placeholder={$t('pages.sourceDetail.newMappingId')} />
            <input class="mini" bind:value={newMapping.title} placeholder={$t('pages.sourceDetail.newMappingTitle')} />
            <button class="btn btn-ghost" on:click={registerFromTurtle} disabled={saving || !rmlText.trim()}>{$t('pages.sourceDetail.registerMapping')}</button>
          </div>
        </div>

      {:else if mapView === 'yarrrml'}
        <div class="card">
          <p class="dim small">{$t('pages.sourceDetail.yarrrmlHint')}</p>
          <textarea class="code" rows="16" bind:value={yarrrmlText} placeholder={YARRRML_EXAMPLE}></textarea>
          <div class="actions">
            {#if !selectedMapping}
              <input class="mini" bind:value={newMapping.id} placeholder={$t('pages.sourceDetail.newMappingId')} />
              <input class="mini" bind:value={newMapping.title} placeholder={$t('pages.sourceDetail.newMappingTitle')} />
            {/if}
            <button class="btn" on:click={saveYarrrml} disabled={saving || !yarrrmlText.trim()}>
              <Save size={13} /> {selectedMapping ? $t('pages.sourceDetail.saveVersion') : $t('pages.sourceDetail.registerMapping')}
            </button>
          </div>
        </div>

      {:else}
        <div class="card">
          <p class="dim small">{$t('pages.sourceDetail.legacyHint')}</p>
          <textarea class="code" rows="16" bind:value={legacyText} placeholder={LEGACY_EXAMPLE}></textarea>
          <div class="actions">
            <label class="switch small"><input type="checkbox" bind:checked={legacyEmptyAsNull} /> {$t('pages.sourceDetail.emptyAsNull')}</label>
            <span class="grow"></span>
            <button class="btn" on:click={convertLegacy} disabled={saving || !legacyText.trim()}>{$t('pages.sourceDetail.convert')}</button>
          </div>
        </div>
      {/if}

    <!-- ═══════════════════════ Dry-run ═══════════════════════ -->
    {:else if tab === 'dryrun'}
      <div class="card">
        <div class="bar">
          <select bind:value={dr.mapping}>
            {#each mappings as m (m.id)}<option value={m.id}>{m.title} (v{m.version})</option>{/each}
            {#if rmlText.trim()}<option value="__editor__">{$t('pages.sourceDetail.useEditor')}</option>{/if}
          </select>
          <label class="small">{$t('pages.sourceDetail.table')} <input class="mini" bind:value={dr.table} placeholder={$t('pages.sourceDetail.anyTable')} /></label>
          <label class="small">{$t('pages.sourceDetail.sampleSize')} <input class="mini num" type="number" min="1" max="1000" bind:value={dr.sampleSize} /></label>
          <span class="grow"></span>
          <button class="btn" on:click={runDryRun} disabled={dryRunning || !dr.mapping}>
            {#if dryRunning}<Loader2 size={13} class="spin" />{:else}<FlaskConical size={13} />{/if}
            {dryRunning ? $t('pages.sourceDetail.dryRunning') : $t('pages.sourceDetail.runDryRun')}
          </button>
        </div>
        {#if !mappings.length && !rmlText.trim()}<p class="dim">{$t('pages.sourceDetail.noMappings')}</p>{/if}
      </div>

      {#if drResult}
        <div class="card">
          <div class="bar">
            <strong>{$t('pages.sourceDetail.drRows', { values: { rows: drResult.rows, triples: drResult.triples } })}</strong>
            {#if drResult.report}
              <span class="chip" class:chip-ok={drResult.report.conforms} class:chip-warn={!drResult.report.conforms}>
                {drResult.report.conforms ? $t('pages.sourceDetail.drConforms') : $t('pages.sourceDetail.drViolations', { values: { count: drResult.report.resultsCount } })}
              </span>
            {/if}
            <span class="dim small">{$t('pages.sourceDetail.expires', { values: { graph: drResult.graph, when: when(drResult.expiresAt) } })}</span>
          </div>
          <div class="chips">
            {#each drResult.maps as m (m.triplesMap)}
              <span class="chip chip-tiny" title={m.triplesMap}>{m.table ?? shortenIRI(m.triplesMap)}: {$t('pages.sourceDetail.drMaps', { values: { sampled: m.sampledRows, pulled: m.pulledInRows } })} → {m.triples}</span>
            {/each}
          </div>
          {#if drResult.warnings.length}
            <ul class="warnings">{#each drResult.warnings as w}<li><AlertTriangle size={12} /> {w}</li>{/each}</ul>
          {/if}
        </div>

        <div class="two">
          <div class="card">
            <h4 class="bad-h"><AlertTriangle size={13} /> {$t('pages.sourceDetail.mappingDefects')} <span class="count">{drResult.classification.mappingDefects.length}</span></h4>
            {#if !drResult.classification.mappingDefects.length}<p class="dim small">{$t('pages.sourceDetail.noneFound')}</p>{/if}
            {#each drResult.classification.mappingDefects as f}
              <div class="finding defect">
                <div><code>{shortenIRI(String(f.path ?? f.shape).replace(/^<|>$/g, ''))}</code> <span class="chip chip-tiny">{f.constraint}</span></div>
                <div class="small">{f.message}</div>
                <div class="small dim">{$t('pages.sourceDetail.share', { values: { affected: f.affected, population: f.population, share: Math.round(f.share * 100) } })}</div>
              </div>
            {/each}
          </div>
          <div class="card">
            <h4><AlertTriangle size={13} /> {$t('pages.sourceDetail.dataIssues')} <span class="count">{drResult.classification.dataIssues.length}</span></h4>
            {#if !drResult.classification.dataIssues.length}<p class="dim small">{$t('pages.sourceDetail.noneFound')}</p>{/if}
            {#each drResult.classification.dataIssues as f}
              <div class="finding issue">
                <div><code>{shortenIRI(String(f.path ?? f.shape).replace(/^<|>$/g, ''))}</code> <span class="chip chip-tiny">{f.constraint}</span></div>
                <div class="small">{f.message}</div>
                <div class="small dim">{$t('pages.sourceDetail.share', { values: { affected: f.affected, population: f.population, share: Math.round(f.share * 100) } })} · {f.focusNodes.map(shortenIRI).join(', ')}</div>
              </div>
            {/each}
          </div>
        </div>

        <div class="card">
          <h4>{$t('pages.sourceDetail.entities')} <span class="count">{drResult.entities.length}</span></h4>
          <ul class="entity-list">
            {#each drResult.entities as e (e.subject)}
              <li>
                <button class="tbl-head" on:click={() => (openEntity = openEntity === e.subject ? null : e.subject)}>
                  {#if openEntity === e.subject}<ChevronDown size={13} />{:else}<ChevronRight size={13} />{/if}
                  <code>{shortenIRI(e.subject)}</code>
                  {#each e.types as ty}<span class="chip chip-tiny">{shortenIRI(ty)}</span>{/each}
                  {#if e.violations.length}<span class="chip chip-tiny chip-warn">{$t('pages.sourceDetail.violations', { values: { count: e.violations.length } })}</span>{/if}
                </button>
                {#if openEntity === e.subject}
                  <pre class="rml">{e.turtle}</pre>
                  {#each e.violations as v}
                    <div class="small bad"><code>{v.path ? shortenIRI(v.path.replace(/^<|>$/g, '')) : ''}</code> {v.message}</div>
                  {/each}
                {/if}
              </li>
            {/each}
          </ul>
        </div>
      {/if}

      {#if rounds.length}
        <div class="card">
          <h4>{$t('pages.sourceDetail.rounds')}</h4>
          <ul class="round-list">
            {#each rounds as r (r.n)}
              <li>
                <button class="tbl-head" on:click={() => (drResult = r.result)}>
                  <span class="chip chip-tiny">#{r.n}</span> <span class="dim small">{r.at}</span> <code>{r.mapping}</code>
                  <span class="small" class:bad={r.defects > 0}>{$t('pages.sourceDetail.mappingDefects')}: {r.defects}</span>
                  <span class="small">{$t('pages.sourceDetail.dataIssues')}: {r.issues}</span>
                  <span class="small dim">{r.triples} {$t('pages.sourceDetail.triples')}</span>
                </button>
              </li>
            {/each}
          </ul>
        </div>
      {/if}

    <!-- ═══════════════════════ Runs ═══════════════════════ -->
    {:else}
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
    {/if}
  {/if}
</div>

<style>
  .src-detail { display: flex; flex-direction: column; gap: 1rem; }
  .crumb { font-size: .8rem; opacity: .75; margin-bottom: .35rem; }
  .head h2 { display: flex; align-items: center; gap: .5rem; margin: 0 0 .5rem; }
  h3 { display: inline-flex; align-items: center; gap: .4rem; margin: 0; font-size: 1rem; }
  h4 { display: flex; align-items: center; gap: .4rem; margin: .8rem 0 .4rem; font-size: .9rem; }
  .count { font-size: .7rem; padding: 0 .4rem; border-radius: 999px; background: color-mix(in srgb, currentColor 12%, transparent); }
  .chips { display: flex; flex-wrap: wrap; gap: .4rem; }
  .facts { display: grid; grid-template-columns: repeat(auto-fit, minmax(240px, 1fr)); gap: .6rem 1.5rem; margin: .9rem 0 0; }
  .facts div { display: flex; flex-direction: column; gap: .15rem; }
  .facts dt { font-size: .75rem; text-transform: uppercase; letter-spacing: .03em; opacity: .6; }
  .facts dd { margin: 0; font-size: .87rem; word-break: break-all; }
  .tabs, .subtabs { display: flex; gap: .25rem; flex-wrap: wrap; }
  .tabs button, .subtabs button { padding: .4rem .8rem; border: 1px solid var(--border, #ccc); border-radius: 6px; background: transparent; color: inherit; cursor: pointer; font-size: .85rem; display: inline-flex; align-items: center; gap: .35rem; }
  .tabs button.active, .subtabs button.active { background: color-mix(in srgb, currentColor 10%, transparent); font-weight: 600; }
  .subtabs { margin-top: .6rem; }
  .subtabs button { font-size: .8rem; padding: .3rem .65rem; }
  .bar, .run-bar, .actions { display: flex; gap: .5rem; align-items: center; flex-wrap: wrap; }
  .actions { margin-top: .6rem; }
  .grow { flex: 1; }
  .bar select, .run-bar select, .mini { padding: .4rem .6rem; border-radius: 6px; border: 1px solid var(--border, #ccc); background: inherit; color: inherit; font-size: .85rem; }
  .mini { width: 12rem; }
  .mini.num { width: 5.5rem; }
  .gate { display: flex; gap: .5rem; margin-top: .75rem; padding: .6rem .8rem; border-radius: 6px; background: color-mix(in srgb, crimson 12%, transparent); font-size: .85rem; }
  .run-list, .tbl-list, .ticket-list, .entity-list, .round-list, .warnings { list-style: none; padding: 0; margin: 0; display: flex; flex-direction: column; gap: .6rem; }
  .ticket-list li, .round-list li { display: flex; flex-wrap: wrap; align-items: center; gap: .5rem; font-size: .85rem; }
  .ticket-list li.closed { opacity: .6; }
  .warnings { margin-top: .5rem; gap: .25rem; font-size: .8rem; }
  .warnings li { display: flex; gap: .4rem; align-items: flex-start; }
  .run-card { border: 1px solid var(--border, #ddd); border-radius: 8px; padding: .7rem .9rem; }
  .run-card.prod { border-color: color-mix(in srgb, green 45%, var(--border, #ddd)); }
  .run-head, .run-meta, .run-actions, .map-head { display: flex; flex-wrap: wrap; align-items: center; gap: .5rem; }
  .run-meta { font-size: .8rem; opacity: .85; margin-top: .35rem; }
  .run-actions { margin-top: .5rem; }
  .run-error { margin-top: .4rem; font-size: .8rem; color: crimson; }
  .status { font-size: .72rem; padding: .1rem .45rem; border-radius: 999px; display: inline-flex; align-items: center; gap: .25rem; }
  .status-succeeded { background: color-mix(in srgb, green 16%, transparent); }
  .status-rejected { background: color-mix(in srgb, orange 20%, transparent); }
  .status-failed { background: color-mix(in srgb, crimson 16%, transparent); }
  .graph { font-size: .76rem; opacity: .8; }
  .chip { font-size: .7rem; padding: .1rem .4rem; border-radius: 999px; border: 1px solid var(--border, #ccc); display: inline-flex; align-items: center; gap: .25rem; }
  .chip-tiny { font-size: .66rem; padding: 0 .35rem; }
  .chip em { font-style: normal; opacity: .6; }
  .chip-ok { background: color-mix(in srgb, green 14%, transparent); }
  .chip-warn { background: color-mix(in srgb, orange 18%, transparent); }
  .chip-assist { background: color-mix(in srgb, rebeccapurple 16%, transparent); }
  .tbl-head { display: flex; align-items: center; flex-wrap: wrap; gap: .5rem; width: 100%; background: none; border: 0; color: inherit; cursor: pointer; padding: 0; text-align: left; }
  .cols { display: flex; flex-wrap: wrap; gap: .3rem; margin-top: .5rem; }
  .col { font-size: .75rem; padding: .1rem .4rem; border-radius: 4px; border: 1px solid var(--border, #ddd); display: inline-flex; align-items: center; gap: .25rem; }
  .col.pk { border-color: color-mix(in srgb, goldenrod 60%, var(--border, #ddd)); }
  .col em { opacity: .55; font-style: normal; }
  .fk { margin-top: .5rem; display: flex; flex-wrap: wrap; gap: .5rem; align-items: center; }
  .edge { font-family: monospace; }
  .small { font-size: .78rem; }
  .mono { font-family: monospace; }
  .scroll { overflow-x: auto; margin-top: .5rem; }
  .preview, .stats, .matrix { border-collapse: collapse; font-size: .78rem; width: 100%; }
  .preview th, .preview td, .stats th, .stats td, .matrix th, .matrix td { border: 1px solid var(--border, #ddd); padding: .2rem .45rem; text-align: left; vertical-align: top; }
  .preview td { white-space: nowrap; }
  .stats td.values { display: flex; flex-wrap: wrap; gap: .2rem; border: 0; }
  .preview-note { margin-top: .5rem; }
  .rml { margin-top: .4rem; max-height: 22rem; overflow: auto; font-size: .75rem; padding: .6rem; border-radius: 6px; background: color-mix(in srgb, currentColor 6%, transparent); }
  .code { width: 100%; font-family: monospace; font-size: .8rem; padding: .6rem; border-radius: 6px; border: 1px solid var(--border, #ccc); background: inherit; color: inherit; box-sizing: border-box; }
  .switch { display: inline-flex; align-items: center; gap: .35rem; }
  .two { display: grid; grid-template-columns: repeat(auto-fit, minmax(320px, 1fr)); gap: 1rem; }
  .finding { border-left: 3px solid var(--border, #ccc); padding: .3rem .6rem; margin-bottom: .5rem; }
  .finding.defect { border-color: crimson; }
  .finding.issue { border-color: orange; }
  .bad-h { color: crimson; }
  .drift-table { margin-top: .5rem; }
  .ok { color: green; display: flex; align-items: center; gap: .3rem; }
  .warn { color: darkorange; display: flex; align-items: center; gap: .3rem; }
  .placeholder { text-align: center; padding: 1.5rem; }
  .dim { opacity: .7; }
  .bad { color: crimson; }
  .danger { color: crimson; }
</style>
