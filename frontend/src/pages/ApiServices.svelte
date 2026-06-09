<script>
  import { onMount } from 'svelte';
  import {
    listSavedQueries, getSavedQuery, createSavedQuery, updateSavedQuery, deleteSavedQuery,
    listSavedQueryRevisions, listSavedQueryTests, acknowledgeSavedQueryTest,
    repairSavedQuery, runSavedQuery, savedQueryOpenApiUrl, getDataset, getOrganisation,
    sparqlQuery, nlToSparql, llmHealth, sendLlmFeedback, browseSuggest,
  } from '../lib/api.js';
  import { extractDeclaredPrefixes } from '../lib/ontology/prefixService.js';
  import { formatSparql } from '../lib/ontology/sparqlFormat.js';
  import { highlight, highlightRdf, prettyJson, prettyXml } from '../lib/resultHighlight.js';
  import { downloadFile } from '../lib/rdf-utils.js';
  import { toastSuccess, toastError } from '../lib/toast';
  import { copyToClipboard } from '../lib/clipboard.js';
  import { isAuthenticated } from '../lib/stores.js';
  import { t as i18nT } from 'svelte-i18n';
  import { Link, navigate } from '../lib/router/index.js';
  import PageHeader from '../components/PageHeader.svelte';
  import SparqlEditorCM from '../components/SparqlEditorCM.svelte';
  import Select from '../components/Select.svelte';
  import Combobox from '../components/Combobox.svelte';
  import RdfTerm from '../components/RdfTerm.svelte';
  import { Bookmark, Play, Plus, Trash2, Wand2, Check, FileCode, BookOpen, Loader2,
           ChevronRight, ChevronDown, Database, Layers, History, Download, Pencil, X, Copy, GitCommit } from 'lucide-svelte';

  // Exactly one of these is set by the route, selecting the scope.
  export let datasetId = null;
  export let orgId = null;
  export let groupId = null;

  const PARAM_TYPES = ['iri', 'string', 'integer', 'decimal', 'boolean', 'date', 'dateTime'];
  const XSD = 'http://www.w3.org/2001/XMLSchema#';
  // Return formats offered when running a service. `accept` drives content
  // negotiation; results are rendered by the *actual* returned content-type, so a
  // SELECT/ASK asked for an RDF format (which the backend answers as SPARQL JSON)
  // still renders as a table.
  const TABULAR_FORMATS = [
    { value: 'json', label: 'SPARQL JSON', accept: 'application/sparql-results+json' },
    { value: 'csv',  label: 'CSV',         accept: 'text/csv' },
    { value: 'tsv',  label: 'TSV',         accept: 'text/tab-separated-values' },
    { value: 'xml',  label: 'SPARQL XML',  accept: 'application/sparql-results+xml' },
  ];
  const RDF_FORMATS = [
    { value: 'turtle',   label: 'Turtle',    accept: 'text/turtle' },
    { value: 'jsonld',   label: 'JSON-LD',   accept: 'application/ld+json' },
    { value: 'ntriples', label: 'N-Triples', accept: 'application/n-triples' },
    { value: 'rdfxml',   label: 'RDF/XML',   accept: 'application/rdf+xml' },
  ];
  const FORMATS = [...TABULAR_FORMATS, ...RDF_FORMATS];
  const FORMAT_EXT = { json: 'json', csv: 'csv', tsv: 'tsv', xml: 'xml', turtle: 'ttl', jsonld: 'jsonld', ntriples: 'nt', rdfxml: 'rdf' };
  const PLURAL = { dataset: 'datasets', organisation: 'organisations', group: 'groups' };

  /** @type {import('../lib/api').SavedQueryScope} */
  $: scope = datasetId ? 'datasets' : orgId ? 'organisations' : 'groups';
  $: ownerId = datasetId ?? orgId ?? groupId ?? '';
  $: scopeLabel = datasetId ? 'dataset' : orgId ? 'organisation' : 'group';
  $: editorPath = datasetId ? `/datasets/${datasetId}/sparql` : orgId ? `/organisations/${orgId}/sparql` : '/sparql';
  $: docsParam = datasetId ? `dataset=${ownerId}` : orgId ? `organisation=${ownerId}` : `group=${ownerId}`;
  $: breadcrumbs = datasetId
    ? [{ label: $i18nT('pages.apiServices.crumbDatasets'), href: '/datasets' }, { label: ownerName ?? '…', href: '/datasets/' + datasetId }, { label: $i18nT('pages.apiServices.title') }]
    : orgId
      ? [{ label: $i18nT('pages.apiServices.crumbOrganisations'), href: '/organisations' }, { label: ownerName ?? '…', href: '/organisations/' + orgId }, { label: $i18nT('pages.apiServices.title') }]
      : [{ label: $i18nT('pages.apiServices.title') }];

  let ownerName = ownerId;
  let queries = [];
  let listCanWrite = false;
  let loading = true;
  let error = '';

  // Page-level write (drives the "New API service" button). Per-service editing
  // uses each query's own can_write, since an org page may list a dataset service
  // the caller cannot edit.
  $: canWrite = listCanWrite && $isAuthenticated;
  const qCanWrite = (q) => (q.can_write ?? listCanWrite) && $isAuthenticated;
  // A listed query keeps its own scope/owner; route per-query so dataset services
  // surfaced under an org still hit the right endpoint.
  const qScope = (q) => PLURAL[q.scope] ?? scope;
  const qOwner = (q) => q.owner_id ?? ownerId;

  // ── Open cards (multiple at once), keyed by query id ─────────────────────────
  let openIds = new Set();
  let cards = {}; // id -> { detail, run, revRev }

  // ── Create/edit form (modal) ─────────────────────────────────────────────────
  let showForm = false;
  let editingSlug = null;
  /** @type {import('../lib/api').SavedQueryScope} */
  let editingScope = 'datasets';
  let editingOwner = '';
  let form = blankForm();

  // AI generation + live test-run inside the form.
  let llmStatus = null;
  let nlQuestion = '';
  let nlLoading = false;
  let nlError = '';
  let lastGenerated = null;
  let testRunning = false;
  let testResult = null;
  let testError = '';

  function blankForm() {
    return { name: '', description: '', sparql: 'SELECT * WHERE { ?s ?p ?o } LIMIT 100', parameters: [], test_parameters_text: '', version_name: '', note: '' };
  }
  // Commit message of the service's current head revision, shown in edit mode.
  let lastCommit = null;
  function resetFormExtras() { nlQuestion = ''; nlError = ''; lastGenerated = null; testResult = null; testError = ''; lastCommit = null; }
  async function loadLastCommit(scp, owner, slug, rev) {
    try {
      const res = await listSavedQueryRevisions(scp, owner, slug);
      const list = res.revisions ?? [];
      const head = list.find((r) => r.revision === rev) ?? list[0] ?? null;
      lastCommit = head ? { revision: head.revision, name: head.name, note: head.note, origin: head.origin, created_at: head.created_at } : null;
    } catch { lastCommit = null; }
  }

  async function load() {
    loading = true; error = '';
    try {
      const res = await listSavedQueries(scope, ownerId);
      queries = res.queries ?? [];
      listCanWrite = res.can_write ?? false;
    } catch (e) {
      error = e.message;
    } finally {
      loading = false;
    }
  }

  onMount(async () => {
    if (datasetId) { try { ownerName = (await getDataset(datasetId))?.name ?? datasetId; } catch { ownerName = datasetId; } }
    else if (orgId) { try { ownerName = (await getOrganisation(orgId))?.name ?? orgId; } catch { ownerName = orgId; } }
    else ownerName = groupId;

    llmHealth().then((s) => { llmStatus = s; }).catch(() => {});
    await load();

    // Re-open the services named in ?open= (used to return from the SPARQL editor).
    try {
      const want = new URLSearchParams(window.location.search).get('open');
      if (want) {
        const wanted = want.split(',').map(decodeURIComponent).filter(Boolean);
        for (const key of wanted) {
          const q = queries.find((x) => x.id === key || x.slug === key);
          if (q) await openCard(q);
        }
      }
    } catch {}

    // Prefill from the SPARQL editor "Save as API service" handoff (writers only).
    try {
      const prefill = sessionStorage.getItem('ots_sq_prefill');
      if (prefill) {
        sessionStorage.removeItem('ots_sq_prefill');
        if (listCanWrite) { openCreate(); form.sparql = prefill; form = form; }
      }
    } catch {}
  });

  // ── Open / detail ────────────────────────────────────────────────────────────
  function syncOpenUrl() {
    try {
      const url = new URL(window.location.href);
      const ids = [...openIds];
      if (ids.length) url.searchParams.set('open', ids.join(',')); else url.searchParams.delete('open');
      history.replaceState(history.state, '', url);
    } catch {}
  }

  async function toggleCard(q) {
    if (openIds.has(q.id)) { openIds.delete(q.id); openIds = new Set(openIds); syncOpenUrl(); return; }
    await openCard(q);
  }

  async function openCard(q) {
    openIds.add(q.id); openIds = new Set(openIds); syncOpenUrl();
    if (!cards[q.id]) {
      const values = {};
      (q.parameters ?? []).forEach((p) => { values[p.name] = p.default ?? ''; });
      cards[q.id] = {
        detail: { tests: [], revisions: [], loading: true },
        run: { values, version: '', format: 'json', viewMode: null, result: null, raw: '', contentType: '', versionServed: null, running: false, error: '', noResult: false, repairing: false, repairSuggestion: null, suggest: {} },
        revRev: null,
      };
      cards = cards;
    }
    await loadDetail(q);
  }

  async function loadDetail(q) {
    const c = cards[q.id]; if (!c) return;
    c.detail = { ...c.detail, loading: true }; cards = cards;
    try {
      const [full, tests, revisions] = await Promise.all([
        getSavedQuery(qScope(q), qOwner(q), q.slug).catch(() => null),
        listSavedQueryTests(qScope(q), qOwner(q), q.slug),
        listSavedQueryRevisions(qScope(q), qOwner(q), q.slug),
      ]);
      if (full?.sparql != null) q.sparql = full.sparql;
      // Make sure every declared/detected param has a run input value.
      const vals = { ...c.run.values };
      for (const p of paramSpecsFor(q)) if (!(p.name in vals)) vals[p.name] = p.default ?? '';
      // Align the default format with the query form (SELECT→json, CONSTRUCT→turtle…)
      // so the picker only offers — and starts on — an applicable serialisation.
      const form = queryForm(q.sparql);
      let format = c.run.format;
      if (isRdfForm(form) && TABULAR_FORMATS.some((f) => f.value === format)) format = defaultFormatFor(form);
      else if (isTabularForm(form) && RDF_FORMATS.some((f) => f.value === format)) format = defaultFormatFor(form);
      c.run = { ...c.run, values: vals, format };
      c.detail = { tests: tests.tests ?? [], revisions: revisions.revisions ?? [], loading: false };
      cards = cards;
    } catch (e) {
      c.detail = { tests: [], revisions: [], loading: false }; cards = cards; toastError(e.message);
    }
  }

  // ── Parameters: declared + auto-detected {{placeholders}} ────────────────────
  function detectPlaceholders(sparql) {
    const names = []; const re = /\{\{\s*([A-Za-z_][\w-]*)\s*\}\}/g; let m;
    while ((m = re.exec(sparql || ''))) if (!names.includes(m[1])) names.push(m[1]);
    return names;
  }
  function paramSpecsFor(q) {
    const declared = q.parameters ?? [];
    const seen = new Set(declared.map((p) => p.name));
    const out = [...declared];
    for (const n of detectPlaceholders(q.sparql)) if (!seen.has(n)) out.push({ name: n, type: 'string', required: true });
    return out;
  }

  // ── Run ──────────────────────────────────────────────────────────────────────
  async function runQuery(q) {
    const c = cards[q.id]; if (!c) return;
    c.run = { ...c.run, running: true, result: null, raw: '', error: '', versionServed: null, noResult: false }; cards = cards;
    try {
      const fmt = FORMATS.find((f) => f.value === c.run.format) ?? FORMATS[0];
      const { data, versionServed, raw, contentType } = await runSavedQuery(qScope(q), qOwner(q), q.slug, c.run.values, c.run.version || null, fmt.accept);
      // "No result" depends on shape: ASK is never empty; an RDF/text body is
      // empty only when blank; tabular results are empty with zero bindings.
      let noResult;
      if (data?.boolean !== undefined) noResult = false;
      else if (data?.raw !== undefined) noResult = (raw || '').trim().length === 0;
      else noResult = (data?.results?.bindings?.length ?? 0) === 0;
      c.run = { ...c.run, result: data, raw, contentType, versionServed, running: false, noResult };
      cards = cards;
    } catch (e) {
      c.run = { ...c.run, running: false, error: e.message }; cards = cards;
    }
  }

  // The public, runnable endpoint for a service (handy to copy from the run panel).
  const runPath = (q) => `/api/${qScope(q)}/${encodeURIComponent(qOwner(q))}/api-services/${encodeURIComponent(q.slug)}/run`;
  async function copyText(text) {
    if (await copyToClipboard(text)) toastSuccess($i18nT('system.copied'));
    else toastError($i18nT('pages.apiServices.copyFailed'));
  }

  function downloadRun(q) {
    const c = cards[q.id]; if (!c?.run?.raw) return;
    const ext = FORMAT_EXT[c.run.format] ?? 'txt';
    downloadFile(c.run.raw, `${q.slug}.${ext}`, c.run.contentType || 'text/plain');
  }

  // A service is "broken" (and worth offering an LLM fix) only when its latest
  // version test errored, or the last run errored / returned nothing.
  function serviceBroken(q) {
    const c = cards[q.id]; if (!c) return false;
    const testErr = (c.detail.tests || []).some((t) => t.status === 'error' && !t.acknowledged);
    return testErr || !!c.run.error || c.run.noResult;
  }

  // ── Smart variable autocomplete (dataset + predicate aware) ──────────────────
  function predicateContext(q, name) {
    const sparql = q.sparql || '';
    const ph = `{{${name}}}`;
    const idx = sparql.indexOf(ph);
    if (idx < 0) return null;
    const declared = extractDeclaredPrefixes(sparql);
    const expand = (tok) => {
      if (!tok) return null;
      if (tok.startsWith('<') && tok.endsWith('>')) return tok.slice(1, -1);
      const m = tok.match(/^([A-Za-z_][\w-]*):([A-Za-z0-9_.%/-]*)$/);
      if (m && declared[m[1]]) return declared[m[1]] + m[2];
      return null;
    };
    const before = sparql.slice(Math.max(0, idx - 200), idx);
    const after = sparql.slice(idx + ph.length, idx + ph.length + 200);
    const bTok = (before.match(/(\S+)\s*$/) || [])[1];
    const pBefore = expand(bTok);
    if (pBefore) return { predicate: pBefore, field: 'object' };
    const aTok = (after.match(/^\s*(\S+)/) || [])[1];
    const pAfter = expand(aTok);
    if (pAfter) return { predicate: pAfter, field: 'subject' };
    return null;
  }

  const suggestTimers = {};
  function requestSuggest(q, param, prefix) {
    const key = `${q.id}|${param.name}`;
    clearTimeout(suggestTimers[key]);
    suggestTimers[key] = setTimeout(() => loadSuggest(q, param, prefix), 250);
  }
  async function loadSuggest(q, param, prefix = '') {
    const c = cards[q.id]; if (!c) return;
    if (param.type && !['iri', 'string'].includes(param.type)) return;
    const ctx = predicateContext(q, param.name);
    // No predicate context: IRI params usually fill a subject slot (always IRIs),
    // other types fill objects (literals).
    const field = ctx?.field ?? (param.type === 'iri' ? 'subject' : 'object');
    const dataset = q.scope === 'dataset' ? q.owner_id : (datasetId ?? null);
    try {
      const res = await browseSuggest(field, prefix, 20, { dataset, predicate: ctx?.predicate ?? null });
      const vals = (res?.values ?? []).map((v) => (typeof v === 'string' ? v : v?.value)).filter(Boolean);
      c.run.suggest = { ...c.run.suggest, [param.name]: vals }; cards = cards;
    } catch {}
  }

  // ── Open in the standalone SPARQL editor (with a return link) ─────────────────
  async function openInEditor(q) {
    let sparql = q.sparql;
    if (!sparql) { try { sparql = (await getSavedQuery(qScope(q), qOwner(q), q.slug)).sparql; } catch {} }
    try { sessionStorage.setItem('ots_sparql_load', sparql ?? ''); } catch {}
    const ret = `${window.location.pathname}?open=${encodeURIComponent(q.id)}`;
    const ep = q.scope === 'dataset' ? `/datasets/${q.owner_id}/sparql` : editorPath;
    const sep = ep.includes('?') ? '&' : '?';
    navigate(`${ep}${sep}return=${encodeURIComponent(ret)}&from=${encodeURIComponent(q.name)}`);
  }

  // ── LLM repair ────────────────────────────────────────────────────────────────
  async function fixWithLlm(q) {
    const c = cards[q.id]; if (!c) return;
    c.run = { ...c.run, repairing: true, repairSuggestion: null }; cards = cards;
    try {
      const lastError = (c.detail.tests.find((t) => t.status === 'error' && t.error_message) || {}).error_message || c.run.error || null;
      const res = await repairSavedQuery(qScope(q), qOwner(q), q.slug, { error: lastError, save: false });
      c.run = { ...c.run, repairing: false, repairSuggestion: res.sparql }; cards = cards;
    } catch (e) { c.run = { ...c.run, repairing: false }; cards = cards; toastError(e.message); }
  }
  async function saveRepair(q) {
    const c = cards[q.id]; if (!c) return;
    try {
      await updateSavedQuery(qScope(q), qOwner(q), q.slug, { sparql: formatSparql(c.run.repairSuggestion), version_name: $i18nT('pages.apiServices.llmRepairVersionName'), note: $i18nT('pages.apiServices.llmRepairNote') });
      toastSuccess($i18nT('pages.apiServices.repairSaved'));
      c.run.repairSuggestion = null; cards = cards;
      await load(); await loadDetail(q);
    } catch (e) { toastError(e.message); }
  }

  // ── Revisions ──────────────────────────────────────────────────────────────────
  function toggleRevision(q, r) {
    const c = cards[q.id]; if (!c) return;
    c.revRev = c.revRev === r.revision ? null : r.revision; cards = cards;
  }
  async function restoreRevision(q, r) {
    if (!confirm($i18nT('pages.apiServices.restoreConfirm', { values: { revision: r.revision } }))) return;
    try {
      await updateSavedQuery(qScope(q), qOwner(q), q.slug, {
        sparql: formatSparql(r.sparql),
        version_name: $i18nT('pages.apiServices.restoreVersionName', { values: { revision: r.revision } }),
        note: r.name ? $i18nT('pages.apiServices.restoreNoteNamed', { values: { revision: r.revision, name: r.name } }) : $i18nT('pages.apiServices.restoreNote', { values: { revision: r.revision } }),
      });
      toastSuccess($i18nT('pages.apiServices.restoreSuccess', { values: { revision: r.revision } }));
      await load(); await loadDetail(q);
    } catch (e) { toastError(e.message); }
  }

  async function acknowledge(q, test) {
    try { await acknowledgeSavedQueryTest(qScope(q), qOwner(q), q.slug, test.id); test.acknowledged = true; cards = cards; toastSuccess($i18nT('pages.apiServices.acknowledged')); }
    catch (e) { toastError(e.message); }
  }

  async function remove(q) {
    if (!confirm($i18nT('pages.apiServices.deleteConfirm', { values: { name: q.name } }))) return;
    try {
      await deleteSavedQuery(qScope(q), qOwner(q), q.slug);
      toastSuccess($i18nT('pages.apiServices.deleted'));
      openIds.delete(q.id); openIds = new Set(openIds); syncOpenUrl();
      await load();
    } catch (e) { toastError(e.message); }
  }

  // ── Create / edit form ─────────────────────────────────────────────────────────
  function openCreate() { editingSlug = null; editingScope = scope; editingOwner = ownerId; form = blankForm(); resetFormExtras(); showForm = true; }
  function openEdit(q) {
    editingSlug = q.slug; editingScope = qScope(q); editingOwner = qOwner(q);
    form = {
      name: q.name, description: q.description ?? '', sparql: q.sparql ?? '',
      parameters: (q.parameters ?? []).map((p) => ({ ...p })),
      test_parameters_text: q.test_parameters ? JSON.stringify(q.test_parameters) : '',
      // Version name + note describe THIS edit (a fresh commit), so start blank.
      version_name: '', note: '',
    };
    resetFormExtras(); showForm = true;
    loadLastCommit(editingScope, editingOwner, q.slug, q.current_revision);
    if (!q.sparql) getSavedQuery(editingScope, editingOwner, q.slug).then((full) => { form.sparql = full?.sparql ?? form.sparql; form = form; }).catch(() => {});
  }
  function addParam() { form.parameters = [...form.parameters, { name: '', type: 'string', required: true, default: '', description: '' }]; }
  function removeParam(i) { form.parameters = form.parameters.filter((_, idx) => idx !== i); }

  // Auto-detect placeholders in the query and reconcile with declared params.
  $: formDetected = detectPlaceholders(form.sparql);
  $: formDeclaredNames = new Set(form.parameters.map((p) => p.name).filter(Boolean));
  $: formMissingParams = formDetected.filter((n) => !formDeclaredNames.has(n));
  $: formUnusedParams = form.parameters.map((p) => p.name).filter((n) => n && !formDetected.includes(n));
  // The revision number a save would create (current head + 1; 1 for a new service).
  $: editingQuery = editingSlug ? queries.find((q) => q.slug === editingSlug) : null;
  $: nextRevision = editingQuery ? (editingQuery.current_revision ?? 0) + 1 : 1;
  function addDetectedParams() {
    const add = formMissingParams.map((n) => ({ name: n, type: 'string', required: true, default: '', description: '' }));
    form.parameters = [...form.parameters, ...add];
  }

  async function generateFromNl() {
    const q = nlQuestion.trim();
    if (!q || nlLoading) return;
    nlLoading = true; nlError = '';
    try {
      const schemaHint = Object.entries(extractDeclaredPrefixes(form.sparql)).map(([p, ns]) => `PREFIX ${p}: <${ns}>`).join('\n');
      const { sparql } = await nlToSparql(q, schemaHint);
      if (sparql && sparql.trim()) { form.sparql = sparql.trim(); lastGenerated = { question: q, sparql: form.sparql }; testResult = null; testError = ''; }
      else nlError = $i18nT('pages.apiServices.nlEmptyQuery');
    } catch (e) { nlError = e?.message || $i18nT('pages.apiServices.nlGenerationFailed'); }
    finally { nlLoading = false; }
  }
  function buildSparqlSignal(decision, edited) {
    return {
      track: 'sparql', event: 'sparql_gen',
      input: { nl_question: lastGenerated?.question ?? null },
      output: { corrected_turtle: lastGenerated?.sparql ?? null },
      label: { decision, edited_turtle: edited ?? null, source: 'human', rating: null, comment: null },
      prov: { app: 'opentriplestore', surface: 'api-services' },
    };
  }

  function renderParamValue(type, raw) {
    const v = String(raw ?? '').trim();
    if (!v) return null;
    switch (type) {
      case 'iri': return `<${v}>`;
      case 'integer': case 'decimal': case 'boolean': return v;
      case 'date': return `"${v}"^^<${XSD}date>`;
      case 'dateTime': return `"${v}"^^<${XSD}dateTime>`;
      default: return JSON.stringify(v);
    }
  }
  function substituteParams(sparql, params, values) {
    let out = sparql;
    for (const p of params) {
      if (!p.name) continue;
      const rendered = renderParamValue(p.type, values[p.name] ?? p.default ?? '');
      if (rendered == null) continue;
      out = out.split(`{{${p.name}}}`).join(rendered);
    }
    return out;
  }
  async function testRun() {
    testRunning = true; testResult = null; testError = '';
    try {
      let values = {};
      const txt = form.test_parameters_text.trim();
      if (txt) { try { values = JSON.parse(txt); } catch { throw new Error($i18nT('pages.apiServices.testParamsInvalid', { values: { example: '{"city":"urn:nl:utrecht"}' } })); } }
      const q = substituteParams(form.sparql, form.parameters, values);
      if (/\{\{\s*[\w-]+\s*\}\}/.test(q)) { testError = $i18nT('pages.apiServices.fillTestParams'); return; }
      const res = await sparqlQuery(q);
      testResult = res?._graphResult ? { raw: res.ntriples } : res;
      if (lastGenerated) {
        const edited = form.sparql.trim() !== lastGenerated.sparql.trim();
        sendLlmFeedback(buildSparqlSignal(edited ? 'edit' : 'approve', edited ? form.sparql.trim() : null));
        if (edited) lastGenerated = { ...lastGenerated, sparql: form.sparql };
      }
    } catch (e) { testError = e.message; } finally { testRunning = false; }
  }

  function buildPayload() {
    let test_parameters = null;
    const txt = form.test_parameters_text.trim();
    if (txt) { try { test_parameters = JSON.parse(txt); } catch { throw new Error($i18nT('pages.apiServices.testParamsInvalid', { values: { example: '{"city":"urn:nl:utrecht"}' } })); } }
    const parameters = form.parameters.filter((p) => p.name.trim()).map((p) => ({
      name: p.name.trim(), type: p.type, required: !!p.required,
      default: p.default === '' ? null : p.default, description: p.description || null,
    }));
    return {
      // Always persist the pretty-printed query so every stored revision is
      // consistently formatted.
      name: form.name.trim(), description: form.description || null, sparql: formatSparql(form.sparql), parameters, test_parameters,
      // Commit-style version metadata for the revision this save creates.
      version_name: form.version_name?.trim() || null,
      note: form.note?.trim() || null,
    };
  }
  async function submitForm() {
    let payload; try { payload = buildPayload(); } catch (e) { toastError(e.message); return; }
    if (!payload.name) { toastError($i18nT('pages.apiServices.nameRequired')); return; }
    try {
      if (editingSlug) { await updateSavedQuery(editingScope, editingOwner, editingSlug, payload); toastSuccess($i18nT('pages.apiServices.serviceUpdated')); }
      else { await createSavedQuery(scope, ownerId, payload); toastSuccess($i18nT('pages.apiServices.serviceCreated')); }
      if (lastGenerated) {
        const edited = form.sparql.trim() !== lastGenerated.sparql.trim();
        sendLlmFeedback(buildSparqlSignal(edited ? 'edit' : 'approve', edited ? form.sparql.trim() : null));
      }
      showForm = false; resetFormExtras(); await load();
    } catch (e) { toastError(e.message); }
  }
  function cancelForm() {
    showForm = false;
    if (lastGenerated) sendLlmFeedback(buildSparqlSignal('reject', null));
    resetFormExtras();
  }

  // ── Grouping per dataset (org/group scope) + reads-from labels ────────────────
  $: grouped = buildGroups(queries, scope, ownerName);
  function buildGroups(qs, scp, ownName) {
    if (scp === 'datasets') return [{ key: 'all', label: null, kind: 'dataset', items: qs }];
    const map = new Map();
    const WIDE = '__wide__';
    for (const q of qs) {
      if (q.scope === 'dataset') {
        const rf = q.reads_from || {};
        const id = rf.dataset_id ?? q.owner_id;
        const label = rf.dataset_name ?? id;
        if (!map.has(id)) map.set(id, { key: id, label, kind: 'dataset', datasetId: id, items: [] });
        map.get(id).items.push(q);
      } else {
        if (!map.has(WIDE)) map.set(WIDE, { key: WIDE, label: scp === 'organisations' ? $i18nT('pages.apiServices.acrossAllDatasetsIn', { values: { name: ownName } }) : $i18nT('pages.apiServices.acrossAllDatasetsGroup'), kind: 'wide', items: [] });
        map.get(WIDE).items.push(q);
      }
    }
    const groups = [...map.values()];
    groups.sort((a, b) => (a.kind === 'wide' ? 1 : b.kind === 'wide' ? -1 : (a.label || '').localeCompare(b.label || '')));
    return groups;
  }
  function readsBadge(q) {
    const rf = q.reads_from; if (!rf || scope === 'datasets') return null;
    if (rf.kind === 'dataset') {
      const name = rf.dataset_name ?? rf.dataset_id;
      return { kind: 'dataset', text: name, title: $i18nT('pages.apiServices.readsFromDataset', { values: { name } }) };
    }
    const names = (rf.datasets ?? []).map((d) => d.name);
    return {
      kind: 'wide',
      text: names.length ? $i18nT('pages.apiServices.datasetCount', { values: { count: names.length } }) : $i18nT('pages.apiServices.wholeScope'),
      title: names.length ? $i18nT('pages.apiServices.readsAcross', { values: { names: names.join(', ') } }) : $i18nT('pages.apiServices.readsAcrossWholeScope'),
    };
  }

  function rowTest(q) {
    const c = cards[q.id];
    return openIds.has(q.id) && c?.detail.tests?.length ? c.detail.tests[0] : null;
  }
  function bindingsTable(data) {
    if (!data) return null;
    if (data.boolean !== undefined) return { boolean: data.boolean };
    if (data.raw !== undefined) return { raw: data.raw };
    return { vars: data.head?.vars ?? [], rows: data.results?.bindings ?? [] };
  }

  // ── Render run results by the *actual* returned content-type ─────────────────
  // RFC4180-ish parser for SPARQL CSV results (quoted fields, doubled quotes).
  function parseCsv(text, delim = ',') {
    const s = (text || '').replace(/\r\n/g, '\n').replace(/\r/g, '\n');
    const rows = []; let row = [], field = '', inQ = false;
    for (let i = 0; i < s.length; i++) {
      const ch = s[i];
      if (inQ) {
        if (ch === '"') { if (s[i + 1] === '"') { field += '"'; i++; } else inQ = false; }
        else field += ch;
      } else if (ch === '"') inQ = true;
      else if (ch === delim) { row.push(field); field = ''; }
      else if (ch === '\n') { row.push(field); rows.push(row); row = []; field = ''; }
      else field += ch;
    }
    if (field.length || row.length) { row.push(field); rows.push(row); }
    if (rows.length && rows[rows.length - 1].length === 1 && rows[rows.length - 1][0] === '') rows.pop();
    return { vars: rows.shift() ?? [], rows };
  }
  // SPARQL TSV uses Turtle-encoded terms (no CSV quoting): split on tab/newline.
  function parseTsv(text) {
    const lines = (text || '').replace(/\r\n?/g, '\n').split('\n');
    while (lines.length && lines[lines.length - 1] === '') lines.pop();
    const rows = lines.map((l) => l.split('\t'));
    return { vars: rows.shift() ?? [], rows };
  }
  function ctLabel(ct) {
    ct = (ct || '').toLowerCase();
    if (ct.includes('sparql-results+json')) return 'SPARQL JSON';
    if (ct.includes('sparql-results+xml')) return 'SPARQL XML';
    if (ct.includes('ld+json')) return 'JSON-LD';
    if (ct.includes('csv')) return 'CSV';
    if (ct.includes('tab-separated')) return 'TSV';
    if (ct.includes('turtle')) return 'Turtle';
    if (ct.includes('n-quads')) return 'N-Quads';
    if (ct.includes('n-triples')) return 'N-Triples';
    if (ct.includes('trig')) return 'TriG';
    if (ct.includes('rdf+xml')) return 'RDF/XML';
    if (ct.includes('json')) return 'JSON';
    if (ct.includes('xml')) return 'XML';
    return ct.split(';')[0] || 'raw';
  }
  // ── Result views: Raw · Formatted · Table (decoupled from the wire format) ───
  // A finished run is normalised into three independently-selectable views:
  //   • raw       — the exact bytes the API returned (whatever format was asked)
  //   • formatted — pretty JSON / column-aligned CSV-TSV / RDF as served
  //   • table     — a real, selectable HTML table, when the body is tabular
  // The chosen mode is sticky per card so re-running keeps the user's choice.
  $: MODE_LABEL = { raw: $i18nT('pages.apiServices.modeRaw'), formatted: $i18nT('pages.apiServices.modeFormatted'), table: $i18nT('pages.apiServices.modeTable') };

  // Normalise a term (SPARQL-JSON binding, SPARQL-XML binding, or N-Triples token)
  // into the shape RdfTerm expects: { type:'uri'|'literal'|'bnode', value, datatype?, language? }.
  function termOf(b) {
    if (b == null) return null;
    const type = b.type === 'typed-literal' ? 'literal' : b.type;
    const t = { type, value: b.value };
    if (b.datatype) t.datatype = b.datatype;
    if (b['xml:lang']) t.language = b['xml:lang'];
    return t;
  }
  // A plain-text rendering of a cell (typed term or raw string), for column-aligned
  // text views and cell tooltips.
  function cellText(cell) {
    if (cell == null) return '';
    if (typeof cell !== 'object') return String(cell);
    return cell.value != null ? String(cell.value) : '';
  }
  // SPARQL-results JSON bindings → { vars, rows: term[][] } (cells are RdfTerm terms).
  function gridFromBindings(data) {
    const vars = data?.head?.vars ?? [];
    const rows = (data?.results?.bindings ?? []).map((b) => vars.map((v) => termOf(b[v])));
    return { vars, rows, typed: true };
  }
  // SPARQL-results XML → { vars, rows } or { boolean } (ASK). Uses DOMParser.
  function parseSparqlXml(raw) {
    try {
      const doc = new DOMParser().parseFromString(raw || '', 'application/xml');
      if (doc.querySelector('parsererror')) return null;
      const boolEl = doc.querySelector('boolean');
      if (boolEl) return { boolean: boolEl.textContent.trim() === 'true', vars: [], rows: [] };
      const vars = [...doc.querySelectorAll('head > variable')].map((v) => v.getAttribute('name'));
      const rows = [...doc.querySelectorAll('results > result')].map((res) => {
        const map = {};
        for (const b of res.querySelectorAll('binding')) {
          map[b.getAttribute('name')] = termFromXmlEl(b.firstElementChild);
        }
        return vars.map((v) => map[v] ?? null);
      });
      return { vars, rows, typed: true };
    } catch { return null; }
  }
  // A SPARQL-XML binding child (<uri> / <bnode> / <literal …>) → an RdfTerm term.
  function termFromXmlEl(el) {
    if (!el) return null;
    const tag = (el.tagName || '').toLowerCase();
    if (tag === 'uri') return { type: 'uri', value: el.textContent };
    if (tag === 'bnode') return { type: 'bnode', value: el.textContent };
    const t = { type: 'literal', value: el.textContent };
    const dt = el.getAttribute('datatype'); if (dt) t.datatype = dt;
    const lang = el.getAttribute('xml:lang') || el.getAttribute('lang'); if (lang) t.language = lang;
    return t;
  }
  // Tokenise one N-Triples/N-Quads line into its <iri> / "literal" / _:bnode terms.
  function tokenizeNt(line) {
    const out = []; let i = 0; const n = line.length;
    while (i < n) {
      while (i < n && /\s/.test(line[i])) i++;
      if (i >= n) break;
      const ch = line[i];
      if (ch === '<') {
        const end = line.indexOf('>', i);
        if (end < 0) { out.push(line.slice(i)); break; }
        out.push(line.slice(i, end + 1)); i = end + 1;
      } else if (ch === '"') {
        let j = i + 1;
        while (j < n) { if (line[j] === '\\') j += 2; else if (line[j] === '"') { break; } else j++; }
        j++; // past the closing quote
        if (line[j] === '^' && line[j + 1] === '^') { j += 2; if (line[j] === '<') { const e = line.indexOf('>', j); j = e < 0 ? n : e + 1; } }
        else if (line[j] === '@') { while (j < n && !/\s/.test(line[j])) j++; }
        out.push(line.slice(i, j)); i = j;
      } else { let j = i; while (j < n && !/\s/.test(line[j])) j++; out.push(line.slice(i, j)); i = j; }
    }
    return out;
  }
  // One N-Triples/N-Quads token (<iri> / "lit"@lang / "lit"^^<dt> / _:bnode) → an RdfTerm term.
  function termFromNt(tok) {
    if (tok == null) return null;
    const s = String(tok).trim();
    if (!s) return null;
    if (s[0] === '<' && s.endsWith('>')) return { type: 'uri', value: s.slice(1, -1) };
    if (s.startsWith('_:')) return { type: 'bnode', value: s.slice(2) };
    if (s[0] === '"' || s[0] === "'") {
      const q = s[0];
      let j = 1;
      while (j < s.length) { if (s[j] === '\\') j += 2; else if (s[j] === q) break; else j++; }
      const lex = s.slice(1, j)
        .replace(/\\"/g, '"').replace(/\\n/g, '\n').replace(/\\t/g, '\t').replace(/\\r/g, '\r').replace(/\\\\/g, '\\');
      const rest = s.slice(j + 1);
      const t = { type: 'literal', value: lex };
      if (rest.startsWith('^^<')) { const e = rest.indexOf('>'); if (e > 0) t.datatype = rest.slice(3, e); }
      else if (rest[0] === '@') t.language = rest.slice(1);
      return t;
    }
    return { type: 'literal', value: s };
  }
  // Line-based RDF (N-Triples/N-Quads) → an S/P/O(/G) grid of typed terms.
  function parseNt(raw, quads = false) {
    const vars = quads ? ['subject', 'predicate', 'object', 'graph'] : ['subject', 'predicate', 'object'];
    const rows = [];
    for (let line of (raw || '').split('\n')) {
      line = line.trim();
      if (!line || line.startsWith('#')) continue;
      if (line.endsWith('.')) line = line.slice(0, -1).trim();
      const terms = tokenizeNt(line);
      if (terms.length >= 3) rows.push(vars.map((_, i) => termFromNt(terms[i])));
    }
    return { vars, rows, typed: true };
  }
  // Render a grid as monospace, column-aligned text (the "Formatted" view of CSV/TSV).
  function alignGrid(g) {
    const shown = g.rows.slice(0, 200);
    const widths = g.vars.map((v) => String(v).length);
    for (const row of shown) row.forEach((c, i) => { widths[i] = Math.max(widths[i] ?? 0, cellText(c).length); });
    const fmt = (cells) => cells.map((c, i) => cellText(c).padEnd(widths[i] ?? 0)).join('  ').replace(/\s+$/, '');
    const lines = [fmt(g.vars), widths.map((w) => '─'.repeat(w)).join('  '), ...shown.map(fmt)];
    if (g.rows.length > shown.length) lines.push(`… ${(g.rows.length - shown.length).toLocaleString()} more rows`);
    return lines.join('\n');
  }

  // Normalise a finished run into its three views. `table` is non-null only when
  // the body is genuinely tabular; `boolean` is set for ASK.
  function buildResult(c) {
    const ct = (c.run.contentType || '').toLowerCase();
    const raw = c.run.raw || '';
    const data = c.run.result;
    let boolean = null;
    let table = null;

    if (ct.includes('sparql-results+json')) {
      if (data && data.boolean !== undefined) boolean = data.boolean;
      else table = gridFromBindings(data);
    } else if (ct.includes('sparql-results+xml')) {
      const g = parseSparqlXml(raw);
      if (g && g.boolean !== undefined) boolean = g.boolean;
      else table = g;
    } else if (ct.includes('csv')) {
      table = parseCsv(raw, ',');
    } else if (ct.includes('tab-separated')) {
      table = parseTsv(raw);
    } else if (/n-quads/.test(ct)) {
      table = parseNt(raw, true);
    } else if (/n-triples/.test(ct)) {
      table = parseNt(raw, false);
    }
    if (table && !(table.vars?.length) && !(table.rows?.length)) table = null;

    // Formatted rendering: pretty-print JSON/XML, align tabular text, then
    // syntax-highlight by family. `lang` also drives the left-border colour.
    let lang = 'text';
    let formatted = raw;
    if (ct.includes('json')) { lang = 'json'; formatted = prettyJson(raw); }
    else if (ct.includes('xml')) { lang = 'xml'; formatted = prettyXml(raw); }
    else if (/turtle|n-triples|n-quads|trig|rdf/.test(ct)) { lang = 'rdf'; }
    else if (table) { lang = 'text'; formatted = alignGrid(table); }

    // Highlight the (capped) formatted text. The highlighter HTML-escapes all
    // source text, so this is safe to render with {@html} even for result bodies.
    const FMT_CAP = 20000;
    const truncated = formatted.length > FMT_CAP;
    const shown = truncated ? formatted.slice(0, FMT_CAP) : formatted;
    const formattedHtml = highlight(lang, shown) + (truncated ? '\n…' : '');

    return { boolean, table, formatted, formattedHtml, lang, raw };
  }
  function availableModes(r) {
    const modes = ['raw', 'formatted'];
    if (r.table) modes.push('table');
    return modes;
  }
  // The active mode: the user's sticky choice if still applicable, else a smart
  // default (table when tabular, otherwise formatted).
  function effectiveMode(c, r) {
    if (c.run.viewMode && availableModes(r).includes(c.run.viewMode)) return c.run.viewMode;
    return r.table ? 'table' : 'formatted';
  }
  // ── Query form → applicable formats ──────────────────────────────────────────
  // A SELECT/ASK can only be served as tabular bindings; a CONSTRUCT/DESCRIBE only
  // as RDF. We narrow the format picker to what the query can actually produce, so
  // an inapplicable choice (and the confusing "isn't applicable" note) never arises.
  function queryForm(sparql) {
    const s = (sparql || '').replace(/#[^\n]*/g, ' ');
    const m = s.match(/\b(SELECT|ASK|CONSTRUCT|DESCRIBE)\b/i);
    return m ? m[1].toUpperCase() : null;
  }
  const isTabularForm = (form) => form === 'SELECT' || form === 'ASK';
  const isRdfForm = (form) => form === 'CONSTRUCT' || form === 'DESCRIBE';
  const defaultFormatFor = (form) => (isRdfForm(form) ? 'turtle' : 'json');
  // Format options for a service's run panel, narrowed by its query form.
  function formatOptions(q) {
    const form = queryForm(q?.sparql);
    const tab = TABULAR_FORMATS.map((f) => ({ value: f.value, label: f.label, group: $i18nT('pages.apiServices.groupTabular') }));
    const rdf = RDF_FORMATS.map((f) => ({ value: f.value, label: f.label, group: $i18nT('pages.apiServices.groupRdf') }));
    if (isTabularForm(form)) return tab;
    if (isRdfForm(form)) return rdf;
    return [...tab, ...rdf];
  }
  // Re-run with the newly chosen format so the preview reflects the selection.
  // Formats are serialised server-side, so we re-fetch rather than reformat locally.
  function onFormatChange(q) {
    const c = cards[q.id]; if (!c) return;
    if (c.run.result || c.run.error || c.run.noResult) runQuery(q);
  }
  // Safety net: if a query form couldn't be detected and an RDF format was asked of
  // a SELECT/ASK, the backend serves SPARQL results — explain that rather than look broken.
  function formatServedNote(c) {
    const ct = (c.run.contentType || '').toLowerCase();
    const reqRdf = ['turtle', 'jsonld', 'ntriples', 'rdfxml'].includes(c.run.format);
    if (reqRdf && ct.includes('sparql-results'))
      return $i18nT('pages.apiServices.servedNote');
    return '';
  }
</script>

<PageHeader title={$i18nT('pages.apiServices.title')} icon={Bookmark} count={queries.length} breadcrumbs={breadcrumbs} />

<div class="sq-page">
  <p class="lead">{$i18nT('pages.apiServices.leadIntro', { values: { scope: scopeLabel } })}{#if scope !== 'datasets'} {$i18nT('pages.apiServices.leadGrouped', { values: { scope: scopeLabel } })}{/if}</p>
  <div class="sq-toolbar">
    {#if canWrite}
      <button class="btn primary" on:click={openCreate}><Plus size={16} /> {$i18nT('pages.apiServices.newService')}</button>
    {/if}
    <div class="tb-right">
      <Link class="btn ghost" to={`/api-docs?${docsParam}`}><BookOpen size={16} /> {$i18nT('pages.apiServices.apiDocs')}</Link>
      <a class="btn ghost" href={savedQueryOpenApiUrl(scope, ownerId)} target="_blank" rel="noopener"><FileCode size={15} /> openapi.json</a>
      <Link class="btn ghost" to={editorPath}>{$i18nT('pages.apiServices.sparqlEditor')}</Link>
    </div>
  </div>

  {#if !canWrite}
    <p class="signin-note">
      {$i18nT('pages.apiServices.signinBrowse')} <a href="/login">{$i18nT('pages.apiServices.signinLink')}</a> {$i18nT('pages.apiServices.signinRights')}
    </p>
  {/if}

  {#if loading}
    <p class="muted"><Loader2 class="spin" size={16} /> {$i18nT('system.loading')}</p>
  {:else if error}
    <p class="error">{error}</p>
  {:else if queries.length === 0}
    <p class="muted">{$i18nT('pages.apiServices.noServices')}{#if canWrite} {$i18nT('pages.apiServices.noServicesCreate')}{/if}</p>
  {:else}
    {#each grouped as group (group.key)}
      <section class="group">
        {#if group.label}
          <header class="group-head" class:wide={group.kind === 'wide'}>
            {#if group.kind === 'wide'}<Layers size={15} />{:else}<Database size={15} />{/if}
            <span class="group-title">{group.label}</span>
            <span class="group-count">{group.items.length}</span>
          </header>
        {/if}
        <div class="sq-list">
          {#each group.items as q (q.id)}
            {@const badge = readsBadge(q)}
            <div class="sq-card" class:open={openIds.has(q.id)}>
              <div class="sq-row" on:click={() => toggleCard(q)} on:keydown={(e) => e.key === 'Enter' && toggleCard(q)} role="button" tabindex="0">
                <span class="chev">{#if openIds.has(q.id)}<ChevronDown size={16} />{:else}<ChevronRight size={16} />{/if}</span>
                <div class="sq-main">
                  <div class="sq-name">
                    <strong>{q.name}</strong>
                    <code class="slug">{q.slug}</code>
                    {#if badge}
                      <span class="reads-badge" class:wide={badge.kind === 'wide'} title={badge.title}>
                        {#if badge.kind === 'wide'}<Layers size={11} />{:else}<Database size={11} />{/if} {badge.text}
                      </span>
                    {/if}
                    {#if !q.is_active}<span class="badge muted">{$i18nT('pages.apiServices.inactive')}</span>{/if}
                    {#if q.parameters?.length}<span class="badge">{$i18nT('pages.apiServices.paramCount', { values: { count: q.parameters.length } })}</span>{/if}
                    <span class="badge muted">{$i18nT('pages.apiServices.revBadge', { values: { revision: q.current_revision } })}</span>
                    {#if rowTest(q)}
                      {#if rowTest(q).status === 'ok'}<span class="badge ok">{$i18nT('pages.apiServices.statusOk')}</span>
                      {:else if rowTest(q).status === 'changed'}<span class="badge warn">{$i18nT('pages.apiServices.statusChanged')}</span>
                      {:else}<span class="badge err">{$i18nT('pages.apiServices.statusBroken')}</span>{/if}
                    {/if}
                  </div>
                  {#if q.description}<div class="sq-desc">{q.description}</div>{/if}
                </div>
              </div>

              {#if openIds.has(q.id)}
                {@const c = cards[q.id]}
                <div class="sq-detail">
                  {#if !c || c.detail.loading}
                    <p class="muted"><Loader2 class="spin" size={14} /> {$i18nT('system.loading')}</p>
                  {:else}
                    <!-- Run as API -->
                    <div class="panel">
                      <h4><Play size={14} /> {$i18nT('pages.apiServices.runAsApi')}</h4>
                      <div class="endpoint">
                        <span class="method-chip">GET</span>
                        <code class="endpoint-path">{runPath(q)}</code>
                        <button class="icon-btn" title={$i18nT('pages.apiServices.copyEndpointUrl')} on:click={() => copyText(window.location.origin + runPath(q))}><Copy size={13} /></button>
                      </div>
                      {#if paramSpecsFor(q).length}
                        <div class="run-params">
                          {#each paramSpecsFor(q) as p (p.name)}
                            <label class="run-param">{p.name} <span class="ptype">{p.type}{p.required ? ' *' : ''}</span>
                              <Combobox
                                bind:value={c.run.values[p.name]}
                                placeholder={p.default ?? ''}
                                filter={false}
                                suggestions={c.run.suggest[p.name] ?? []}
                                on:focus={() => loadSuggest(q, p, '')}
                                on:input={(e) => requestSuggest(q, p, e.detail)} />
                            </label>
                          {/each}
                        </div>
                      {/if}
                      <div class="run-controls">
                        <label class="run-param inline">{$i18nT('pages.apiServices.versionLabel')}
                          <input bind:value={c.run.version} placeholder={$i18nT('pages.apiServices.versionPlaceholder')} />
                        </label>
                        <label class="run-param inline">{$i18nT('pages.apiServices.formatLabel')}
                          <Select
                            bind:value={c.run.format}
                            size="sm"
                            on:change={() => onFormatChange(q)}
                            options={formatOptions(q)} />
                        </label>
                        <button class="btn primary" on:click={() => runQuery(q)} disabled={c.run.running}>
                          {#if c.run.running}<Loader2 class="spin" size={14} />{:else}<Play size={14} />{/if} {$i18nT('pages.apiServices.runButton')}
                        </button>
                        {#if c.run.versionServed}<span class="served">{$i18nT('pages.apiServices.versionServed')} <code>{c.run.versionServed}</code></span>{/if}
                      </div>

                      {#if c.run.error}
                        <div class="nl-error">{c.run.error}</div>
                      {:else if c.run.result}
                        {@const r = buildResult(c)}
                        {@const note = formatServedNote(c)}
                        {@const modes = availableModes(r)}
                        {@const mode = effectiveMode(c, r)}
                        <div class="results">
                          <div class="results-bar">
                            <span class="ct-chip">{ctLabel(c.run.contentType)}</span>
                            <span class="muted small result-count">
                              {#if r.boolean !== null}{$i18nT('pages.apiServices.booleanResult')}
                              {:else if r.table}{$i18nT('pages.apiServices.rowCount', { values: { count: r.table.rows.length } })}
                              {:else}{$i18nT('pages.apiServices.charCount', { values: { count: (c.run.raw || '').length } })}{/if}
                            </span>
                            <span class="results-actions">
                              <div class="seg" role="group" aria-label={$i18nT('pages.apiServices.resultViewMode')}>
                                {#each modes as m}
                                  <button class="seg-btn" class:active={mode === m} on:click={() => { c.run.viewMode = m; cards = cards; }}>{MODE_LABEL[m]}</button>
                                {/each}
                              </div>
                              {#if c.run.raw}<button class="btn tiny" on:click={() => downloadRun(q)}><Download size={12} /> {$i18nT('pages.apiServices.download')}</button>{/if}
                            </span>
                          </div>
                          {#if note}<p class="served-note">{note}</p>{/if}

                          {#if r.boolean !== null && mode !== 'raw'}
                            <div class="bool">ASK → <strong class:tv={r.boolean} class:fv={!r.boolean}>{r.boolean}</strong></div>
                          {:else if mode === 'table' && r.table}
                            {#if r.table.rows.length === 0}
                              <p class="muted small">{$i18nT('pages.apiServices.noRows')}</p>
                            {:else}
                              <div class="table-scroll">
                                <table class="selectable">
                                  <thead><tr>{#each r.table.vars as v}<th>{v}</th>{/each}</tr></thead>
                                  <tbody>
                                    {#each r.table.rows.slice(0, 200) as row}
                                      <tr>{#each row as cell}<td title={cellText(cell)}>{#if r.table.typed}<RdfTerm term={cell} navigable={false} />{:else}{cell}{/if}</td>{/each}</tr>
                                    {/each}
                                  </tbody>
                                </table>
                              </div>
                              {#if r.table.rows.length > 200}<div class="muted small">{$i18nT('pages.apiServices.showingRows', { values: { shown: 200, total: r.table.rows.length } })}</div>{/if}
                            {/if}
                          {:else if mode === 'formatted'}
                            <!-- eslint-disable-next-line svelte/no-at-html-tags -- escaped syntax-highlighter output -->
                            <pre class="code {r.lang}">{@html r.formattedHtml}</pre>
                          {:else}
                            <pre class="code raw">{r.raw.slice(0, 20000)}{r.raw.length > 20000 ? '\n…' : ''}</pre>
                          {/if}
                        </div>
                      {/if}
                    </div>

                    <!-- Version test history -->
                    <div class="panel">
                      <h4>{$i18nT('pages.apiServices.versionTestHistory')}</h4>
                      {#if c.detail.tests.length === 0}
                        <p class="muted small">{$i18nT('pages.apiServices.noTests')}</p>
                      {:else}
                        <table class="tests">
                          <thead><tr><th>{$i18nT('pages.apiServices.thVersion')}</th><th>{$i18nT('pages.apiServices.thStatus')}</th><th>{$i18nT('pages.apiServices.thRows')}</th><th>{$i18nT('pages.apiServices.thDetail')}</th><th></th></tr></thead>
                          <tbody>
                            {#each c.detail.tests as t}
                              <tr>
                                <td><code>{t.dataset_version}</code></td>
                                <td>
                                  {#if t.status === 'ok'}<span class="badge ok">{$i18nT('pages.apiServices.statusOk')}</span>
                                  {:else if t.status === 'changed'}<span class="badge warn">{$i18nT('pages.apiServices.statusChanged')}</span>
                                  {:else}<span class="badge err">{$i18nT('system.error')}</span>{/if}
                                </td>
                                <td>{t.result_rowcount ?? '—'}</td>
                                <td class="terr">{t.error_message ?? (t.prev_version ? $i18nT('pages.apiServices.versusVersion', { values: { version: t.prev_version } }) : '')}</td>
                                <td>
                                  {#if t.acknowledged}<span class="muted small">{$i18nT('pages.apiServices.acked')}</span>
                                  {:else if qCanWrite(q) && t.status !== 'ok'}
                                    <button class="btn tiny" on:click={() => acknowledge(q, t)}><Check size={12} /> {$i18nT('pages.apiServices.ack')}</button>
                                  {/if}
                                </td>
                              </tr>
                            {/each}
                          </tbody>
                        </table>
                      {/if}
                    </div>

                    <!-- Revisions -->
                    <div class="panel">
                      <h4><History size={14} /> {$i18nT('pages.apiServices.revisions')}</h4>
                      <ul class="revs">
                        {#each c.detail.revisions as r}
                          <li class="rev" class:active={c.revRev === r.revision}>
                            <div class="rev-head" on:click={() => toggleRevision(q, r)} on:keydown={(e) => e.key === 'Enter' && toggleRevision(q, r)} role="button" tabindex="0">
                              <span class="chev">{#if c.revRev === r.revision}<ChevronDown size={13} />{:else}<ChevronRight size={13} />{/if}</span>
                              <strong>{$i18nT('pages.apiServices.revLabel', { values: { revision: r.revision } })}</strong>
                              {#if r.name}<span class="rev-name">{r.name}</span>{/if}
                              <span class="badge muted">{r.origin}</span>
                              {#if r.note}<span class="rev-note">{r.note}</span>{/if}
                              <span class="muted small">{r.created_at.slice(0, 19).replace('T', ' ')}</span>
                            </div>
                            {#if c.revRev === r.revision}
                              <div class="rev-body">
                                <div class="editor-wrap"><SparqlEditorCM query={r.sparql ?? ''} readonly height="160px" /></div>
                                {#if qCanWrite(q) && r.revision !== q.current_revision}
                                  <button class="btn tiny" on:click={() => restoreRevision(q, r)}>{$i18nT('pages.apiServices.restoreAsNew')}</button>
                                {:else if r.revision === q.current_revision}
                                  <span class="muted small">{$i18nT('pages.apiServices.currentRevision')}</span>
                                {/if}
                              </div>
                            {/if}
                          </li>
                        {/each}
                      </ul>
                    </div>

                    <!-- Actions -->
                    <div class="panel actions">
                      <button class="btn" on:click={() => openInEditor(q)} title={$i18nT('pages.apiServices.openInEditorTitle')}>
                        <FileCode size={14} /> {$i18nT('pages.apiServices.openInEditor')}
                      </button>
                      {#if qCanWrite(q)}
                        <button class="btn" on:click={() => openEdit(q)}><Pencil size={14} /> {$i18nT('system.edit')}</button>
                        {#if serviceBroken(q)}
                          <button class="btn warn-btn" on:click={() => fixWithLlm(q)} disabled={c.run.repairing} title={$i18nT('pages.apiServices.fixWithLlmTitle')}>
                            {#if c.run.repairing}<Loader2 class="spin" size={14} />{:else}<Wand2 size={14} />{/if} {$i18nT('pages.apiServices.fixWithLlm')}
                          </button>
                        {/if}
                        <button class="btn danger" on:click={() => remove(q)}><Trash2 size={14} /> {$i18nT('system.delete')}</button>
                      {/if}
                    </div>

                    {#if c.run.repairSuggestion}
                      <div class="panel repair">
                        <h4><Wand2 size={14} /> {$i18nT('pages.apiServices.suggestedRepair')}</h4>
                        <div class="editor-wrap">
                          <SparqlEditorCM bind:query={c.run.repairSuggestion} lint height="180px" sparqlFetcher={sparqlQuery} />
                        </div>
                        <div class="actions">
                          <button class="btn primary" on:click={() => saveRepair(q)}>{$i18nT('pages.apiServices.saveAsNewRevision')}</button>
                          <button class="btn ghost" on:click={() => { c.run.repairSuggestion = null; cards = cards; }}>{$i18nT('pages.apiServices.discard')}</button>
                        </div>
                      </div>
                    {/if}
                  {/if}
                </div>
              {/if}
            </div>
          {/each}
        </div>
      </section>
    {/each}
  {/if}
</div>

<!-- Create / edit modal -->
{#if showForm && (editingSlug ? true : canWrite)}
  <div class="modal-backdrop" on:click|self={cancelForm} role="presentation">
    <div class="modal" role="dialog" aria-modal="true">
      <header class="modal-head">
        <h3>{editingSlug ? $i18nT('pages.apiServices.editService') : $i18nT('pages.apiServices.newService')}</h3>
        <button class="icon-x" on:click={cancelForm} title={$i18nT('system.close')}><X size={18} /></button>
      </header>
      {#if editingSlug && editingScope !== scope}
        <p class="scope-note">{$i18nT('pages.apiServices.scopeNote', { values: { scope: editingScope === 'datasets' ? 'dataset' : editingScope.slice(0, -1) } })} <code>{editingOwner}</code>.</p>
      {/if}
      <div class="modal-body">
        <div class="field-grid">
          <label>{$i18nT('pages.apiServices.nameLabel')}<input bind:value={form.name} placeholder={$i18nT('pages.apiServices.namePlaceholder')} /></label>
          <label>{$i18nT('pages.apiServices.descriptionLabel')}<input bind:value={form.description} placeholder={$i18nT('pages.apiServices.descriptionPlaceholder')} /></label>
        </div>

        <div class="nl-box">
          <Wand2 size={15} class="nl-icon" />
          {#if llmStatus}
            <span class="nl-status" class:offline={!llmStatus.reachable}
              title={llmStatus.reachable ? $i18nT('pages.apiServices.llmOnline', { values: { gateway: llmStatus.gateway ?? '' } }) : $i18nT('pages.apiServices.llmOffline')}>
              {llmStatus.reachable ? '● LLM' : '○ LLM'}
            </span>
          {/if}
          <input class="nl-input" type="text" bind:value={nlQuestion}
            disabled={llmStatus && !llmStatus.reachable}
            placeholder={llmStatus && !llmStatus.reachable ? $i18nT('pages.apiServices.nlPlaceholderOffline') : $i18nT('pages.apiServices.nlPlaceholder')}
            on:keydown={(e) => { if (e.key === 'Enter') { e.preventDefault(); generateFromNl(); } }} />
          <button class="btn" type="button" on:click={generateFromNl} disabled={nlLoading || !nlQuestion.trim()}>
            {#if nlLoading}<Loader2 class="spin" size={14} /> {$i18nT('pages.apiServices.generating')}{:else}<Wand2 size={14} /> {$i18nT('pages.apiServices.generate')}{/if}
          </button>
        </div>
        {#if nlError}<div class="nl-error">{nlError}</div>{/if}

        <span class="editor-label">SPARQL <span class="muted small">— {$i18nT('pages.apiServices.editorUse')} <code>{'{{name}}'}</code> {$i18nT('pages.apiServices.editorForParams')}</span></span>
        <div class="editor-wrap">
          <SparqlEditorCM bind:query={form.sparql} lint height="240px" sparqlFetcher={sparqlQuery} />
        </div>

        <div class="testrun-row">
          <button class="btn" type="button" on:click={testRun} disabled={testRunning} title={$i18nT('pages.apiServices.testRunTitle')}>
            {#if testRunning}<Loader2 class="spin" size={14} />{:else}<Play size={14} />{/if} {$i18nT('pages.apiServices.testRun')}
          </button>
          <span class="muted small">{$i18nT('pages.apiServices.testRunHint')}</span>
        </div>
        {#if testError}<div class="nl-error">{testError}</div>{/if}
        {#if testResult}
          {@const tbl = bindingsTable(testResult)}
          <div class="results testrun-results">
            {#if tbl.boolean !== undefined}
              <div class="bool">ASK → <strong class:tv={tbl.boolean} class:fv={!tbl.boolean}>{tbl.boolean}</strong></div>
            {:else if tbl.raw !== undefined}
              <!-- eslint-disable-next-line svelte/no-at-html-tags -- escaped syntax-highlighter output -->
              <pre class="code rdf">{@html highlightRdf(tbl.raw.slice(0, 8000)) + (tbl.raw.length > 8000 ? '\n…' : '')}</pre>
            {:else}
              <div class="table-scroll">
                <table>
                  <thead><tr>{#each tbl.vars as v}<th>{v}</th>{/each}</tr></thead>
                  <tbody>{#each tbl.rows.slice(0, 50) as row}<tr>{#each tbl.vars as v}<td title={row[v]?.value ?? ''}><RdfTerm term={termOf(row[v])} navigable={false} /></td>{/each}</tr>{/each}</tbody>
                </table>
              </div>
              <div class="muted small">{$i18nT('pages.apiServices.rowCount', { values: { count: tbl.rows.length } })}{tbl.rows.length > 50 ? ' ' + $i18nT('pages.apiServices.showing50') : ''}</div>
            {/if}
          </div>
        {/if}

        <div class="params">
          <div class="params-head">
            <span>{$i18nT('pages.apiServices.parametersHead')}</span>
            <button class="btn tiny" on:click={addParam}><Plus size={13} /> {$i18nT('system.add')}</button>
          </div>
          {#if formMissingParams.length}
            <div class="param-hint">
              {$i18nT('pages.apiServices.paramsDetectedNotDeclared')} {#each formMissingParams as n}<code>{n}</code> {/each}
              <button class="btn tiny" on:click={addDetectedParams}><Plus size={12} /> {$i18nT('pages.apiServices.addN', { values: { count: formMissingParams.length } })}</button>
            </div>
          {/if}
          {#if formUnusedParams.length}
            <div class="param-hint warn">{$i18nT('pages.apiServices.paramsDeclaredNotUsed')} {#each formUnusedParams as n}<code>{n}</code> {/each}</div>
          {/if}
          {#each form.parameters as p, i}
            <div class="param-row">
              <input class="p-name" bind:value={p.name} placeholder={$i18nT('pages.apiServices.phName')} class:undeclared={p.name && !formDetected.includes(p.name)} />
              <Select bind:value={p.type} size="sm" options={PARAM_TYPES} />
              <label class="p-req"><input type="checkbox" bind:checked={p.required} /> {$i18nT('pages.apiServices.required')}</label>
              <input class="p-def" bind:value={p.default} placeholder={$i18nT('pages.apiServices.phDefault')} />
              <input class="p-desc" bind:value={p.description} placeholder={$i18nT('pages.apiServices.phDescription')} />
              <button class="btn tiny danger" on:click={() => removeParam(i)}><Trash2 size={13} /></button>
            </div>
          {/each}
        </div>
        <label>{$i18nT('pages.apiServices.testParamsLabel')}
          <input bind:value={form.test_parameters_text} placeholder={'{"city":"urn:nl:utrecht"}'} />
        </label>

        <div class="formats-note">
          <span class="fn-title"><FileCode size={13} /> {$i18nT('pages.apiServices.supportedFormats')}</span>
          <span class="fn-list">
            {#each FORMATS as f}<code class="fmt-chip">{f.label}</code>{/each}
          </span>
          <span class="muted small">{$i18nT('pages.apiServices.formatsHintPre')} <code>format</code> {$i18nT('pages.apiServices.formatsHintMid')} <code>Accept</code> {$i18nT('pages.apiServices.formatsHintPost')}</span>
        </div>

        <div class="version-box">
          <span class="vb-title"><GitCommit size={14} /> {$i18nT('pages.apiServices.versionTitle', { values: { revision: nextRevision } })}</span>
          {#if editingSlug && lastCommit}
            <div class="last-commit">
              <span class="lc-label">{$i18nT('pages.apiServices.lastChange', { values: { revision: lastCommit.revision } })}</span>
              {#if lastCommit.name}<span class="lc-name">{lastCommit.name}</span>{/if}
              {#if lastCommit.note}<span class="lc-note">{lastCommit.note}</span>{/if}
              {#if !lastCommit.name && !lastCommit.note}<span class="muted small">{$i18nT('pages.apiServices.noMessage')}</span>{/if}
              {#if lastCommit.created_at}<span class="lc-when muted small">{lastCommit.created_at.slice(0, 19).replace('T', ' ')}</span>{/if}
            </div>
          {/if}
          <div class="field-grid">
            <label>{$i18nT('pages.apiServices.versionNameLabel')} <span class="muted small">{$i18nT('pages.apiServices.versionNameHint')}</span>
              <input bind:value={form.version_name} placeholder={editingSlug ? $i18nT('pages.apiServices.versionNamePlaceholderEdit') : $i18nT('pages.apiServices.versionNamePlaceholderNew')} />
            </label>
            <label>{$i18nT('pages.apiServices.notesLabel')} <span class="muted small">{$i18nT('pages.apiServices.notesHint')}</span>
              <input bind:value={form.note} placeholder={$i18nT('pages.apiServices.notesPlaceholder')} />
            </label>
          </div>
          <p class="vb-hint">
            {editingSlug
              ? $i18nT('pages.apiServices.vbHintEdit')
              : $i18nT('pages.apiServices.vbHintNew')}
          </p>
        </div>
      </div>
      <footer class="modal-foot">
        <button class="btn primary" on:click={submitForm}>{editingSlug ? $i18nT('pages.apiServices.saveChanges') : $i18nT('system.create')}</button>
        <button class="btn ghost" on:click={cancelForm}>{$i18nT('system.cancel')}</button>
      </footer>
    </div>
  </div>
{/if}

<style>
  /* Map this page's local tokens onto the app theme so it works in light AND
     dark. In light mode these resolve to the same values used before, so the
     light appearance is unchanged; in dark mode surfaces/text follow the theme. */
  .sq-page, .modal-backdrop {
    --surface: var(--bg-strong);
    --muted: var(--ink-500);
    --border: var(--line-strong);
  }
  .sq-page { padding: 1rem 1.25rem; max-width: 1100px; }
  .lead { color: var(--muted, #64748b); font-size: .9rem; margin: 0 0 .8rem; }
  .sq-toolbar { display: flex; gap: .5rem; flex-wrap: wrap; margin-bottom: 1rem; align-items: center; }
  .tb-right { display: flex; gap: .5rem; flex-wrap: wrap; align-items: center; margin-left: auto; }
  .signin-note { margin: -.5rem 0 1rem; padding: .5rem .75rem; background: var(--bg-soft, #f1f5f9); border: 1px solid var(--line-soft, transparent); border-radius: 6px; font-size: .82rem; color: var(--ink-600, #475569); }
  .signin-note a { color: var(--accent, #2563eb); }
  .btn { display: inline-flex; align-items: center; gap: .35rem; padding: .4rem .7rem; border: 1px solid var(--border, #d0d7de); border-radius: 6px; background: var(--surface, #fff); color: inherit; cursor: pointer; font-size: .85rem; text-decoration: none; }
  .btn.primary { background: var(--accent, #2563eb); color: #fff; border-color: transparent; }
  .btn.ghost { background: transparent; }
  .btn.danger { color: #b42318; border-color: #f3c0bb; }
  .btn.warn-btn { color: #92400e; border-color: #fde68a; background: #fffbeb; }
  .btn.tiny { padding: .2rem .45rem; font-size: .75rem; }
  .btn[disabled] { opacity: .6; cursor: default; }

  /* Groups */
  .group { margin-bottom: 1.1rem; }
  .group-head { display: flex; align-items: center; gap: .4rem; padding: .3rem .1rem .45rem; font-size: .82rem; font-weight: 700; color: var(--ink-700, #334155); border-bottom: 2px solid var(--line-strong, #e2e8f0); margin-bottom: .55rem; }
  .group-head.wide { color: #8b5cf6; border-bottom-color: #ddd6fe; }
  .group-title { letter-spacing: .01em; }
  .group-count { margin-left: auto; font-weight: 600; font-size: .72rem; color: var(--ink-500, #64748b); background: var(--bg-soft, #f1f5f9); padding: .05rem .45rem; border-radius: 999px; }

  .sq-list { display: flex; flex-direction: column; gap: .55rem; }
  .sq-card { border: 1px solid var(--border, #e2e8f0); border-radius: 8px; background: var(--surface, #fff); transition: border-color .12s ease, box-shadow .12s ease; }
  .sq-card:hover { border-color: #cbd5e1; box-shadow: 0 1px 3px rgba(15,23,42,.06); }
  .sq-card.open { border-color: #c7d2fe; box-shadow: 0 2px 8px rgba(99,102,241,.10); }
  .sq-row { padding: .65rem .8rem; cursor: pointer; display: flex; gap: .5rem; align-items: flex-start; border-radius: 8px; }
  .sq-row:hover { background: var(--bg-soft, #fafbff); }
  .chev { color: #94a3b8; display: inline-flex; padding-top: .1rem; }
  .sq-main { flex: 1; min-width: 0; }
  .sq-name { display: flex; gap: .45rem; align-items: center; flex-wrap: wrap; }
  .slug { font-size: .75rem; color: var(--muted, #64748b); }
  .sq-desc { font-size: .8rem; color: var(--muted, #64748b); margin-top: .2rem; }

  .reads-badge { display: inline-flex; align-items: center; gap: .25rem; font-size: .68rem; padding: .1rem .45rem; border-radius: 999px; background: #ecfeff; color: #0e7490; border: 1px solid #a5f3fc; }
  .reads-badge.wide { background: #f5f3ff; color: #6d28d9; border-color: #ddd6fe; }

  .badge { font-size: .68rem; padding: .1rem .4rem; border-radius: 999px; background: #eef2ff; color: #3730a3; }
  .badge.ok { background: #dcfce7; color: #166534; }
  .badge.warn { background: #fef3c7; color: #92400e; }
  .badge.err { background: #fee2e2; color: #991b1b; }
  .badge.muted { background: #f1f5f9; color: #64748b; }

  .sq-detail { border-top: 1px solid var(--border, #e2e8f0); padding: .8rem .9rem; display: flex; flex-direction: column; gap: .9rem; }
  .panel h4 { display: flex; align-items: center; gap: .35rem; margin: 0 0 .4rem; font-size: .85rem; }
  /* Endpoint hint */
  .endpoint { display: flex; align-items: center; gap: .4rem; margin: 0 0 .6rem; flex-wrap: wrap; }
  .method-chip { font-size: .65rem; font-weight: 800; letter-spacing: .04em; color: #1e40af; background: #dbeafe; padding: .12rem .4rem; border-radius: 4px; }
  .endpoint-path { font-size: .76rem; color: var(--ink-800, #334155); background: var(--bg-soft, #f8fafc); border: 1px solid var(--line-strong, #e2e8f0); padding: .15rem .45rem; border-radius: 5px; word-break: break-all; }
  .icon-btn { display: inline-flex; align-items: center; justify-content: center; padding: .2rem; border: 1px solid var(--border, #e2e8f0); border-radius: 5px; background: var(--surface, #fff); color: var(--ink-500, #64748b); cursor: pointer; }
  .icon-btn:hover { background: var(--bg-soft, #f1f5f9); color: var(--ink-800, #334155); }

  .run-params { display: grid; grid-template-columns: repeat(auto-fit, minmax(13rem, 1fr)); gap: .5rem .7rem; margin-bottom: .55rem; }
  .run-param { display: flex; flex-direction: column; gap: .2rem; font-size: .8rem; font-weight: 600; margin-bottom: 0; }
  .run-param input { padding: .4rem; border: 1px solid var(--border, #d0d7de); border-radius: 5px; font: inherit; font-weight: 400; }
  .run-controls { display: flex; gap: .6rem; align-items: flex-end; flex-wrap: wrap; padding-top: .15rem; }
  .run-param.inline { margin-bottom: 0; }
  .run-param.inline input { min-width: 11rem; }
  .ptype { font-weight: 400; color: var(--muted, #64748b); font-size: .72rem; }
  .served { font-size: .78rem; color: var(--muted, #64748b); align-self: center; }

  .results { margin-top: .7rem; }
  .results-bar { display: flex; align-items: center; gap: .5rem; margin-bottom: .4rem; }
  .ct-chip { font-size: .7rem; font-weight: 700; color: #0e7490; background: #ecfeff; border: 1px solid #a5f3fc; padding: .1rem .45rem; border-radius: 999px; }
  .result-count { margin-right: auto; }
  .results-actions { display: flex; gap: .4rem; align-items: center; }
  /* Raw · Formatted · Table segmented switch */
  .seg { display: inline-flex; border: 1px solid var(--border, #d0d7de); border-radius: 6px; overflow: hidden; }
  .seg-btn { padding: .2rem .55rem; font-size: .75rem; line-height: 1.4; background: var(--surface, #fff); color: var(--ink-600, #475569); border: none; border-left: 1px solid var(--border, #e2e8f0); cursor: pointer; }
  .seg-btn:first-child { border-left: none; }
  .seg-btn:not(.active):hover { background: var(--bg-soft, #f1f5f9); }
  .seg-btn.active { background: var(--accent, #2563eb); color: #fff; }
  .served-note { font-size: .75rem; color: #854d0e; background: #fffbeb; border: 1px solid #fde68a; border-radius: 5px; padding: .3rem .5rem; margin: 0 0 .45rem; }

  .table-scroll { max-height: 420px; overflow: auto; border: 1px solid var(--border, #e2e8f0); border-radius: 6px; }
  table { border-collapse: collapse; width: 100%; font-size: .8rem; }
  th, td { border: 1px solid var(--border, #e2e8f0); padding: .3rem .5rem; text-align: left; vertical-align: top; }
  .table-scroll thead th { position: sticky; top: 0; background: var(--bg-soft, #f8fafc); z-index: 1; }
  .table-scroll td { max-width: 520px; white-space: pre-wrap; word-break: break-word; }
  /* Selectable results table: zebra striping + freely selectable text so a user
     can highlight and copy cells/rows (the old view truncated cells with ellipsis). */
  table.selectable, table.selectable th, table.selectable td { user-select: text; -webkit-user-select: text; }
  table.selectable tbody tr:nth-child(even) { background: var(--bg-soft, #f8fafc); }
  table.selectable tbody tr:hover { background: color-mix(in srgb, var(--accent, #2563eb) 8%, transparent); }
  .tests td.terr { color: #991b1b; font-size: .75rem; max-width: 420px; word-break: break-word; }

  /* Formatted code views (JSON / XML / RDF / text), coloured by family */
  .code { background: #0f172a; color: #e2e8f0; padding: .65rem .75rem; border-radius: 6px; overflow: auto; font-size: .78rem; line-height: 1.5; margin: 0; max-height: 440px; white-space: pre; font-family: ui-monospace, SFMono-Regular, Menlo, monospace; border-left: 3px solid #334155; }
  .code.json { background: #0b1220; border-left-color: #38bdf8; }
  .code.xml { background: #1a1322; border-left-color: #c084fc; }
  .code.rdf { background: #0f1b14; border-left-color: #34d399; }
  .code.text { background: #111827; border-left-color: #94a3b8; }
  .code.raw { background: #0c1322; border-left-color: #64748b; }
  /* Syntax-highlight tokens (always rendered on the dark .code background). */
  .code :global(.tok-key)     { color: #7dd3fc; }
  .code :global(.tok-str)     { color: #86efac; }
  .code :global(.tok-num)     { color: #fca5a5; }
  .code :global(.tok-kw)      { color: #c4b5fd; font-weight: 600; }
  .code :global(.tok-punct)   { color: #94a3b8; }
  .code :global(.tok-tag)     { color: #93c5fd; }
  .code :global(.tok-attr)    { color: #fcd34d; }
  .code :global(.tok-meta)    { color: #64748b; }
  .code :global(.tok-comment) { color: #64748b; font-style: italic; }
  .code :global(.tok-iri)     { color: #7dd3fc; }
  .code :global(.tok-pname)   { color: #f0abfc; }

  .bool { font-size: .95rem; }
  .bool strong.tv { color: #16a34a; }
  .bool strong.fv { color: #ef4444; }

  .revs { margin: 0; padding: 0; list-style: none; font-size: .8rem; display: flex; flex-direction: column; gap: .25rem; }
  .rev { border: 1px solid var(--line-soft, #eef2f7); border-radius: 6px; }
  .rev.active { border-color: #c7d2fe; }
  .rev-head { display: flex; align-items: center; gap: .4rem; padding: .35rem .5rem; cursor: pointer; }
  .rev-name { font-weight: 600; color: var(--ink-800, #1e293b); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; max-width: 45%; }
  .rev-head .muted.small { margin-left: auto; }
  .rev-note { color: var(--ink-600, #475569); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; max-width: 45%; }
  .rev-body { padding: .1rem .5rem .5rem; display: flex; flex-direction: column; gap: .4rem; }

  .actions { display: flex; gap: .5rem; flex-wrap: wrap; }
  .editor-wrap { border: 1px solid var(--border, #d0d7de); border-radius: 6px; overflow: hidden; }
  .muted { color: var(--muted, #64748b); }
  .small { font-size: .75rem; }
  .error { color: #b42318; }
  .nl-error { padding: .4rem .6rem; background: #fff8f8; border: 1px solid #f3c9c9; border-radius: 4px; color: #b91c1c; font-size: .8rem; }

  /* Modal form */
  .modal-backdrop { position: fixed; inset: 0; background: rgba(15, 23, 42, .45); display: flex; align-items: flex-start; justify-content: center; padding: 3vh 1rem; z-index: 1000; overflow: auto; }
  .modal { background: #fff; border-radius: 10px; width: min(780px, 96vw); max-height: 94vh; display: flex; flex-direction: column; box-shadow: 0 20px 60px rgba(0,0,0,.3); }
  .modal-head { display: flex; align-items: center; justify-content: space-between; padding: .85rem 1.1rem; border-bottom: 1px solid #e5e7eb; }
  .modal-head h3 { margin: 0; font-size: 1rem; }
  .icon-x { background: none; border: none; cursor: pointer; color: #64748b; display: inline-flex; padding: .25rem; border-radius: 5px; }
  .icon-x:hover { background: #f1f5f9; }
  .scope-note { margin: 0; padding: .5rem 1.1rem; background: #fefce8; color: #854d0e; font-size: .8rem; border-bottom: 1px solid #fde68a; }
  .modal-body { padding: 1rem 1.1rem; overflow: auto; display: flex; flex-direction: column; gap: .65rem; }
  .modal-foot { display: flex; gap: .5rem; padding: .8rem 1.1rem; border-top: 1px solid #e5e7eb; }
  .field-grid { display: grid; grid-template-columns: 1fr 1fr; gap: .6rem; }
  .modal-body label { display: flex; flex-direction: column; gap: .25rem; font-size: .8rem; font-weight: 600; }
  .modal-body > label > input, .field-grid input { padding: .45rem; border: 1px solid var(--border, #d0d7de); border-radius: 5px; font: inherit; font-weight: 400; }
  .editor-label { margin-top: .15rem; }
  .version-box { border: 1px solid var(--line-soft, #e5e9f0); background: var(--bg-soft, #f8fafc); border-radius: 8px; padding: .6rem .7rem; display: flex; flex-direction: column; gap: .5rem; margin-top: .15rem; }
  .vb-title { display: inline-flex; align-items: center; gap: .35rem; font-size: .8rem; font-weight: 700; color: var(--ink-700, #334155); }
  .vb-hint { margin: 0; font-size: .72rem; color: var(--muted, #64748b); }
  .last-commit { display: flex; align-items: baseline; gap: .5rem; flex-wrap: wrap; font-size: .76rem; padding: .35rem .5rem; background: var(--surface, #fff); border: 1px solid var(--line-soft, #e5e9f0); border-radius: 6px; }
  .lc-label { font-weight: 700; color: var(--ink-600, #475569); white-space: nowrap; }
  .lc-name { font-weight: 600; color: var(--ink-800, #1e293b); }
  .lc-note { color: var(--ink-600, #475569); }
  .lc-when { margin-left: auto; }
  .formats-note { display: flex; flex-direction: column; gap: .35rem; font-size: .76rem; padding: .5rem .6rem; background: var(--bg-soft, #f8fafc); border: 1px solid var(--line-soft, #e5e9f0); border-radius: 8px; }
  .fn-title { display: inline-flex; align-items: center; gap: .35rem; font-weight: 700; color: var(--ink-700, #334155); }
  .fn-list { display: flex; flex-wrap: wrap; gap: .3rem; }
  .fmt-chip { font-size: .7rem; padding: .1rem .4rem; border-radius: 999px; background: #eef2ff; color: #3730a3; }
  .nl-box { display: flex; align-items: center; gap: .5rem; padding: .4rem .5rem; background: #f5f8ff; border: 1px solid #dbe4ff; border-radius: 6px; }
  .nl-box :global(.nl-icon) { color: #4a6fd9; flex-shrink: 0; }
  .nl-status { font-size: .7rem; font-weight: 600; color: #16a34a; white-space: nowrap; padding: 1px 6px; border-radius: 999px; background: #ecfdf5; border: 1px solid #bbf7d0; }
  .nl-status.offline { color: #b45309; background: #fffbeb; border-color: #fde68a; }
  .nl-input { flex: 1; font-size: .85rem; padding: .35rem .5rem; border: 1px solid #cdd7ee; border-radius: 4px; background: #fff; }
  .nl-input:focus { outline: none; border-color: #4a6fd9; }
  .testrun-row { display: flex; align-items: center; gap: .6rem; flex-wrap: wrap; }
  .testrun-results { border: 1px solid var(--border, #e2e8f0); border-radius: 6px; padding: .5rem; }
  .params-head { display: flex; justify-content: space-between; align-items: center; font-size: .8rem; font-weight: 600; margin-top: .3rem; }
  .param-hint { font-size: .75rem; color: #475569; background: #f8fafc; border: 1px dashed #cbd5e1; border-radius: 5px; padding: .3rem .5rem; margin: .35rem 0; display: flex; align-items: center; gap: .35rem; flex-wrap: wrap; }
  .param-hint.warn { background: #fffbeb; border-color: #fde68a; color: #854d0e; }
  .param-hint code { background: #eef2ff; padding: 0 .3rem; border-radius: 3px; }
  .param-row { display: grid; grid-template-columns: 1.2fr .9fr auto 1fr 1.4fr auto; gap: .4rem; align-items: center; margin: .3rem 0; }
  .param-row input { padding: .35rem; border: 1px solid var(--border, #d0d7de); border-radius: 5px; font: inherit; }
  .param-row .p-req { flex-direction: row; align-items: center; gap: .25rem; font-weight: 400; }
  .p-name.undeclared { border-color: #fca5a5; background: #fff5f5; }
  :global(.spin) { animation: spin 1s linear infinite; }
  @keyframes spin { to { transform: rotate(360deg); } }

  /* ─── Dark theme overrides ───────────────────────────────────────────────── */
  /* Local --surface/--muted/--border tokens already follow the theme; these
     rules re-map the remaining hardcoded accents/banners. Scoped specificity
     out-ranks the global theme, so the :global(:is(...)) prefix lifts these. */
  :global(:is([data-theme="dark"], .dark)) .btn.danger { color: #fca5a5; border-color: rgba(220,38,38,0.45); }
  :global(:is([data-theme="dark"], .dark)) .btn.warn-btn { color: #fcd34d; border-color: rgba(245,158,11,0.45); background: rgba(245,158,11,0.12); }
  :global(:is([data-theme="dark"], .dark)) .group-head.wide { border-bottom-color: rgba(139,92,246,0.45); }
  :global(:is([data-theme="dark"], .dark)) .sq-card:hover { border-color: var(--line-strong); }
  :global(:is([data-theme="dark"], .dark)) .sq-card.open,
  :global(:is([data-theme="dark"], .dark)) .rev.active { border-color: rgba(99,102,241,0.5); }

  :global(:is([data-theme="dark"], .dark)) .reads-badge { background: rgba(6,182,212,0.15); color: #67e8f9; border-color: rgba(6,182,212,0.4); }
  :global(:is([data-theme="dark"], .dark)) .reads-badge.wide { background: rgba(124,58,237,0.18); color: #c4b5fd; border-color: rgba(124,58,237,0.4); }
  :global(:is([data-theme="dark"], .dark)) .ct-chip { background: rgba(6,182,212,0.15); color: #67e8f9; border-color: rgba(6,182,212,0.4); }
  :global(:is([data-theme="dark"], .dark)) .badge { background: rgba(99,102,241,0.18); color: #c7d2fe; }
  :global(:is([data-theme="dark"], .dark)) .badge.ok { background: rgba(16,185,129,0.18); color: #6ee7b7; }
  :global(:is([data-theme="dark"], .dark)) .badge.warn { background: rgba(245,158,11,0.18); color: #fcd34d; }
  :global(:is([data-theme="dark"], .dark)) .badge.err { background: rgba(220,38,38,0.18); color: #fca5a5; }
  :global(:is([data-theme="dark"], .dark)) .badge.muted { background: rgba(255,255,255,0.06); color: var(--ink-500); }
  :global(:is([data-theme="dark"], .dark)) .method-chip { color: #bfdbfe; background: rgba(59,130,246,0.2); }

  :global(:is([data-theme="dark"], .dark)) .tests td.terr,
  :global(:is([data-theme="dark"], .dark)) .error { color: #fca5a5; }
  :global(:is([data-theme="dark"], .dark)) .bool strong.tv { color: #4ade80; }
  :global(:is([data-theme="dark"], .dark)) .bool strong.fv { color: #f87171; }
  :global(:is([data-theme="dark"], .dark)) .served-note { background: rgba(245,158,11,0.12); border-color: rgba(245,158,11,0.4); color: #fcd34d; }
  :global(:is([data-theme="dark"], .dark)) .nl-error { background: rgba(220,38,38,0.12); border-color: rgba(220,38,38,0.35); color: #fca5a5; }

  :global(:is([data-theme="dark"], .dark)) .modal { background: var(--bg-strong); }
  :global(:is([data-theme="dark"], .dark)) .modal-head,
  :global(:is([data-theme="dark"], .dark)) .modal-foot { border-color: var(--line-strong); }
  :global(:is([data-theme="dark"], .dark)) .icon-x:hover { background: rgba(255,255,255,0.06); }
  :global(:is([data-theme="dark"], .dark)) .scope-note { background: rgba(245,158,11,0.12); color: #fcd34d; border-bottom-color: rgba(245,158,11,0.4); }

  :global(:is([data-theme="dark"], .dark)) .nl-box { background: rgba(99,102,241,0.1); border-color: rgba(99,102,241,0.3); }
  :global(:is([data-theme="dark"], .dark)) .nl-box :global(.nl-icon) { color: #a5b4fc; }
  :global(:is([data-theme="dark"], .dark)) .nl-status { color: #6ee7b7; background: rgba(16,185,129,0.15); border-color: rgba(16,185,129,0.4); }
  :global(:is([data-theme="dark"], .dark)) .nl-status.offline { color: #fcd34d; background: rgba(245,158,11,0.12); border-color: rgba(245,158,11,0.4); }
  :global(:is([data-theme="dark"], .dark)) .nl-input { background: var(--bg-strong); border-color: var(--line-strong); color: var(--ink-900); }
  :global(:is([data-theme="dark"], .dark)) .nl-input:focus { border-color: var(--brand-500); }

  :global(:is([data-theme="dark"], .dark)) .param-hint { color: var(--ink-700); background: rgba(255,255,255,0.03); border-color: var(--line-strong); }
  :global(:is([data-theme="dark"], .dark)) .param-hint.warn { background: rgba(245,158,11,0.12); border-color: rgba(245,158,11,0.4); color: #fcd34d; }
  :global(:is([data-theme="dark"], .dark)) .param-hint code { background: rgba(99,102,241,0.2); }
  :global(:is([data-theme="dark"], .dark)) .p-name.undeclared { border-color: rgba(220,38,38,0.5); background: rgba(220,38,38,0.1); }
  :global(:is([data-theme="dark"], .dark)) .last-commit { background: rgba(255,255,255,0.03); border-color: var(--line-strong); }
  :global(:is([data-theme="dark"], .dark)) .formats-note { background: rgba(255,255,255,0.03); border-color: var(--line-strong); }
  :global(:is([data-theme="dark"], .dark)) .fmt-chip { background: rgba(99,102,241,0.18); color: #c7d2fe; }
</style>
