<script>
  // "Sources" workspace, step 1 — Connect. Register a SQL datasource, test the
  // connection, and see at a glance whether it is allowlisted, read-only, and
  // whether model assistance is allowed for it.
  //
  // There is deliberately no password field anywhere on this page: a
  // credential is a REFERENCE into a secret store (env:, file:, vault:), and
  // the API returns the reference, never a value.
  import { onMount } from 'svelte';
  import { t } from 'svelte-i18n';
  import { Database, Plug, Loader2, ShieldCheck, ShieldAlert, Lock, Sparkles, Play, AlertTriangle, Check, SlidersHorizontal } from 'lucide-svelte';
  import { listSources, createSource, testSource, sourceMetrics, getMappingGates, updateMappingGates } from '../lib/api.js';
  import { Link, navigate } from '../lib/router/index.js';
  import { isAdmin, authInitialized } from '../lib/stores.js';
  import { toastError, toastSuccess } from '../lib/toast.ts';

  let sources = [];
  let metrics = null;
  let loading = true;
  let showForm = false;
  let testing = false;
  let saving = false;
  let testResult = null;

  // The mapping gates: the thresholds a proposal is judged by, a config graph
  // the proposer reads and an administrator edits here.
  let gates = null;
  let showGates = false;
  let gatesForm = null;
  let savingGates = false;
  const GATE_FIELDS = [
    'autoThreshold', 'reviewThreshold', 'datatypeMismatchCap', 'ambiguityMargin',
    'enumMatchMinimum', 'systematicShare', 'systematicMinSubjects', 'driftKlThreshold',
  ];
  const LEXICAL_FIELDS = ['nameWeight', 'commentWeight', 'typeWeight', 'minimumScore'];

  async function loadGates() {
    try {
      gates = await getMappingGates();
      gatesForm = { ...gates, lexical: { ...gates.lexical } };
    } catch (e) {
      toastError(e.message);
    }
  }

  async function saveGates() {
    savingGates = true;
    try {
      const patch = {};
      for (const f of GATE_FIELDS) patch[f] = Number(gatesForm[f]);
      patch.lexical = {};
      for (const f of LEXICAL_FIELDS) patch.lexical[f] = Number(gatesForm.lexical[f]);
      gates = await updateMappingGates(patch);
      gatesForm = { ...gates, lexical: { ...gates.lexical } };
      toastSuccess($t('pages.sources.gatesSaved'));
    } catch (e) {
      toastError(e.message);
    } finally {
      savingGates = false;
    }
  }

  const EMPTY = {
    id: '', name: '', dialect: 'sqlite', host: '', port: null, database: '',
    username: '', credential: '', readOnly: true, statementTimeoutMs: 30000,
    watermarkColumn: '', allowModelAssist: false, tls: false, dataset: '', optionsText: '',
  };
  let form = { ...EMPTY };
  const OPTIONS_EXAMPLE = 'sslrootcert=/run/secrets/db-ca.pem\nsearch_path=legacy';

  /** `key=value` per line → the datasource's `options` map; blanks ignored. */
  function parseOptions(text) {
    const options = {};
    for (const line of String(text ?? '').split('\n')) {
      const at = line.indexOf('=');
      if (at <= 0) continue;
      const key = line.slice(0, at).trim();
      const value = line.slice(at + 1).trim();
      if (key && value) options[key] = value;
    }
    return options;
  }

  let _guardChecked = false;
  $: if ($authInitialized && !_guardChecked) {
    _guardChecked = true;
    if (!$isAdmin) navigate('/');
  }

  onMount(load);

  async function load() {
    loading = true;
    try {
      sources = await listSources();
      metrics = await sourceMetrics().catch(() => null);
    } catch (e) {
      toastError(e.message);
    } finally {
      loading = false;
    }
  }

  /** Only the fields the API accepts, with blanks dropped. */
  function payload() {
    const body = { ...form };
    for (const key of ['name', 'host', 'username', 'credential', 'watermarkColumn', 'dataset']) {
      if (!String(body[key] ?? '').trim()) delete body[key];
    }
    if (!body.port) delete body.port;
    const options = parseOptions(body.optionsText);
    delete body.optionsText;
    if (Object.keys(options).length) body.options = options;
    return body;
  }

  async function runTest() {
    testing = true;
    testResult = null;
    try {
      testResult = await testSource(payload());
    } catch (e) {
      testResult = { ok: false, error: e.message };
    } finally {
      testing = false;
    }
  }

  async function save() {
    saving = true;
    try {
      const created = await createSource(payload());
      toastSuccess($t('pages.sources.registered', { values: { id: created.id } }));
      showForm = false;
      form = { ...EMPTY };
      testResult = null;
      await load();
    } catch (e) {
      toastError(e.message);
    } finally {
      saving = false;
    }
  }

  const fileBacked = (dialect) => dialect === 'sqlite';
</script>

<div class="sources-page">
  <div class="card toolbar">
    <h2><Database size={18} /> {$t('pages.sources.heading')}</h2>
    <p class="dim">{$t('pages.sources.intro')}</p>
    <div class="toolbar-cta">
      <button class="btn" on:click={() => (showForm = !showForm)}>
        {showForm ? $t('pages.sources.cancel') : $t('pages.sources.connectSource')}
      </button>
    </div>
  </div>

  {#if metrics}
    <div class="card stats">
      <div><span class="stat-n">{metrics.sources}</span><span class="stat-l">{$t('pages.sources.statSources')}</span></div>
      <div><span class="stat-n">{metrics.mappings}</span><span class="stat-l">{$t('pages.sources.statMappings')}</span></div>
      <div><span class="stat-n">{metrics.runs?.total ?? 0}</span><span class="stat-l">{$t('pages.sources.statRuns')}</span></div>
      <div><span class="stat-n">{(metrics.triplesProduced ?? 0).toLocaleString()}</span><span class="stat-l">{$t('pages.sources.statTriples')}</span></div>
    </div>
  {/if}

  <div class="card gates">
    <button class="gates-head" on:click={() => { showGates = !showGates; if (showGates && !gates) loadGates(); }} aria-expanded={showGates}>
      <SlidersHorizontal size={15} /> <strong>{$t('pages.sources.gatesHeading')}</strong>
      {#if gates}<span class="chip">{$t(`pages.sources.gatesSource_${gates.source}`)}</span>{/if}
    </button>
    {#if showGates}
      <p class="hint">{$t('pages.sources.gatesIntro')}</p>
      {#if !gatesForm}
        <div class="placeholder"><Loader2 size={18} class="spin" /></div>
      {:else}
        <form class="form" on:submit|preventDefault={saveGates}>
          <div class="grid">
            {#each GATE_FIELDS as f (f)}
              <label>{$t(`pages.sources.gate_${f}`)}
                <input type="number" step={f === 'systematicMinSubjects' ? '1' : '0.01'} min="0" max={f === 'systematicMinSubjects' || f === 'driftKlThreshold' ? undefined : '1'} bind:value={gatesForm[f]} required />
              </label>
            {/each}
            {#each LEXICAL_FIELDS as f (f)}
              <label>{$t(`pages.sources.gateLexical_${f}`)}
                <input type="number" step="0.01" min="0" max="1" bind:value={gatesForm.lexical[f]} required />
              </label>
            {/each}
          </div>
          <div class="actions">
            <button type="submit" class="btn" disabled={savingGates}>
              {#if savingGates}<Loader2 size={14} class="spin" />{/if} {$t('pages.sources.gatesSave')}
            </button>
          </div>
        </form>
      {/if}
    {/if}
  </div>

  {#if showForm}
    <form class="card form" on:submit|preventDefault={save}>
      <h3>{$t('pages.sources.connectHeading')}</h3>

      <div class="grid">
        <label>{$t('pages.sources.fieldId')}
          <input bind:value={form.id} required placeholder="legacy-assets" />
        </label>
        <label>{$t('pages.sources.fieldName')}
          <input bind:value={form.name} placeholder={$t('pages.sources.fieldNamePlaceholder')} />
        </label>
        <label>{$t('pages.sources.fieldDialect')}
          <select bind:value={form.dialect}>
            <option value="sqlite">SQLite</option>
            <option value="postgresql">PostgreSQL</option>
            <option value="mysql">MySQL</option>
            <option value="mssql">SQL Server</option>
            <option value="sparql">{$t('pages.sources.dialectSparql')}</option>
          </select>
        </label>
        {#if !fileBacked(form.dialect)}
          <label>{$t('pages.sources.fieldHost')}
            <input bind:value={form.host} placeholder="db.internal" />
          </label>
          <label>{$t('pages.sources.fieldPort')}
            <input type="number" bind:value={form.port} placeholder="5432" />
          </label>
          <label>{$t('pages.sources.fieldUsername')}
            <input bind:value={form.username} placeholder="reader" />
          </label>
        {/if}
        <label class="wide">{fileBacked(form.dialect) ? $t('pages.sources.fieldPath') : form.dialect === 'sparql' ? $t('pages.sources.fieldEndpointPath') : $t('pages.sources.fieldDatabase')}
          <input bind:value={form.database} required={form.dialect !== 'sparql'} placeholder={form.dialect === 'sparql' ? '/sparql' : ''} />
        </label>
        <label class="wide">{$t('pages.sources.fieldCredential')}
          <input bind:value={form.credential} placeholder="vault:secret/data/sources/legacy#password" />
          <span class="hint">{$t('pages.sources.credentialHint')}</span>
        </label>
        <label>{$t('pages.sources.fieldTimeout')}
          <input type="number" bind:value={form.statementTimeoutMs} min="1" required />
        </label>
        <label>{$t('pages.sources.fieldWatermark')}
          <input bind:value={form.watermarkColumn} placeholder="updated_at" />
        </label>
        <label>{$t('pages.sources.fieldDataset')}
          <input bind:value={form.dataset} placeholder={$t('pages.sources.fieldDatasetPlaceholder')} />
        </label>
        {#if !fileBacked(form.dialect)}
          <label class="wide">{$t('pages.sources.fieldOptions')}
            <textarea rows="2" bind:value={form.optionsText} placeholder={OPTIONS_EXAMPLE}></textarea>
            <span class="hint">{$t('pages.sources.optionsHint')}</span>
          </label>
        {/if}
      </div>

      <div class="switches">
        <label class="switch"><input type="checkbox" checked disabled />
          <span><Lock size={13} /> {$t('pages.sources.readOnlyAlways')}</span>
        </label>
        <label class="switch"><input type="checkbox" bind:checked={form.allowModelAssist} />
          <span><Sparkles size={13} /> {$t('pages.sources.allowModelAssist')}</span>
        </label>
        {#if !fileBacked(form.dialect)}
          <label class="switch"><input type="checkbox" bind:checked={form.tls} />
            <span>{$t('pages.sources.useTls')}</span>
          </label>
        {/if}
      </div>
      <p class="hint">{$t('pages.sources.modelAssistHint')}</p>

      {#if testResult}
        <div class="probe" class:ok={testResult.ok} class:bad={!testResult.ok}>
          {#if testResult.ok}
            <Check size={14} /> {$t('pages.sources.probeOk', { values: { tables: testResult.tables ?? 0, version: testResult.serverVersion || '—' } })}
          {:else}
            <AlertTriangle size={14} /> {testResult.error}
          {/if}
        </div>
      {/if}

      <div class="actions">
        <button type="button" class="btn btn-ghost" on:click={runTest} disabled={testing || !form.database}>
          {#if testing}<Loader2 size={14} class="spin" />{:else}<Plug size={14} />{/if}
          {$t('pages.sources.testConnection')}
        </button>
        <button type="submit" class="btn" disabled={saving}>{$t('pages.sources.register')}</button>
      </div>
    </form>
  {/if}

  {#if loading}
    <div class="card placeholder"><Loader2 size={24} class="spin" /><p>{$t('pages.sources.loading')}</p></div>
  {:else if sources.length === 0}
    <div class="card placeholder">
      <Database size={36} strokeWidth={1.2} />
      <h3>{$t('pages.sources.emptyHeading')}</h3>
      <p>{$t('pages.sources.emptyDesc')}</p>
    </div>
  {:else}
    <ul class="src-list">
      {#each sources as s (s.id)}
        <li class="src-card">
          <div class="src-head">
            <Link to={`/sources/${s.id}`} class="src-name">{s.name}</Link>
            <span class="chip">{s.dialect}</span>
            {#if s.readOnly}<span class="chip chip-ok"><Lock size={10} /> {$t('pages.sources.chipReadOnly')}</span>{/if}
            {#if s.allowlisted}
              <span class="chip chip-ok"><ShieldCheck size={10} /> {$t('pages.sources.chipAllowlisted')}</span>
            {:else}
              <span class="chip chip-warn"><ShieldAlert size={10} /> {$t('pages.sources.chipNotAllowlisted')}</span>
            {/if}
            {#if s.allowModelAssist}<span class="chip chip-assist"><Sparkles size={10} /> {$t('pages.sources.chipModelAssist')}</span>{/if}
          </div>
          <div class="src-meta">
            <span>{s.host ? `${s.host}${s.port ? `:${s.port}` : ''} · ` : ''}{s.database}</span>
            {#if s.credential}<code class="ref" title={$t('pages.sources.credentialRefTitle')}>{s.credential}</code>{/if}
          </div>
          <div class="src-prod">
            {#if s.production}
              <Play size={12} /> {$t('pages.sources.servingFrom')} <code>{s.production.graph}</code>
            {:else}
              <span class="dim">{$t('pages.sources.neverRun')}</span>
            {/if}
          </div>
        </li>
      {/each}
    </ul>
  {/if}
</div>

<style>
  .sources-page { display: flex; flex-direction: column; gap: 1rem; }
  .toolbar h2 { display: flex; align-items: center; gap: .5rem; margin: 0 0 .25rem; }
  .toolbar-cta { margin-top: .75rem; }
  .stats { display: flex; flex-wrap: wrap; gap: 2rem; }
  .stats div { display: flex; flex-direction: column; }
  .stat-n { font-size: 1.5rem; font-weight: 600; }
  .stat-l { font-size: .8rem; opacity: .7; }
  .form .grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(220px, 1fr)); gap: .75rem; }
  .form label { display: flex; flex-direction: column; gap: .25rem; font-size: .85rem; }
  .form label.wide { grid-column: 1 / -1; }
  .form input, .form select, .form textarea { padding: .45rem .6rem; border-radius: 6px; border: 1px solid var(--border, #ccc); background: inherit; color: inherit; font: inherit; }
  .hint { font-size: .78rem; opacity: .7; margin: .35rem 0 0; }
  .switches { display: flex; flex-wrap: wrap; gap: 1rem; margin-top: .75rem; }
  .switch { display: flex; align-items: center; gap: .4rem; font-size: .85rem; }
  .switch span { display: inline-flex; align-items: center; gap: .3rem; }
  .actions { display: flex; gap: .5rem; margin-top: 1rem; }
  .probe { margin-top: .75rem; padding: .5rem .7rem; border-radius: 6px; font-size: .85rem; display: flex; align-items: center; gap: .4rem; }
  .probe.ok { background: color-mix(in srgb, green 12%, transparent); }
  .probe.bad { background: color-mix(in srgb, crimson 12%, transparent); }
  .src-list { list-style: none; padding: 0; margin: 0; display: flex; flex-direction: column; gap: .6rem; }
  .src-card { border: 1px solid var(--border, #ddd); border-radius: 8px; padding: .75rem 1rem; }
  .src-head { display: flex; flex-wrap: wrap; align-items: center; gap: .5rem; }
  .src-meta, .src-prod { font-size: .82rem; opacity: .85; margin-top: .35rem; display: flex; flex-wrap: wrap; align-items: center; gap: .5rem; }
  .chip { font-size: .7rem; padding: .1rem .4rem; border-radius: 999px; border: 1px solid var(--border, #ccc); display: inline-flex; align-items: center; gap: .25rem; }
  .chip-ok { background: color-mix(in srgb, green 14%, transparent); }
  .chip-warn { background: color-mix(in srgb, orange 18%, transparent); }
  .chip-assist { background: color-mix(in srgb, rebeccapurple 16%, transparent); }
  .ref { font-size: .75rem; opacity: .8; }
  .placeholder { text-align: center; padding: 2rem; }
  .dim { opacity: .7; }
  .gates-head { display: flex; align-items: center; gap: .5rem; width: 100%; background: none; border: 0; color: inherit; cursor: pointer; padding: 0; text-align: left; font-size: .95rem; }
  .gates .form { margin-top: .75rem; }
</style>
