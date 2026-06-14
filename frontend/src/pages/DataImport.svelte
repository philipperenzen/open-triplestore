<script>
  import { onMount } from 'svelte';
  import { t as i18nT } from 'svelte-i18n';
  import { navigate } from '../lib/router/index.js';
  import { Link } from '../lib/router/index.js';
  import { isAuthenticated, user, isAdmin } from '../lib/stores.js';
  import {
    listDatasets, listDatasetGraphs, listOrganisations,
    createDataset, validateDataset, sparqlUpdate, sparqlQuery,
    getDataset, adminListUsers,
    listServices, addServiceGraph, detectShapes, updateDatasetShacl,
    bulkImport, analyzeImport,
  } from '../lib/api.js';
  import { detectRdfFormat, parseNTriplesToBindings, isValidIri, parseSparqlUpdatePreview, detectContentKindFromText, detectGraphRolesFromContent } from '../lib/rdf-utils.js';
  import { collectGraphIris, aggregateShapesProbe } from '../lib/shapesProbe.js';
  import SparqlEditorCM from '../components/SparqlEditorCM.svelte';
  import StepIndicator from '../components/StepIndicator.svelte';
  import Select from '../components/Select.svelte';
  import {
    Upload, FileText, X, Link as LinkIcon, Terminal, Plus, ChevronRight, ChevronLeft,
    User, Building2, Database, Eye, Users, Lock, Check, AlertTriangle,
    Loader2, ExternalLink, BarChart3, RefreshCw, LayoutGrid, Zap, Target, Shield, Info, GitBranch, Tag,
  } from 'lucide-svelte';

  let step = 1;
  let authed = false;
  let currentUser = null;
  let isAdminValue = false;
  isAuthenticated.subscribe(v => authed = v);
  user.subscribe(v => currentUser = v);
  isAdmin.subscribe(v => isAdminValue = v);

  let adminUserMap = {}; // userId (string) → username

  // ── Step 1: files + graph target ──────────────────────────────────────────
  let files = [];
  let useSprarqlUpdate = false;
  let sparqlUpdateText = 'INSERT DATA {\n  \n}';
  $: sparqlPreview = useSprarqlUpdate ? parseSparqlUpdatePreview(sparqlUpdateText) : null;

  const SPARQL_TEMPLATES = [
    {
      id: 'insert-data',
      labelKey: 'pages.import.tpl.insertData',
      query: 'INSERT DATA {\n  \n}',
    },
    {
      id: 'dbpedia-persons',
      labelKey: 'pages.import.tpl.dbpediaPersons',
      query: `PREFIX rdf:  <http://www.w3.org/1999/02/22-rdf-syntax-ns#>
PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
PREFIX dbo:  <http://dbpedia.org/ontology/>

INSERT {
  GRAPH <http://example.org/import/dbpedia-persons> {
    ?person a dbo:Person ;
            rdfs:label ?name ;
            dbo:birthDate ?birth .
  }
}
WHERE {
  SERVICE <https://dbpedia.org/sparql> {
    SELECT ?person ?name ?birth WHERE {
      ?person a dbo:Person ;
              rdfs:label ?name ;
              dbo:birthDate ?birth .
      FILTER(langMatches(lang(?name), "en"))
    }
    LIMIT 100
  }
}`,
    },
    {
      id: 'dbpedia-resource',
      labelKey: 'pages.import.tpl.dbpediaResource',
      query: `# Replace the resource IRI below with the DBpedia resource you want to fetch.
# Example: <http://dbpedia.org/resource/Amsterdam>

INSERT {
  GRAPH <http://example.org/import/dbpedia-resource> {
    <http://dbpedia.org/resource/Amsterdam> ?predicate ?value .
  }
}
WHERE {
  SERVICE <https://dbpedia.org/sparql> {
    <http://dbpedia.org/resource/Amsterdam> ?predicate ?value .
  }
}`,
    },
    {
      id: 'wikidata-items',
      labelKey: 'pages.import.tpl.wikidataItems',
      query: `PREFIX wd:     <http://www.wikidata.org/entity/>
PREFIX wdt:    <http://www.wikidata.org/prop/direct/>
PREFIX rdfs:   <http://www.w3.org/2000/01/rdf-schema#>
PREFIX schema: <http://schema.org/>

INSERT {
  GRAPH <http://example.org/import/wikidata> {
    ?item rdfs:label ?label ;
          schema:description ?desc .
  }
}
WHERE {
  SERVICE <https://query.wikidata.org/sparql> {
    SELECT ?item ?label ?desc WHERE {
      ?item wdt:P31 wd:Q5 ;
            rdfs:label ?label ;
            schema:description ?desc .
      FILTER(lang(?label) = "en")
      FILTER(lang(?desc) = "en")
    }
    LIMIT 50
  }
}`,
    },
    {
      id: 'delete-insert',
      labelKey: 'pages.import.tpl.deleteInsert',
      query: `# DELETE old triples and INSERT updated ones in a single operation.
# Adjust the GRAPH IRI, patterns, and BIND expression to match your data.

DELETE {
  GRAPH <http://example.org/my-graph> {
    ?s ?p ?o .
  }
}
INSERT {
  GRAPH <http://example.org/my-graph> {
    ?s ?p ?newValue .
  }
}
WHERE {
  GRAPH <http://example.org/my-graph> {
    ?s ?p ?o .
    BIND(?o AS ?newValue)
  }
}`,
    },
    {
      id: 'load-url',
      labelKey: 'pages.import.tpl.loadUrl',
      query: `# LOAD fetches an RDF document from a URL and inserts its triples into a named graph.
# Replace the source URL and target graph IRI below.

LOAD <https://example.org/data.ttl>
INTO GRAPH <http://example.org/import/loaded>`,
    },
  ];

  let selectedTemplateId = '';

  function loadTemplate(id) {
    const tpl = SPARQL_TEMPLATES.find(t => t.id === id);
    if (tpl) { sparqlUpdateText = tpl.query; selectedTemplateId = id; }
  }

  // ── Preview pagination ──────────────────────────────────────────────────────
  const PREVIEW_PAGE_SIZE = 20;
  let sparqlPreviewPage = 0;

  // WHERE-clause preview: runs the update's WHERE block as a read-only SELECT so
  // the user can see which rows a pattern-based INSERT/DELETE would match.
  let wherePreviewResult = null; // null=not run; { rows, vars } on success; { error } on failure
  let wherePreviewPage = 0;
  let wherePreviewLoading = false;

  // A pattern-based update's matches depend on the query text, so a previously
  // fetched preview goes stale the moment the text changes — clear it and reset
  // both page counters.
  $: { sparqlUpdateText; sparqlPreviewPage = 0; wherePreviewPage = 0; wherePreviewResult = null; }

  // URL fetch preview (before adding to files)
  let urlPreviewData = null; // { name, content, format, fromUrl, rows, rawLines, graphIri }
  let urlPreviewPage = 0;

  let useUrlImport = false;
  let importUrl = '';
  let fetchingUrl = false;
  let urlError = '';
  let dragOver = false;
  let browseDropdownOpen = false;

  let kindWarningDismissed = false;

  // High-confidence typed files among the current selection
  $: highConfidenceKindFiles = files.filter(
    f => f.detectedKind && f.detectedKind !== 'unknown' && f.detectedKind !== 'mixed' && f.detectedKindConfidence === 'high'
  );
  $: dominantKind = (() => {
    const modelCount = highConfidenceKindFiles.filter(f => f.detectedKind === 'model').length;
    const vocabs = highConfidenceKindFiles.filter(f => f.detectedKind === 'vocabulary').length;
    if (modelCount === 0 && vocabs === 0) return null;
    return modelCount >= vocabs ? 'model' : 'vocabulary';
  })();
  // Reset dismissal when file list changes
  $: { files; kindWarningDismissed = false; }

  // ── Step 2: owner & dataset ────────────────────────────────────────────────
  let organisations = [];
  let datasets = [];
  let ownerType = 'personal';
  let selectedOrgId = '';
  let datasetMode = 'existing';
  let selectedDatasetId = '';
  // Canonical IRI ({base}/dataset/{id}) of the selected/created dataset. Default
  // target graphs are minted under `{selectedDatasetIri}/...` so they pass the
  // server's per-graph write boundary; empty when no dataset is chosen yet.
  let selectedDatasetIri = '';
  let newDatasetName = '';
  let newDatasetDesc = '';
  let newDatasetVis = 'private';
  /** @type {Array<{ val: string, Icon: any, labelKey: string }>} */
  const VIS_OPTIONS = [
    { val: 'public', Icon: Eye, labelKey: 'visPublic' },
    { val: 'members', Icon: Users, labelKey: 'visMembers' },
    { val: 'private', Icon: Lock, labelKey: 'visPrivate' },
  ];

  // ── Step 3: review & import ────────────────────────────────────────────────
  let loadingDatasetGraphs = false;
  // Graph IRIs already registered to the selected dataset (fetched in goToStep3).
  // The bulk-import boundary admits these even when they fall outside the dataset
  // namespace, so the quad pre-flight consults them to avoid false positives.
  let registeredGraphIris = new Set();
  let selectedDatasetShapesIri = null; // null=loading, ''=no shapes, string=has shapes
  let doValidate = false;
  let validationDatasetId = '';
  let preValidationResult = null;
  let validating = false;
  let importing = false;
  let importResult = null;
  let importError = '';
  let importProgress = { current: 0, total: 0, currentFile: '' };
  let fileResults = []; // { name, status: 'ok'|'error', graphIri, error? }
  // Semver bump applied when a replace import changes data (patch|minor|major).
  let versionBump = 'patch';
  // True when at least one file replaces a graph in an existing dataset, so the
  // upload may cut a new version (and the bump selector is relevant).
  $: anyReplaceExisting = !useSprarqlUpdate && datasetMode !== 'new' && files.some(f => f.replace);

  // Quad embedded-graph targets that the server's per-graph write boundary would
  // still reject (effective target not under the dataset namespace and not already
  // registered to it). Best-effort: the server's cross-dataset-ownership check
  // can't be mirrored client-side, so per-file import errors remain the backstop.
  $: foreignQuadTargets = (() => {
    if (!selectedDatasetIri) return [];
    const ns = `${selectedDatasetIri}/`;
    const out = [];
    for (const f of files) {
      if (!isQuadFile(f.file?.name)) continue;
      for (const orig of (f.detectedGraphIris || [])) {
        const cur = f.graphIriRenameMap?.[orig] ?? orig;
        const eff = (cur !== orig && isValidIri(cur)) ? cur : orig;
        if (!registeredGraphIris.has(eff) && !eff.startsWith(ns)) {
          out.push({ file: f.file?.name, orig, target: eff });
        }
      }
    }
    return out;
  })();

  // Distinct embedded graphs that would collapse into the SAME write target
  // (silent multi-graph merge). Surfaced as a separate, non-auto-fixable warning.
  $: mergedQuadTargets = (() => {
    const byTarget = new Map();
    for (const f of files) {
      if (!isQuadFile(f.file?.name)) continue;
      for (const orig of (f.detectedGraphIris || [])) {
        const cur = f.graphIriRenameMap?.[orig] ?? orig;
        const eff = (cur !== orig && isValidIri(cur)) ? cur : orig;
        if (!byTarget.has(eff)) byTarget.set(eff, new Set());
        byTarget.get(eff).add(orig);
      }
    }
    return [...byTarget.entries()]
      .filter(([, origs]) => origs.size > 1)
      .map(([target, origs]) => ({ target, count: origs.size }));
  })();

  // Service graph registration (shown after successful import)
  let availableServices = [];
  let selectedServiceIds = new Set();
  let registeringServiceGraphs = false;
  let serviceGraphResult = null; // { success, registered, failed }

  // SHACL shapes auto-detect (shown after successful import)
  // Aggregated over every imported graph (incl. auto-split '{target}/shapes'
  // subgraphs): { shapesDetected, totalShapeCount, shapeGraphs, suggestedDatasets }.
  let shapesDetectResult = null;
  let shapesLinkTargetId = ''; // dataset id to link detected shapes to
  let shapesLinking = false;
  let shapesLinkDone = false;      // a link actually succeeded
  let shapesLinkDismissed = false; // the prompt was dismissed without linking
  let shapesLinkError = '';

  $: stepLabels = [
    $i18nT('pages.import.step1'),
    $i18nT('pages.import.step2'),
    $i18nT('pages.import.step4'),
  ];

  $: canStep1 = (() => {
    if (useSprarqlUpdate) return sparqlUpdateText.trim().length > 0;
    if (files.length === 0) return false;
    for (const f of files) {
      const lower = f.file.name.toLowerCase();
      const isQuad = lower.endsWith('.nq') || lower.endsWith('.trig');
      if (isQuad) {
        for (const [orig, renamed] of Object.entries(f.graphIriRenameMap)) {
          if (renamed && renamed !== orig && !isValidIri(renamed)) return false;
        }
      } else {
        if (!isValidIri(f.graphIri)) return false;
      }
    }
    return true;
  })();
  $: datasetNameTaken = datasetMode === 'new'
    && newDatasetName.trim() !== ''
    && ownerDatasets.some(d => d.name.toLowerCase() === newDatasetName.trim().toLowerCase());
  $: canStep2 = (() => {
    if (ownerType === 'org' && !selectedOrgId) return false;
    if (datasetMode === 'new') return newDatasetName.trim().length > 0 && !datasetNameTaken;
    return !!selectedDatasetId;
  })();
  $: canStep3 = useSprarqlUpdate || files.length > 0;

  // Datasets belonging to the currently selected owner
  $: ownerDatasets = (() => {
    if (ownerType === 'personal')
      return datasets.filter(d => d.owner_type === 'user' && d.owner_id === currentUser?.id);
    if (!selectedOrgId) return [];
    return datasets.filter(d => d.owner_type === 'organisation' && d.owner_id === selectedOrgId);
  })();

  // Keep datasetMode in sync: switch to 'new' when no datasets exist for this owner,
  // and back to 'existing' when datasets become available (e.g. after org selection).
  // Auto-switch to 'new' only when there are no datasets for this owner.
  // Do NOT auto-switch back — let the user control their choice.
  $: if (ownerDatasets.length === 0) datasetMode = 'new';

  let dataLoaded = false;

  async function loadData() {
    try {
      const [ds, orgs] = await Promise.all([
        listDatasets(),
        listOrganisations().catch(() => []),
      ]);
      datasets = ds || [];
      organisations = orgs || [];
    } catch {}
    if (isAdminValue) {
      try {
        const resp = await adminListUsers({ limit: 100 });
        adminUserMap = Object.fromEntries(
          (resp?.users || []).map(u => [String(u.id), u.username])
        );
      } catch {}
    }
  }

  // Load data as soon as the user is authenticated. Using a reactive block
  // handles both: (a) the user was already authed when component mounted,
  // and (b) the auth store resolves after onMount ran (common on page load).
  $: if (authed && !dataLoaded) {
    dataLoaded = true;
    loadData();
  }

  onMount(() => {
    // If already authed (token in localStorage and store already true), kick
    // off loading immediately so we don't wait for the reactive block.
    if (authed && !dataLoaded) {
      dataLoaded = true;
      loadData();
    }
    const closeDropdown = () => { browseDropdownOpen = false; };
    window.addEventListener('click', closeDropdown);
    return () => window.removeEventListener('click', closeDropdown);
  });

  // ── File helpers ────────────────────────────────────────────────────────────

  function detectGraphIriFromContent(filename, content) {
    const lower = filename.toLowerCase();
    const iris = new Set();
    if (lower.endsWith('.nq') || lower.endsWith('.nquads')) {
      for (const line of content.split('\n').slice(0, 200)) {
        const trimmed = line.trim();
        if (!trimmed || trimmed.startsWith('#')) continue;
        const m = trimmed.match(/<[^>]+>\s+<[^>]+>\s+(?:<[^>]+>|"[^"]*"[^\s]*|\S+)\s+(<[^>]+>)\s*\./);
        if (m) iris.add(m[1].slice(1, -1));
      }
    } else if (lower.endsWith('.trig')) {
      // Parse @prefix and PREFIX declarations to resolve prefixed names
      const prefixes = {};
      for (const m of content.matchAll(/@prefix\s+([a-zA-Z0-9_-]*):\s*<([^>]+)>\s*\./gi))
        prefixes[m[1]] = m[2];
      for (const m of content.matchAll(/PREFIX\s+([a-zA-Z0-9_-]*):\s*<([^>]+)>/gi))
        prefixes[m[1]] = m[2];

      // Full IRI graph names: <...> {
      for (const m of content.matchAll(/(?:GRAPH\s+)?<([^>]+)>\s*\{/gi)) iris.add(m[1]);

      // Prefixed graph names: prefix:local { (skip if prefix looks like a URL scheme)
      for (const m of content.matchAll(/(?:GRAPH\s+)?([a-zA-Z][a-zA-Z0-9_-]*):([\w-]*)\s*\{/gi)) {
        const prefix = m[1];
        const local = m[2];
        if (prefixes[prefix] !== undefined) iris.add(prefixes[prefix] + local);
      }
    }
    return [...iris];
  }

  // Best-effort: does a quad file carry DEFAULT-graph triples (no graph label)?
  // These have no addressable IRI, so on a dataset-scoped import the server routes
  // them into the dataset's own default graph. Used only to show an info note.
  function quadFileHasDefaultGraphTriples(filename, content) {
    const lower = filename.toLowerCase();
    if (lower.endsWith('.nq') || lower.endsWith('.nquads')) {
      for (const line of content.split('\n').slice(0, 200)) {
        const trimmed = line.trim();
        if (!trimmed || trimmed.startsWith('#')) continue;
        // A 4-term quad ends with `<graph> .`; a 3-term default-graph triple does not.
        const endsWithGraph = /\s<[^>]+>\s*\.\s*$/.test(trimmed);
        const isStatement = /\.\s*$/.test(trimmed);
        if (isStatement && !endsWithGraph) return true;
      }
      return false;
    }
    if (lower.endsWith('.trig')) {
      // Strip whole `{ … }` graph blocks, then any remaining top-level triple is a
      // default-graph statement. Misses the rarer unlabelled `{ … }` block form.
      const topLevel = content.replace(/\{[^}]*\}/g, '');
      for (const line of topLevel.split('\n')) {
        const trimmed = line.trim();
        if (!trimmed || trimmed.startsWith('#') || trimmed.startsWith('@')) continue;
        if (/^(?:PREFIX|BASE)\b/i.test(trimmed)) continue;
        if (/\.\s*$/.test(trimmed)) return true;
      }
      return false;
    }
    return false;
  }

  function graphSlug(filename) {
    return filename.replace(/\.[^.]+$/, '').replace(/[^a-zA-Z0-9_-]/g, '-').replace(/-+/g, '-').toLowerCase();
  }

  // Slugify the trailing segment of an IRI (after the last '/' or '#') for use as
  // a dataset-namespaced graph suffix. Falls back to a generic 'graph' when the
  // IRI has no usable tail.
  function slugFromIri(iri) {
    const tail = String(iri || '').split(/[#/]/).filter(Boolean).pop() || '';
    const slug = tail.replace(/[^a-zA-Z0-9_-]/g, '-').replace(/-+/g, '-').replace(/^-+|-+$/g, '').toLowerCase();
    return slug || 'graph';
  }

  function isQuadFile(name) {
    const lower = (name || '').toLowerCase();
    return lower.endsWith('.nq') || lower.endsWith('.trig');
  }

  function generateDefaultGraphIri(filename) {
    return `https://opentriplestore.org/graphs/${graphSlug(filename)}`;
  }

  // Re-home auto-generated default target graphs under the selected dataset's IRI
  // namespace ({datasetIri}/{slug}). The server's bulk-import write boundary only
  // admits graphs registered to the dataset or under this namespace, so an
  // unqualified default like https://opentriplestore.org/graphs/x would be rejected.
  //   - Triple files: re-home the auto-default `graphIri` (untouched ⇔
  //     graphIriAutoDefault). User-edited and content-detected targets are kept.
  //   - Quad files: re-home each embedded graph's rename target to
  //     {datasetIri}/{slugFromIri(embedded)}, but only while it is still
  //     "untouched" (identity or empty) and not already under the namespace.
  //     User-edited and already-namespaced targets are kept. Idempotent.
  function namespaceTargets(fileList, datasetIri) {
    if (!datasetIri) return fileList;
    const ns = `${datasetIri}/`;
    return fileList.map((f) => {
      if (isQuadFile(f.file?.name)) {
        if (!f.detectedGraphIris?.length) return f;
        const map = { ...f.graphIriRenameMap };
        const used = new Set(Object.values(map).filter(Boolean));
        let changed = false;
        for (const orig of f.detectedGraphIris) {
          const cur = map[orig] ?? orig;
          const untouched = cur === orig || cur === '';
          const alreadyNamespaced = typeof cur === 'string' && cur.startsWith(ns);
          if (untouched && !alreadyNamespaced) {
            let target = `${ns}${slugFromIri(orig)}`;
            // Disambiguate within-file slug collisions so two distinct embedded
            // graphs don't silently merge into one target.
            if (used.has(target)) {
              let n = 2;
              while (used.has(`${target}-${n}`)) n++;
              target = `${target}-${n}`;
            }
            map[orig] = target;
            used.add(target);
            changed = true;
          }
        }
        return changed ? { ...f, graphIriRenameMap: map } : f;
      }
      if (!f.graphIriAutoDefault) return f;
      return { ...f, graphIri: `${datasetIri}/${graphSlug(f.file?.name || '')}` };
    });
  }

  // Pre-flight one-click fix: force-re-home every quad embedded graph whose
  // effective write target is still outside the selected dataset (exactly the
  // entries the banner lists), overriding a user-typed foreign IRI. Uses the same
  // collision disambiguation as namespaceTargets.
  function namespaceForeignQuadTargets() {
    const datasetIri = selectedDatasetIri;
    if (!datasetIri) return;
    const ns = `${datasetIri}/`;
    files = files.map((f) => {
      if (!isQuadFile(f.file?.name) || !f.detectedGraphIris?.length) return f;
      const map = { ...f.graphIriRenameMap };
      const used = new Set(Object.values(map).filter(Boolean));
      let changed = false;
      for (const orig of f.detectedGraphIris) {
        const cur = map[orig] ?? orig;
        const eff = (cur !== orig && isValidIri(cur)) ? cur : orig;
        if (!registeredGraphIris.has(eff) && !eff.startsWith(ns)) {
          let target = `${ns}${slugFromIri(orig)}`;
          if (used.has(target)) {
            let n = 2;
            while (used.has(`${target}-${n}`)) n++;
            target = `${target}-${n}`;
          }
          map[orig] = target;
          used.add(target);
          changed = true;
        }
      }
      return changed ? { ...f, graphIriRenameMap: map } : f;
    });
  }

  function addFile(f) {
    if (files.some(x => x.file.name === f.name && x.file.size === f.size)) return;
    const format = detectRdfFormat(f.name);
    const reader = new FileReader();
    reader.onload = ev => {
      const content = /** @type {string} */ (/** @type {FileReader} */ (ev.target).result);
      const lower = f.name.toLowerCase();
      const isQuad = lower.endsWith('.nq') || lower.endsWith('.trig');
      const isNt = lower.endsWith('.nt');
      const detected = detectGraphIriFromContent(f.name, content);
      const graphIriRenameMap = {};
      for (const iri of detected) graphIriRenameMap[iri] = iri;
      const graphIri = isQuad ? '' : (detected.length > 0 ? detected[0] : generateDefaultGraphIri(f.name));
      // True only when graphIri is our generic auto-default (not detected from
      // content, not a quad file) — i.e. safe to re-home under a dataset namespace.
      const graphIriAutoDefault = !isQuad && detected.length === 0;
      let parsedPreview = null;
      if (isNt) {
        const partial = content.split('\n').slice(0, 500).join('\n');
        parsedPreview = parseNTriplesToBindings(partial).results.bindings.slice(0, 20);
      }
      const kindResult = detectContentKindFromText(content);
      // For quad formats, classify each embedded graph on its own so we can show
      // per-graph role badges instead of a single "Mixed" verdict for the file.
      const graphRoles = isQuad ? detectGraphRolesFromContent(f.name, content) : {};
      const hasDefaultGraphTriples = isQuad && quadFileHasDefaultGraphTriples(f.name, content);
      files = [...files, {
        file: f, content, format, detectedGraphIris: detected, hasDefaultGraphTriples,
        graphIri, graphIriAutoDefault, graphIriRenameMap, showPreview: false, parsedPreview,
        previewSearch: '',
        importDest: 'named-graph',
        detectedKind: kindResult.kind,
        detectedKindConfidence: kindResult.confidence,
        graphRoles,
        graphRole: kindToRole(kindResult.kind),
        analyzing: false,
        analyzeResult: null,
        autoSplit: false,
        replace: false,
      }];
      // Mixed triple files: fetch the per-role breakdown immediately so we can
      // show specific role chips ("Model · Vocabulary · …") instead of a bare
      // "Mixed" badge. Quad formats carry their own graphs and are skipped.
      if (kindResult.kind === 'mixed' && !isQuad) {
        analyzeFile(files.length - 1);
      }
    };
    reader.readAsText(f);
  }

  function removeFile(idx) {
    files = files.filter((_, i) => i !== idx);
  }

  async function analyzeFile(idx) {
    const f = files[idx];
    if (!f || f.analyzing) return;
    files = files.map((x, i) => i === idx ? { ...x, analyzing: true, analyzeResult: null } : x);
    try {
      const result = await analyzeImport(f.file);
      files = files.map((x, i) => i === idx ? { ...x, analyzing: false, analyzeResult: result } : x);
    } catch {
      files = files.map((x, i) => i === idx ? { ...x, analyzing: false } : x);
    }
  }

  function toggleAutoSplit(idx, value) {
    files = files.map((x, i) => i === idx ? { ...x, autoSplit: value } : x);
  }

  function setFileReplace(idx, value) {
    files = files.map((x, i) => i === idx ? { ...x, replace: value } : x);
  }

  function setFileGraphRole(idx, value) {
    files = files.map((x, i) => i === idx ? { ...x, graphRole: value } : x);
  }

  // Map a detected content kind to a graph role (1:1 for typed kinds). 'mixed'
  // and 'unknown' have no single role, so the selector starts unset ('').
  const ROLE_OPTIONS = ['instances', 'model', 'vocabulary', 'shapes', 'entailment'];
  function kindToRole(kind) {
    return ROLE_OPTIONS.includes(kind) ? kind : '';
  }

  // Tailwind classes for a role badge/chip. Mirrors the colors used in the
  // split-suggestion panel so chips read consistently across the wizard.
  function roleChipClass(role) {
    switch (role) {
      case 'model': return 'bg-blue-100 text-blue-800';
      case 'shapes': return 'bg-orange-100 text-orange-800';
      case 'vocabulary': return 'bg-purple-100 text-purple-800';
      case 'entailment': return 'bg-amber-100 text-amber-800';
      case 'instances': return 'bg-green-100 text-green-800';
      default: return 'bg-slate-100 text-slate-700';
    }
  }

  function setFileGraphIri(idx, value) {
    // A manual edit pins the target: clear the auto-default flag so dataset
    // selection no longer re-homes it under the dataset namespace.
    files = files.map((f, i) => i === idx ? { ...f, graphIri: value, graphIriAutoDefault: false } : f);
  }

  function setQuadRename(idx, origIri, newValue) {
    files = files.map((f, i) => {
      if (i !== idx) return f;
      return { ...f, graphIriRenameMap: { ...f.graphIriRenameMap, [origIri]: newValue } };
    });
  }

  function togglePreview(idx) {
    files = files.map((f, i) => i === idx ? { ...f, showPreview: !f.showPreview } : f);
  }

  function setFilePreviewSearch(idx, value) {
    files = files.map((f, i) => i === idx ? { ...f, previewSearch: value } : f);
  }

  const RDF_EXTENSIONS = new Set(['.ttl', '.n3', '.nt', '.nq', '.trig', '.rdf', '.owl', '.jsonld', '.json']);

  function isRdfFile(name) {
    const dot = name.lastIndexOf('.');
    if (dot === -1) return false;
    return RDF_EXTENSIONS.has(name.substring(dot).toLowerCase());
  }

  async function readAllEntries(reader) {
    const all = [];
    while (true) {
      const batch = await new Promise((resolve, reject) => reader.readEntries(resolve, reject));
      if (batch.length === 0) break;
      all.push(...batch);
    }
    return all;
  }

  async function traverseEntry(entry) {
    if (entry.isFile) {
      return [await new Promise((resolve, reject) => entry.file(resolve, reject))];
    }
    if (entry.isDirectory) {
      const reader = entry.createReader();
      const entries = await readAllEntries(reader);
      const nested = await Promise.all(entries.map(traverseEntry));
      return nested.flat();
    }
    return [];
  }

  function handleFileInput(e) {
    for (const f of e.target.files) addFile(f);
    e.target.value = '';
  }

  function handleFolderInput(e) {
    for (const f of e.target.files) {
      if (isRdfFile(f.name)) addFile(f);
    }
    e.target.value = '';
  }

  async function handleDrop(e) {
    e.preventDefault();
    dragOver = false;
    const items = e.dataTransfer?.items;
    if (items && items.length > 0) {
      const allFiles = [];
      for (const item of items) {
        const entry = item.webkitGetAsEntry?.();
        if (entry) {
          const traversed = await traverseEntry(entry);
          allFiles.push(...traversed);
        } else {
          const f = item.getAsFile?.();
          if (f) allFiles.push(f);
        }
      }
      for (const f of allFiles) {
        if (isRdfFile(f.name)) addFile(f);
      }
    } else {
      for (const f of e.dataTransfer.files) {
        if (isRdfFile(f.name)) addFile(f);
      }
    }
  }

  async function fetchFromUrl() {
    if (!importUrl.trim()) return;
    fetchingUrl = true;
    urlError = '';
    urlPreviewData = null;
    urlPreviewPage = 0;
    try {
      const resp = await fetch(importUrl.trim());
      if (!resp.ok) throw new Error(`HTTP ${resp.status}`);
      const text = await resp.text();
      const name = importUrl.split('/').pop().split('?')[0] || 'remote.ttl';
      const format = detectRdfFormat(name);
      const lower = name.toLowerCase();
      let rows = null;
      if (lower.endsWith('.nt') || lower.endsWith('.nq')) {
        rows = parseNTriplesToBindings(text).results.bindings;
      }
      const rawLines = text.split('\n').filter(l => { const t = l.trim(); return t && !t.startsWith('#'); });
      urlPreviewData = {
        name, content: text, format, fromUrl: importUrl.trim(),
        rows, rawLines, graphIri: generateDefaultGraphIri(name),
      };
    } catch (e) { urlError = e.message; }
    finally { fetchingUrl = false; }
  }

  function confirmAddUrl() {
    if (!urlPreviewData) return;
    const { name, content, format, fromUrl, rows, graphIri } = urlPreviewData;
    const lower = name.toLowerCase();
    const isNt = lower.endsWith('.nt');
    const isQuad = lower.endsWith('.nq') || lower.endsWith('.trig');
    const kindResult = detectContentKindFromText(content);
    const detected = isQuad ? detectGraphIriFromContent(name, content) : [];
    const graphIriRenameMap = {};
    for (const iri of detected) graphIriRenameMap[iri] = iri;
    const graphRoles = isQuad ? detectGraphRolesFromContent(name, content) : {};
    const hasDefaultGraphTriples = isQuad ? quadFileHasDefaultGraphTriples(name, content) : false;
    files = [...files, {
      file: { name, size: content.length },
      content, format, fromUrl,
      detectedGraphIris: detected, hasDefaultGraphTriples, graphIri: isQuad ? '' : graphIri,
      graphIriAutoDefault: !isQuad,
      graphIriRenameMap, showPreview: false,
      parsedPreview: (isNt && rows) ? rows.slice(0, 20) : null,
      previewSearch: '',
      importDest: 'named-graph',
      detectedKind: kindResult.kind,
      detectedKindConfidence: kindResult.confidence,
      graphRoles,
      analyzing: false,
      analyzeResult: null,
      autoSplit: false,
      replace: false,
    }];
    urlPreviewData = null;
    importUrl = '';
  }

  // ── Owner helpers ───────────────────────────────────────────────────────────

  /** Select personal account as owner — resets all dataset state. */
  function selectPersonal() {
    ownerType = 'personal';
    selectedOrgId = '';
    selectedDatasetId = '';
    selectedDatasetIri = '';
    datasetMode = 'existing';
  }

  /** Select an organisation as owner — resets dataset state. */
  function selectOrg(orgId) {
    ownerType = 'org';
    selectedOrgId = orgId;
    selectedDatasetId = '';
    selectedDatasetIri = '';
    datasetMode = 'existing';
  }

  // ── Step transitions ────────────────────────────────────────────────────────

  async function goToStep2() {
    step = 2;
    // Refresh the dataset list so datasets created earlier this session (the
    // wizard creates them lazily on import) or by others appear in the existing
    // list — otherwise only datasets present at page load would show.
    try {
      const ds = await listDatasets();
      datasets = ds || [];
    } catch { /* keep the previously loaded list on failure */ }
  }

  async function goToStep3() {
    step = 3;
    selectedDatasetShapesIri = null;
    doValidate = false;
    preValidationResult = null;

    const dsId = datasetMode === 'new' ? null : selectedDatasetId;
    // A brand-new dataset's IRI isn't known until it's created at import time;
    // namespacing for that case happens in runImport once the id exists.
    if (!dsId) { selectedDatasetShapesIri = ''; selectedDatasetIri = ''; registeredGraphIris = new Set(); return; }

    loadingDatasetGraphs = true;
    // Reset up front so a previously-selected dataset's graphs don't leak into the
    // pre-flight when this fetch fails.
    registeredGraphIris = new Set();
    try {
      const graphs = await listDatasetGraphs(dsId);
      registeredGraphIris = new Set((graphs || []).map(g => g.graph_iri).filter(Boolean));
    } catch { /* graphs unavailable, continue */ }
    try {
      const dsDetail = await getDataset(dsId);
      selectedDatasetShapesIri = dsDetail?.shapes_graph_iri || '';
      // Re-home untouched default target graphs under the dataset's namespace so
      // the review (and the import) show the IRIs the server will actually accept.
      selectedDatasetIri = dsDetail?.dataset_iri || '';
      if (selectedDatasetIri) files = namespaceTargets(files, selectedDatasetIri);
    } catch {
      // 403 on private dataset or network error — hide validation section
      selectedDatasetShapesIri = '';
      selectedDatasetIri = '';
    } finally {
      loadingDatasetGraphs = false;
    }
  }

  // ── Import ──────────────────────────────────────────────────────────────────

  // Extract the WHERE block from a pattern-based update and wrap it in a
  // read-only SELECT, preserving PREFIX declarations and capping the result so a
  // broad pattern can't pull the whole store back into the preview.
  function buildWherePreviewQuery(updateText) {
    const prefixes = [...updateText.matchAll(/PREFIX\s+[a-zA-Z0-9_-]*:\s*<[^>]+>/gi)].map(m => m[0]);
    const whereKw = /\bWHERE\b/i.exec(updateText);
    if (!whereKw) return null;
    const open = updateText.indexOf('{', whereKw.index);
    if (open === -1) return null;
    // Walk braces from the WHERE's opening { to its balanced close so nested
    // GRAPH/SERVICE/sub-SELECT blocks are captured whole.
    let depth = 0;
    let close = -1;
    for (let i = open; i < updateText.length; i++) {
      if (updateText[i] === '{') depth++;
      else if (updateText[i] === '}' && --depth === 0) { close = i; break; }
    }
    if (close === -1) return null;
    const body = updateText.slice(open + 1, close);
    const prefixBlock = prefixes.length ? prefixes.join('\n') + '\n' : '';
    return `${prefixBlock}SELECT * WHERE {${body}} LIMIT 100`;
  }

  async function runWherePreview() {
    wherePreviewLoading = true;
    wherePreviewResult = null;
    wherePreviewPage = 0;
    try {
      const query = buildWherePreviewQuery(sparqlUpdateText);
      if (!query) {
        wherePreviewResult = { error: $i18nT('pages.import.noWhereClause') };
        return;
      }
      const res = await sparqlQuery(query);
      wherePreviewResult = {
        vars: res?.head?.vars || [],
        rows: res?.results?.bindings || [],
      };
    } catch (e) {
      wherePreviewResult = { error: e.message };
    } finally {
      wherePreviewLoading = false;
    }
  }

  async function runPreValidation() {
    if (!validationDatasetId) return;
    validating = true;
    preValidationResult = null;
    try { preValidationResult = await validateDataset(validationDatasetId, {}); }
    catch (e) { preValidationResult = { error: e.message }; }
    finally { validating = false; }
  }

  async function runImport() {
    importing = true;
    importError = '';
    importResult = null;
    fileResults = [];
    importProgress = { current: 0, total: files.length, currentFile: '' };
    // Lazily create the dataset only after the first successful upload so a
    // failed import doesn't leave a stranded empty dataset behind.
    const pendingNewDataset = (datasetMode === 'new' && newDatasetName.trim())
      ? {
          name: newDatasetName.trim(),
          description: newDatasetDesc.trim() || null,
          visibility: newDatasetVis,
          owner_type: ownerType === 'org' ? 'organisation' : 'user',
          owner_id: ownerType === 'org' ? selectedOrgId : currentUser?.id,
        }
      : null;
    const ensureDataset = async () => {
      if (selectedDatasetId || !pendingNewDataset) return selectedDatasetId;
      const ds = await createDataset(pendingNewDataset);
      selectedDatasetId = ds.id;
      // The create response carries the canonical IRI so we can namespace targets
      // for this just-created dataset (its slug-derived id wasn't known until now).
      selectedDatasetIri = ds.dataset_iri || '';
      // Keep the local list in sync so the new dataset shows up as "existing"
      // without needing a full page reload.
      if (!datasets.some(d => d.id === ds.id)) datasets = [...datasets, ds];
      return selectedDatasetId;
    };

    try {
      if (useSprarqlUpdate) {
        await sparqlUpdate(sparqlUpdateText);
        importResult = { success: true, sparql: true, preview: sparqlPreview };
      } else {
        // Single multipart POST: parsed in parallel server-side, one bulk-insert.
        importProgress = { current: 0, total: files.length, currentFile: $i18nT('pages.import.filesCount', { values: { count: files.length } }) };
        await ensureDataset();
        // Final safety net: re-home untouched default targets under the dataset
        // namespace. Idempotent for existing datasets (already done in goToStep3)
        // and essential for a just-created one. Without a dataset, leave targets
        // as-is (admin-only unmanaged import).
        if (selectedDatasetIri) files = namespaceTargets(files, selectedDatasetIri);

        const entries = files.map((f) => {
          // Quad files: re-home embedded graphs to their (namespaced) rename
          // targets at write time so the server's per-graph boundary admits them.
          // Only non-identity, valid-IRI renames are sent; the rest keep their
          // embedded names. Replaces the old, boundary-incompatible post-import MOVE.
          let graphRemap;
          if (isQuadFile(f.file.name)) {
            graphRemap = {};
            for (const [orig, tgt] of Object.entries(f.graphIriRenameMap || {})) {
              if (tgt && tgt !== orig && isValidIri(tgt)) graphRemap[orig] = tgt;
            }
          }
          return {
            file: f.file,
            filename: f.file.name,
            targetGraph: f.graphIri || undefined,
            autoSplit: f.autoSplit || false,
            replace: f.replace || false,
            // Explicit role override for the file's target graph. Only applies to
            // non-auto-split files (auto-split derives roles per sub-graph).
            graphRole: (!f.autoSplit && f.graphRole) ? f.graphRole : undefined,
            graphRemap: (graphRemap && Object.keys(graphRemap).length) ? graphRemap : undefined,
          };
        });
        const bulkRes = await bulkImport(entries, {
          datasetId: selectedDatasetId || undefined,
          versionBump: anyReplaceExisting ? versionBump : undefined,
        });

        // Map server's per-file results back to the wizard's shape.
        const byName = new Map(
          (bulkRes.file_results || []).map((r) => [r.filename, r]),
        );
        fileResults = files.map((f) => {
          const r = byName.get(f.file.name);
          if (!r || r.status !== 'ok') {
            return { name: f.file.name, status: 'error', error: r?.error || 'unknown error' };
          }
          // Prefer the authoritative final graph the server wrote and registered;
          // fall back to a valid rename target, then the file's own target.
          const graphIri = (r.graph_iris || [])[0]
            || Object.values(f.graphIriRenameMap || {}).find(v => v && isValidIri(v))
            || f.graphIri
            || '';
          // Keep every graph the file produced — auto-split can route shapes
          // into a '{target}/shapes' subgraph beyond graph_iris[0].
          const graphIris = (r.graph_iris || []).length ? r.graph_iris : (graphIri ? [graphIri] : []);
          return { name: f.file.name, status: 'ok', graphIri, graphIris };
        });

        importProgress = { current: files.length, total: files.length, currentFile: '' };
        const successFiles = fileResults.filter(r => r.status === 'ok');
        const failedFiles = fileResults.filter(r => r.status === 'error');
        const uploadedIris = successFiles.map(r => r.graphIri).filter(Boolean);
        importResult = {
          success: failedFiles.length === 0,
          graphIri: uploadedIris[0] || '',
          count: files.length,
          successCount: successFiles.length,
          failedCount: failedFiles.length,
          datasetId: selectedDatasetId,
          fileResults,
          versionOutcome: bulkRes.version_outcome || null,
        };
        if (failedFiles.length > 0 && successFiles.length === 0) {
          importError = $i18nT('pages.import.allFilesFailed', { values: { count: failedFiles.length } });
        } else if (failedFiles.length > 0) {
          importError = $i18nT('pages.import.someFilesFailed', { values: { failed: failedFiles.length, total: files.length } });
        }

        // Auto-detect SHACL shapes across ALL successfully imported graphs
        // (every file, every graph — auto-split shapes subgraphs included).
        if (successFiles.length > 0 && authed) {
          const probeIris = collectGraphIris(fileResults);
          const probes = [];
          for (const iri of probeIris) {
            try {
              probes.push({ graphIri: iri, result: await detectShapes(iri) });
            } catch (_) {
              // non-critical — skip this graph
            }
          }
          const agg = aggregateShapesProbe(probes);
          shapesDetectResult = agg.shapesDetected ? agg : null;
        }
      }
    } catch (e) {
      importError = e.message;
    } finally {
      importing = false;
    }
  }

  function resetWizard() {
    step = 1;
    files = [];
    importResult = null;
    importError = '';
    fileResults = [];
    importProgress = { current: 0, total: 0, currentFile: '' };
    useSprarqlUpdate = false;
    useUrlImport = false;
    selectedTemplateId = '';
    sparqlPreviewPage = 0;
    wherePreviewResult = null;
    wherePreviewPage = 0;
    urlPreviewData = null;
    urlPreviewPage = 0;
    selectedDatasetId = '';
    selectedDatasetIri = '';
    datasetMode = 'existing';
    selectedDatasetShapesIri = null;
    doValidate = false;
    preValidationResult = null;
    availableServices = [];
    selectedServiceIds = new Set();
    serviceGraphResult = null;
    shapesDetectResult = null;
    shapesLinkTargetId = '';
    shapesLinking = false;
    shapesLinkDone = false;
    shapesLinkDismissed = false;
    shapesLinkError = '';
  }

  $: ownerLabel = ownerType === 'personal'
    ? currentUser?.username
    : organisations.find(o => o.id === selectedOrgId)?.name ?? '';
  $: datasetLabel = datasetMode === 'new'
    ? newDatasetName
    : datasets.find(d => d.id === selectedDatasetId)?.name ?? '';
</script>

<div class="max-w-3xl mx-auto di-page">
  {#if !authed}
    <div class="card text-center py-12">
      <div class="flex justify-center mb-4">
        <div class="w-16 h-16 rounded-2xl bg-[var(--bg-accent-soft)] flex items-center justify-center">
          <Lock size={28} class="text-[var(--brand-600)]" />
        </div>
      </div>
      <h2 class="text-xl font-bold mb-2">{$i18nT('pages.import.signInRequired')}</h2>
      <p class="text-[var(--ink-500)] mb-6 max-w-md mx-auto">{$i18nT('pages.import.signInDesc')}</p>
      <Link to="/login" class="btn inline-flex">
        <Lock size={16} />
        {$i18nT('pages.import.signInBtn')}
      </Link>
    </div>
  {:else}
    <div class="card">
      <div class="mb-6">
        <StepIndicator steps={stepLabels} current={step - 1} />
      </div>

      <!-- ═══════════════════════════════════════════════════════════════════ -->
      <!-- Step 1: Upload + Graph Target                                       -->
      <!-- ═══════════════════════════════════════════════════════════════════ -->
      {#if step === 1}
        <div class="space-y-5">
          <!-- Mode toggle -->
          <div class="flex gap-2 flex-wrap">
            <button
              class="flex-1 min-w-[140px] flex items-center gap-2.5 px-4 py-3 rounded-xl border-2 transition-all cursor-pointer text-left
                {!useSprarqlUpdate && !useUrlImport ? 'border-[var(--brand-500)] bg-[var(--bg-accent-soft)]' : 'border-[var(--line-soft)] bg-white/50 hover:border-[var(--brand-300)]'}"
              on:click={() => { useSprarqlUpdate = false; useUrlImport = false; }}
            >
              <Upload size={18} />
              <span class="font-semibold text-sm">{$i18nT('pages.import.uploadRdf')}</span>
            </button>
            <button
              class="flex-1 min-w-[140px] flex items-center gap-2.5 px-4 py-3 rounded-xl border-2 transition-all cursor-pointer text-left
                {useSprarqlUpdate ? 'border-[var(--brand-500)] bg-[var(--bg-accent-soft)]' : 'border-[var(--line-soft)] bg-white/50 hover:border-[var(--brand-300)]'}"
              on:click={() => { useSprarqlUpdate = true; useUrlImport = false; }}
            >
              <Terminal size={18} />
              <span class="font-semibold text-sm">{$i18nT('pages.import.sparqlUpdate')}</span>
            </button>
            <button
              class="flex-1 min-w-[140px] flex items-center gap-2.5 px-4 py-3 rounded-xl border-2 transition-all cursor-pointer text-left
                {useUrlImport ? 'border-[var(--brand-500)] bg-[var(--bg-accent-soft)]' : 'border-[var(--line-soft)] bg-white/50 hover:border-[var(--brand-300)]'}"
              on:click={() => { useUrlImport = true; useSprarqlUpdate = false; }}
            >
              <LinkIcon size={18} />
              <span class="font-semibold text-sm">{$i18nT('pages.import.importFromUrl')}</span>
            </button>
          </div>

          {#if useSprarqlUpdate}
            <div class="rounded-xl border border-[var(--line-soft)] bg-white/60 p-4 space-y-3">
              <div class="flex items-start justify-between gap-3 flex-wrap">
                <div class="flex-1 min-w-0">
                  <p class="text-sm font-medium text-[var(--ink-700)]">{$i18nT('pages.import.sparqlHint')}</p>
                  <p class="text-xs text-[var(--ink-500)] mt-0.5">
                    {$i18nT('pages.import.supportedOps')} <code class="code-inline">INSERT DATA</code>, <code class="code-inline">DELETE DATA</code>,
                    <code class="code-inline">INSERT … WHERE</code>, <code class="code-inline">DELETE … WHERE</code>,
                    <code class="code-inline">LOAD</code>, <code class="code-inline">CLEAR</code>, <code class="code-inline">DROP</code>,
                    <code class="code-inline">COPY</code>, <code class="code-inline">MOVE</code>.
                  </p>
                </div>
                <div class="flex items-center gap-2 shrink-0">
                  <label class="text-xs font-medium text-[var(--ink-500)]" for="import-tpl-select">{$i18nT('pages.import.tplLabel')}</label>
                  <Select
                    id="import-tpl-select"
                    bind:value={selectedTemplateId}
                    size="sm"
                    class="min-w-[180px]"
                    placeholder={$i18nT('pages.import.tplPlaceholder')}
                    on:change={e => loadTemplate(e.detail)}
                    options={[{ value: '', label: $i18nT('pages.import.tplPlaceholder') }, ...SPARQL_TEMPLATES.map(tpl => ({ value: tpl.id, label: $i18nT(tpl.labelKey) }))]}
                  />
                </div>
              </div>
              <SparqlEditorCM bind:query={sparqlUpdateText} height="240px" />
            </div>

            <!-- SPARQL Update live preview -->
            {#if sparqlPreview}
              {#if sparqlPreview.isPatternBased}
                <div class="rounded-xl border border-amber-200 bg-amber-50 overflow-hidden">
                  <div class="flex items-center justify-between gap-3 px-4 py-3">
                    <div class="flex items-center gap-2 text-amber-800 text-sm">
                      <AlertTriangle size={15} class="shrink-0" />
                      <span>{$i18nT('pages.import.wherePreviewNote')}</span>
                    </div>
                    <button
                      class="btn btn-sm shrink-0 bg-amber-600 hover:bg-amber-700 text-white border-0"
                      on:click={runWherePreview}
                      disabled={wherePreviewLoading}
                    >
                      {#if wherePreviewLoading}<Loader2 size={13} class="animate-spin" />{:else}<Eye size={13} />{/if}
                      {wherePreviewLoading ? $i18nT('pages.import.running') : $i18nT('pages.import.previewMatches')}
                    </button>
                  </div>
                  {#if wherePreviewResult}
                    {#if wherePreviewResult.error}
                      <div class="px-4 pb-3 text-xs text-red-700">{wherePreviewResult.error}</div>
                    {:else}
                      {@const wpTotal = wherePreviewResult.rows.length}
                      {@const wpTotalPages = Math.ceil(wpTotal / PREVIEW_PAGE_SIZE)}
                      {@const wpStart = wherePreviewPage * PREVIEW_PAGE_SIZE}
                      {@const wpEnd = wpStart + PREVIEW_PAGE_SIZE}
                      {@const wpVars = wherePreviewResult.vars}
                      {#if wpTotal === 0}
                        <div class="px-4 pb-3 text-xs text-amber-700 italic">{$i18nT('pages.import.noRowsMatched')}</div>
                      {:else}
                        <div class="overflow-x-auto border-t border-amber-200 max-h-64 overflow-y-auto">
                          <table class="w-full text-[0.68rem] font-mono border-collapse">
                            <thead class="sticky top-0 bg-amber-100/90"><tr class="text-left text-amber-800">
                              {#each wpVars as v}<th class="px-2 py-1 border-b border-amber-200">?{v}</th>{/each}
                            </tr></thead>
                            <tbody>
                              {#each wherePreviewResult.rows.slice(wpStart, wpEnd) as row}
                                <tr class="border-b border-amber-100 hover:bg-amber-100/50">
                                  {#each wpVars as v}
                                    <td class="px-2 py-1 max-w-[200px] truncate" title={row[v]?.value ?? ''}>{row[v]?.value ?? ''}</td>
                                  {/each}
                                </tr>
                              {/each}
                            </tbody>
                          </table>
                        </div>
                        <div class="flex items-center justify-between px-3 py-2 border-t border-amber-200">
                          <span class="text-xs text-amber-700">{$i18nT('pages.import.matchingRows', { values: { count: wpTotal } })}{wpTotal === 100 ? $i18nT('pages.import.limitSuffix') : ''}</span>
                          {#if wpTotalPages > 1}
                            <div class="flex items-center gap-1">
                              <button class="p-1 rounded disabled:opacity-30 hover:bg-amber-200 cursor-pointer disabled:cursor-not-allowed" disabled={wherePreviewPage === 0} on:click={() => wherePreviewPage--}><ChevronLeft size={13} /></button>
                              <span class="text-xs font-medium px-1">{wherePreviewPage + 1} / {wpTotalPages}</span>
                              <button class="p-1 rounded disabled:opacity-30 hover:bg-amber-200 cursor-pointer disabled:cursor-not-allowed" disabled={wherePreviewPage >= wpTotalPages - 1} on:click={() => wherePreviewPage++}><ChevronRight size={13} /></button>
                            </div>
                          {/if}
                        </div>
                      {/if}
                    {/if}
                  {/if}
                </div>
              {:else if sparqlPreview.inserts.length === 0 && sparqlPreview.deletes.length === 0 && sparqlUpdateText.trim().length > 10}
                {@const stripped = sparqlUpdateText.replace(/(^|\n)\s*#[^\n]*/g, '').replace(/PREFIX[^\n]*/gi, '').trim()}
                {@const opMatch = stripped.match(/^\s*(LOAD|CLEAR|DROP|MOVE|COPY|ADD|CREATE)\b/i)}
                <div class="rounded-xl border border-[var(--line-soft)] bg-[var(--bg-accent-soft)]/50 px-3 py-2 text-xs text-[var(--ink-600)] flex items-start gap-2">
                  <AlertTriangle size={14} class="text-[var(--brand-500)] shrink-0 mt-0.5" />
                  <div>
                    {#if opMatch}
                      <strong>{opMatch[1].toUpperCase()}</strong> {$i18nT('pages.import.opNoPreview')}
                    {:else if stripped.length === 0}
                      {$i18nT('pages.import.typeUpdateOrTemplate')}
                    {:else}
                      {$i18nT('pages.import.noTriplesInBlocks')} <code class="code-inline">INSERT DATA &#123; … &#125;</code> {$i18nT('pages.import.or')} <code class="code-inline">DELETE DATA &#123; … &#125;</code> {$i18nT('pages.import.noTriplesNoWhere')} <code class="code-inline">WHERE</code> {$i18nT('pages.import.noTriplesPatternHint')} <code class="code-inline">INSERT &#123; … &#125; WHERE &#123; … &#125;</code> {$i18nT('pages.import.patternForm')}
                    {/if}
                  </div>
                </div>
              {:else}
                {@const totalSparqlRows = sparqlPreview.inserts.length + sparqlPreview.deletes.length}
                {@const sparqlTotalPages = Math.ceil(totalSparqlRows / PREVIEW_PAGE_SIZE)}
                {@const sparqlStart = sparqlPreviewPage * PREVIEW_PAGE_SIZE}
                {@const sparqlEnd = sparqlStart + PREVIEW_PAGE_SIZE}
                {#if sparqlPreview.inserts.length > 0}
                  {@const pageInserts = sparqlPreview.inserts.slice(sparqlStart, sparqlEnd)}
                  {#if pageInserts.length > 0}
                    <div class="rounded-xl border border-emerald-200 bg-emerald-50 overflow-hidden">
                      <div class="flex items-center gap-1.5 px-3 py-2 text-xs font-semibold text-emerald-800 bg-emerald-100 border-b border-emerald-200">
                        <Plus size={13} /> {$i18nT('pages.import.triplesToInsert', { values: { count: sparqlPreview.inserts.length } })}
                      </div>
                      <div class="overflow-x-auto">
                        <table class="w-full text-[0.68rem] font-mono border-collapse">
                          <thead><tr class="text-left text-emerald-700">
                            <th class="px-2 py-1 border-b border-emerald-200">{$i18nT('pages.import.subject')}</th>
                            <th class="px-2 py-1 border-b border-emerald-200">{$i18nT('pages.import.predicate')}</th>
                            <th class="px-2 py-1 border-b border-emerald-200">{$i18nT('pages.import.object')}</th>
                          </tr></thead>
                          <tbody>
                            {#each pageInserts as row}
                              <tr class="border-b border-emerald-100 hover:bg-emerald-100/50">
                                <td class="px-2 py-1 max-w-[220px] truncate" title={row.s?.value}>{row.s?.value ?? ''}</td>
                                <td class="px-2 py-1 max-w-[220px] truncate" title={row.p?.value}>{row.p?.value ?? ''}</td>
                                <td class="px-2 py-1 max-w-[220px] truncate" title={row.o?.value}>{row.o?.value ?? ''}</td>
                              </tr>
                            {/each}
                          </tbody>
                        </table>
                      </div>
                    </div>
                  {/if}
                {/if}
                {#if sparqlPreview.deletes.length > 0}
                  {@const delOffset = sparqlStart - sparqlPreview.inserts.length}
                  {@const pageDeletes = sparqlPreview.deletes.slice(Math.max(0, delOffset), Math.max(0, delOffset) + PREVIEW_PAGE_SIZE)}
                  {#if pageDeletes.length > 0}
                    <div class="rounded-xl border border-red-200 bg-red-50 overflow-hidden">
                      <div class="flex items-center gap-1.5 px-3 py-2 text-xs font-semibold text-red-800 bg-red-100 border-b border-red-200">
                        <X size={13} /> {$i18nT('pages.import.triplesToDelete', { values: { count: sparqlPreview.deletes.length } })}
                      </div>
                      <div class="overflow-x-auto">
                        <table class="w-full text-[0.68rem] font-mono border-collapse">
                          <thead><tr class="text-left text-red-700">
                            <th class="px-2 py-1 border-b border-red-200">{$i18nT('pages.import.subject')}</th>
                            <th class="px-2 py-1 border-b border-red-200">{$i18nT('pages.import.predicate')}</th>
                            <th class="px-2 py-1 border-b border-red-200">{$i18nT('pages.import.object')}</th>
                          </tr></thead>
                          <tbody>
                            {#each pageDeletes as row}
                              <tr class="border-b border-red-100 hover:bg-red-100/50">
                                <td class="px-2 py-1 max-w-[220px] truncate" title={row.s?.value}>{row.s?.value ?? ''}</td>
                                <td class="px-2 py-1 max-w-[220px] truncate" title={row.p?.value}>{row.p?.value ?? ''}</td>
                                <td class="px-2 py-1 max-w-[220px] truncate" title={row.o?.value}>{row.o?.value ?? ''}</td>
                              </tr>
                            {/each}
                          </tbody>
                        </table>
                      </div>
                    </div>
                  {/if}
                {/if}
                <!-- Pagination controls -->
                {#if sparqlTotalPages > 1}
                  <div class="flex items-center justify-between px-1 pt-1">
                    <span class="text-xs text-[var(--ink-400)]">
                      {$i18nT('pages.import.showingTriples', { values: { from: sparqlStart + 1, to: Math.min(sparqlEnd, totalSparqlRows), total: totalSparqlRows } })}
                    </span>
                    <div class="flex items-center gap-1">
                      <button
                        class="p-1 rounded-lg disabled:opacity-30 hover:bg-[var(--bg-accent-soft)] transition-colors cursor-pointer disabled:cursor-not-allowed"
                        disabled={sparqlPreviewPage === 0}
                        on:click={() => sparqlPreviewPage--}
                      ><ChevronLeft size={14} /></button>
                      <span class="text-xs font-medium text-[var(--ink-600)] px-1">{sparqlPreviewPage + 1} / {sparqlTotalPages}</span>
                      <button
                        class="p-1 rounded-lg disabled:opacity-30 hover:bg-[var(--bg-accent-soft)] transition-colors cursor-pointer disabled:cursor-not-allowed"
                        disabled={sparqlPreviewPage >= sparqlTotalPages - 1}
                        on:click={() => sparqlPreviewPage++}
                      ><ChevronRight size={14} /></button>
                    </div>
                  </div>
                {/if}
              {/if}
            {/if}
          {:else}
            {#if useUrlImport}
              <div class="rounded-xl border border-[var(--line-soft)] bg-white/60 p-4 space-y-3">
                <div class="flex gap-2">
                  <input
                    bind:value={importUrl}
                    placeholder={$i18nT('pages.import.urlPlaceholder')}
                    class="flex-1"
                    on:keydown={e => e.key === 'Enter' && fetchFromUrl()}
                  />
                  <button class="btn btn-sm shrink-0" on:click={fetchFromUrl} disabled={fetchingUrl || !importUrl.trim()}>
                    {#if fetchingUrl}<Loader2 size={14} class="animate-spin" />{/if}
                    {fetchingUrl ? $i18nT('pages.import.fetchingUrl') : $i18nT('pages.import.fetchUrl')}
                  </button>
                </div>
                {#if urlError}
                  <p class="error text-sm">{$i18nT('pages.import.urlError')} {urlError}</p>
                {/if}

                <!-- URL fetch preview panel -->
                {#if urlPreviewData}
                  {@const urlRows = urlPreviewData.rows}
                  {@const urlRaw = urlPreviewData.rawLines}
                  {@const urlTotal = urlRows ? urlRows.length : urlRaw.length}
                  {@const urlTotalPages = Math.ceil(urlTotal / PREVIEW_PAGE_SIZE)}
                  {@const urlStart = urlPreviewPage * PREVIEW_PAGE_SIZE}
                  {@const urlEnd = urlStart + PREVIEW_PAGE_SIZE}
                  <div class="rounded-xl border border-[var(--line-soft)] overflow-hidden">
                    <!-- header -->
                    <div class="flex items-center justify-between gap-2 px-3 py-2 bg-[var(--bg-accent-soft)] border-b border-[var(--line-soft)]">
                      <div class="flex items-center gap-2 text-xs font-semibold text-[var(--ink-700)]">
                        <Eye size={13} />
                        {urlPreviewData.name}
                        {#if urlPreviewData.format}
                          <span class="px-1.5 py-0.5 rounded bg-white text-[var(--brand-600)] text-[0.68rem]">{urlPreviewData.format.label}</span>
                        {/if}
                        <span class="text-[var(--ink-400)] font-normal">{urlTotal} {urlRows ? $i18nT('pages.import.triples') : $i18nT('pages.import.lines')}</span>
                      </div>
                      <button
                        class="p-1 rounded-lg hover:bg-red-50 text-[var(--ink-400)] hover:text-red-500 transition-colors cursor-pointer"
                        on:click={() => urlPreviewData = null}
                        title={$i18nT('pages.import.dismissPreview')}
                      ><X size={14} /></button>
                    </div>
                    <!-- table or raw -->
                    <div class="overflow-x-auto max-h-64 overflow-y-auto">
                      {#if urlRows}
                        <table class="w-full text-[0.68rem] font-mono border-collapse">
                          <thead class="sticky top-0 bg-white/90"><tr class="text-left text-[var(--ink-500)]">
                            <th class="px-2 py-1 border-b border-[var(--line-soft)]">{$i18nT('pages.import.subject')}</th>
                            <th class="px-2 py-1 border-b border-[var(--line-soft)]">{$i18nT('pages.import.predicate')}</th>
                            <th class="px-2 py-1 border-b border-[var(--line-soft)]">{$i18nT('pages.import.object')}</th>
                          </tr></thead>
                          <tbody>
                            {#each urlRows.slice(urlStart, urlEnd) as row}
                              <tr class="border-b border-[var(--line-soft)]/60 hover:bg-[var(--bg-accent-soft)]">
                                <td class="px-2 py-1 max-w-[220px] truncate" title={row.s?.value}>{row.s?.value ?? ''}</td>
                                <td class="px-2 py-1 max-w-[220px] truncate" title={row.p?.value}>{row.p?.value ?? ''}</td>
                                <td class="px-2 py-1 max-w-[220px] truncate" title={row.o?.value}>{row.o?.value ?? ''}</td>
                              </tr>
                            {/each}
                          </tbody>
                        </table>
                      {:else}
                        <div class="divide-y divide-[var(--line-soft)]/40">
                          {#each urlRaw.slice(urlStart, urlEnd) as line}
                            <div class="px-3 py-1 text-[0.68rem] font-mono text-[var(--ink-700)] hover:bg-[var(--bg-accent-soft)] truncate" title={line}>{line}</div>
                          {/each}
                        </div>
                      {/if}
                    </div>
                    <!-- footer: pagination + confirm -->
                    <div class="flex items-center justify-between gap-2 px-3 py-2 border-t border-[var(--line-soft)] bg-white/60">
                      {#if urlTotalPages > 1}
                        <div class="flex items-center gap-1">
                          <button
                            class="p-1 rounded-lg disabled:opacity-30 hover:bg-[var(--bg-accent-soft)] transition-colors cursor-pointer disabled:cursor-not-allowed"
                            disabled={urlPreviewPage === 0}
                            on:click={() => urlPreviewPage--}
                          ><ChevronLeft size={14} /></button>
                          <span class="text-xs font-medium text-[var(--ink-600)] px-1">{urlPreviewPage + 1} / {urlTotalPages}</span>
                          <button
                            class="p-1 rounded-lg disabled:opacity-30 hover:bg-[var(--bg-accent-soft)] transition-colors cursor-pointer disabled:cursor-not-allowed"
                            disabled={urlPreviewPage >= urlTotalPages - 1}
                            on:click={() => urlPreviewPage++}
                          ><ChevronRight size={14} /></button>
                          <span class="text-xs text-[var(--ink-400)] ml-1">{$i18nT('pages.import.rangeOf', { values: { from: urlStart + 1, to: Math.min(urlEnd, urlTotal), total: urlTotal } })}</span>
                        </div>
                      {:else}
                        <span class="text-xs text-[var(--ink-400)]">{urlTotal} {urlRows ? $i18nT('pages.import.triples') : $i18nT('pages.import.lines')}</span>
                      {/if}
                      <button class="btn btn-sm" on:click={confirmAddUrl}>
                        <Plus size={13} />
                        {$i18nT('pages.import.addToImport')}
                      </button>
                    </div>
                  </div>
                {/if}
              </div>
            {/if}

            <!-- Drop zone -->
            <div
              class="border-2 border-dashed rounded-2xl p-8 text-center transition-all
                {dragOver ? 'border-[var(--brand-500)] bg-[var(--bg-accent-soft)]' : 'border-[var(--line-strong)]/40 bg-white/30'}"
              on:dragover|preventDefault={() => dragOver = true}
              on:dragleave={() => dragOver = false}
              on:drop={handleDrop}
              role="region"
              aria-label={$i18nT('pages.import.fileDropZone')}
            >
              <Upload size={32} class="mx-auto mb-3 text-[var(--ink-500)] opacity-50" />
              <p class="font-semibold text-sm mb-1">{$i18nT('pages.import.dragDrop')}</p>
              <p class="text-sm text-[var(--ink-500)] mb-3">{$i18nT('pages.import.or')}</p>
              <div class="flex gap-0 justify-center">
                <label class="btn btn-sm cursor-pointer inline-flex rounded-r-none border-r-0">
                  {$i18nT('pages.import.browseFiles')}
                  <input type="file" accept=".ttl,.n3,.nt,.nq,.trig,.rdf,.owl,.jsonld,.json" multiple on:change={handleFileInput} class="hidden" />
                </label>
                <div class="relative">
                  <button
                    type="button"
                    class="btn btn-sm rounded-l-none px-2 border-l border-[var(--brand-400)] flex items-center"
                    title={$i18nT('pages.import.browseFolder')}
                    on:click|stopPropagation={() => { browseDropdownOpen = !browseDropdownOpen; }}
                  >
                    <ChevronRight size={14} class="rotate-90" />
                  </button>
                  {#if browseDropdownOpen}
                    <!-- svelte-ignore a11y-click-events-have-key-events -->
                    <!-- svelte-ignore a11y-no-static-element-interactions -->
                    <div
                      class="absolute left-1/2 -translate-x-1/2 mt-1 z-20 bg-white border border-[var(--line-soft)] rounded-xl shadow-lg overflow-hidden text-sm min-w-[150px]"
                      on:click|stopPropagation
                    >
                      <label class="flex items-center gap-2 px-4 py-2.5 hover:bg-[var(--bg-accent-soft)] cursor-pointer whitespace-nowrap">
                        <Upload size={14} />
                        {$i18nT('pages.import.browseFiles')}
                        <input type="file" accept=".ttl,.n3,.nt,.nq,.trig,.rdf,.owl,.jsonld,.json" multiple on:change={e => { browseDropdownOpen = false; handleFileInput(e); }} class="hidden" />
                      </label>
                      <label class="flex items-center gap-2 px-4 py-2.5 hover:bg-[var(--bg-accent-soft)] cursor-pointer whitespace-nowrap">
                        <LayoutGrid size={14} />
                        {$i18nT('pages.import.browseFolder')}
                        <input type="file" webkitdirectory multiple on:change={e => { browseDropdownOpen = false; handleFolderInput(e); }} class="hidden" />
                      </label>
                    </div>
                  {/if}
                </div>
              </div>
              <p class="text-xs text-[var(--ink-500)] mt-3">{$i18nT('pages.import.formatHint')}</p>
            </div>

            <!-- File list -->
            {#if files.length > 0}
              <div class="space-y-2">
                <div class="text-sm font-semibold text-[var(--ink-700)]">
                  {$i18nT('pages.import.filesSelected', { values: { count: files.length } })}
                </div>
                {#each files as f, i}
                  {@const lower = f.file.name.toLowerCase()}
                  {@const isQuad = lower.endsWith('.nq') || lower.endsWith('.trig')}
                  {@const isNt = lower.endsWith('.nt')}
                  {@const iriInvalid = !isQuad && f.graphIri !== '' && !isValidIri(f.graphIri)}
                  {@const graphRoleEntries = Object.entries(f.graphRoles || {})}
                  {@const distinctGraphRoles = [...new Set(graphRoleEntries.map(([, r]) => r).filter(r => r && r !== 'unknown'))]}
                  <div class="rounded-xl bg-white/60 border border-[var(--line-soft)] overflow-hidden">
                    <!-- Header row -->
                    <div class="flex items-center gap-3 p-3">
                      <FileText size={20} class="text-[var(--brand-600)] shrink-0" />
                      <div class="flex-1 min-w-0">
                        <div class="font-semibold text-sm truncate">{f.file.name}</div>
                        <div class="text-xs text-[var(--ink-500)] flex items-center gap-1.5 flex-wrap">
                          <span>{(f.file.size / 1024).toFixed(1)} KB</span>
                          {#if f.format}
                            <span class="px-1.5 py-0.5 rounded bg-[var(--bg-accent-soft)] text-[var(--brand-600)] text-[0.7rem] font-medium">{f.format.label}</span>
                          {/if}
                          {#if isQuad && graphRoleEntries.length > 0}
                            <!-- Quad files carry named graphs: badge each graph's role, not a single "Mixed". -->
                            {#each distinctGraphRoles as role}
                              {@const n = graphRoleEntries.filter(([, r]) => r === role).length}
                              <span class="px-1.5 py-0.5 rounded text-[0.7rem] font-medium {roleChipClass(role)}" title={$i18nT('pages.import.embeddedGraphsDetectedAs', { values: { count: n, role } })}>{role}{n > 1 ? ` ×${n}` : ''}</span>
                            {/each}
                            {#if distinctGraphRoles.length === 0}
                              <span class="px-1.5 py-0.5 rounded text-[0.7rem] font-medium bg-slate-100 text-slate-600" title={$i18nT('pages.import.embeddedNotClassified')}>{$i18nT('pages.import.unclassified')}</span>
                            {/if}
                          {:else if f.detectedKind === 'model'}
                            <span class="px-1.5 py-0.5 rounded text-[0.7rem] font-medium bg-blue-100 text-blue-800" title={$i18nT('pages.import.kindModelTitle')}>{$i18nT('pages.import.kindModel')}</span>
                          {:else if f.detectedKind === 'vocabulary'}
                            <span class="px-1.5 py-0.5 rounded text-[0.7rem] font-medium bg-purple-100 text-purple-800" title={$i18nT('pages.import.kindVocabularyTitle')}>{$i18nT('pages.import.kindVocabulary')}</span>
                          {:else if f.detectedKind === 'shapes'}
                            <span class="px-1.5 py-0.5 rounded text-[0.7rem] font-medium bg-orange-100 text-orange-800" title={$i18nT('pages.import.kindShapesTitle')}>{$i18nT('pages.import.kindShapes')}</span>
                          {:else if f.detectedKind === 'entailment'}
                            <span class="px-1.5 py-0.5 rounded text-[0.7rem] font-medium bg-amber-100 text-amber-800" title={$i18nT('pages.import.kindEntailmentTitle')}>{$i18nT('pages.import.kindEntailment')}</span>
                          {:else if f.detectedKind === 'instances'}
                            <span class="px-1.5 py-0.5 rounded text-[0.7rem] font-medium bg-green-100 text-green-800" title={$i18nT('pages.import.kindInstancesTitle')}>{$i18nT('pages.import.kindInstances')}</span>
                          {:else if f.detectedKind === 'mixed'}
                            {#if f.analyzeResult?.splits?.length}
                              {#each f.analyzeResult.splits as sp}
                                <span class="px-1.5 py-0.5 rounded text-[0.7rem] font-medium {roleChipClass(sp.role)}" title={$i18nT('pages.import.triplesDetectedAs', { values: { count: sp.triple_count, role: sp.role } })}>{sp.role}</span>
                              {/each}
                            {:else if f.analyzing}
                              <span class="px-1.5 py-0.5 rounded text-[0.7rem] font-medium bg-slate-100 text-slate-600 inline-flex items-center gap-1" title={$i18nT('pages.import.detectingRoles')}><Loader2 size={9} class="animate-spin" />{$i18nT('pages.import.mixedAnalyzing')}</span>
                            {:else}
                              <span class="px-1.5 py-0.5 rounded text-[0.7rem] font-medium bg-red-100 text-red-700" title={$i18nT('pages.import.mixedConsiderSplit')}>{$i18nT('pages.import.kindMixed')}</span>
                            {/if}
                          {/if}
                          {#if f.fromUrl}
                            <span class="truncate opacity-60">{f.fromUrl}</span>
                          {/if}
                          {#if isQuad && f.detectedGraphIris?.length > 0}
                            <span class="flex items-center gap-1 text-emerald-700 font-medium">
                              <Zap size={11} />
                              {$i18nT('pages.import.graphsEmbedded', { values: { count: f.detectedGraphIris.length } })}
                            </span>
                          {/if}

                        </div>
                      </div>
                      <!-- Preview toggle -->
                      <button
                        class="p-1.5 rounded-lg text-[var(--ink-400)] hover:text-[var(--brand-600)] hover:bg-[var(--bg-accent-soft)] transition-colors cursor-pointer"
                        on:click={() => togglePreview(i)}
                        title={$i18nT('pages.import.togglePreview')}
                      >
                        <Eye size={15} />
                      </button>
                      <!-- Remove -->
                      <button
                        class="p-1.5 rounded-lg hover:bg-red-50 text-[var(--ink-500)] hover:text-red-500 transition-colors cursor-pointer"
                        on:click={() => removeFile(i)}
                        title={$i18nT('pages.import.removeFile')}
                      >
                        <X size={16} />
                      </button>
                    </div>

                    <!-- Graph IRI section -->
                    <div class="px-3 pb-3 space-y-2 border-t border-[var(--line-soft)] pt-3">
                      {#if isQuad}
                        {#if f.detectedGraphIris.length === 0}
                          <p class="text-xs text-[var(--ink-500)] italic">{$i18nT('pages.import.noGraphIrisDetected')}</p>
                        {:else}
                          {#each f.detectedGraphIris as orig}
                            {@const renamed = f.graphIriRenameMap[orig] ?? orig}
                            {@const renameInvalid = renamed !== '' && renamed !== orig && !isValidIri(renamed)}
                            {@const graphRole = f.graphRoles?.[orig]}
                            <div class="space-y-1">
                              <div class="text-xs text-[var(--ink-500)] font-medium flex items-center gap-1.5 flex-wrap">
                                <Zap size={11} class="text-emerald-600" />
                                {$i18nT('pages.import.embeddedLabel')} <code class="text-[0.7rem] bg-slate-100 px-1 rounded break-all">{orig}</code>
                                {#if graphRole && graphRole !== 'unknown'}
                                  <span class="px-1.5 py-0.5 rounded text-[0.65rem] font-medium {roleChipClass(graphRole)}" title={$i18nT('pages.import.detectedRoleTitle')}>{graphRole}</span>
                                {/if}
                              </div>
                              <div class="flex items-center gap-2">
                                <span class="text-xs text-[var(--ink-400)] shrink-0">{$i18nT('pages.import.renameTo')}</span>
                                <input
                                  id="quad-rename-{i}"
                                  class="flex-1 font-mono text-xs {renameInvalid ? 'border-red-400' : ''}"
                                  value={renamed}
                                  placeholder={orig}
                                  on:input={e => setQuadRename(i, orig, e.currentTarget.value)}
                                />
                              </div>
                              {#if renameInvalid}
                                <p class="text-xs text-red-600">{$i18nT('pages.import.graphIriAbsolute')}</p>
                              {/if}
                            </div>
                          {/each}
                        {/if}
                        {#if f.hasDefaultGraphTriples && selectedDatasetId}
                          <div class="flex items-start gap-1.5 text-xs text-[var(--ink-500)]">
                            <Info size={12} class="text-[var(--brand-500)] shrink-0 mt-0.5" />
                            <span>{$i18nT('pages.import.defaultGraphRoutedNote')}</span>
                          </div>
                        {/if}
                      {:else}
                        <div class="flex items-center gap-2">
                          <Target size={13} class="text-[var(--brand-500)] shrink-0" />
                          <span class="text-xs font-medium text-[var(--ink-600)] shrink-0">{$i18nT('pages.import.targetGraphLabel')}</span>
                          <input
                            class="flex-1 font-mono text-xs {iriInvalid ? 'border-red-400' : ''}"
                            value={f.graphIri}
                            placeholder="https://example.org/my-graph"
                            on:input={e => setFileGraphIri(i, e.currentTarget.value)}
                          />
                        </div>
                        {#if iriInvalid}
                          <p class="text-xs text-red-600">{$i18nT('pages.import.graphIriAbsolute')}</p>
                        {:else if f.graphIri === ''}
                          <p class="text-xs text-amber-600">{$i18nT('pages.import.targetGraphRequired')}</p>
                        {/if}
                        <!-- Graph role: prefilled from auto-detection, editable; can be
                             set even when nothing was detected. Hidden when auto-split
                             is on (roles are derived per sub-graph then). -->
                        {#if !(f.detectedKind === 'mixed' && f.autoSplit)}
                          <div class="flex items-center gap-2">
                            <Tag size={13} class="text-[var(--brand-500)] shrink-0" />
                            <span class="text-xs font-medium text-[var(--ink-600)] shrink-0">{$i18nT('pages.import.graphRoleLabel')}</span>
                            <Select
                              size="sm"
                              value={f.graphRole}
                              on:change={e => setFileGraphRole(i, e.detail)}
                              options={[{ value: '', label: $i18nT('pages.import.autoDetectOption') }, ...ROLE_OPTIONS.map(role => ({ value: role, label: $i18nT('pages.import.role.' + role) }))]}
                            />
                            {#if f.graphRole === '' && (f.detectedKind === 'unknown' || f.detectedKind === undefined)}
                              <span class="text-[0.7rem] text-amber-600">{$i18nT('pages.import.noRoleDetected')}</span>
                            {/if}
                          </div>
                        {/if}
                      {/if}
                    </div>

                    <!-- Split suggestion panel (mixed content) -->
                    {#if !isQuad && f.detectedKind === 'mixed'}
                      <div class="border-t border-[var(--line-soft)] bg-amber-50 px-3 py-2">
                        {#if f.autoSplit}
                          <div class="flex items-center gap-2 text-xs text-amber-900">
                            <Check size={13} class="text-green-600 shrink-0" />
                            <span class="flex-1">{$i18nT('pages.import.autoSplitEnabled')}</span>
                            <button class="btn btn-xs btn-ghost" on:click={() => toggleAutoSplit(i, false)}>{$i18nT('pages.import.undo')}</button>
                          </div>
                          {#if f.analyzeResult}
                            <div class="mt-2 space-y-1">
                              {#each f.analyzeResult.splits as sp}
                                <div class="flex items-center gap-2 text-xs">
                                  <span class="px-1.5 py-0.5 rounded font-medium {roleChipClass(sp.role)}">{sp.role}</span>
                                  <span class="font-mono text-[var(--ink-500)] truncate">{f.graphIri}{sp.suggested_suffix}</span>
                                  <span class="ml-auto shrink-0 text-[var(--ink-400)]">{$i18nT('pages.import.nTriples', { values: { count: sp.triple_count } })}</span>
                                </div>
                              {/each}
                            </div>
                          {/if}
                        {:else}
                          <div class="flex items-center gap-2">
                            <AlertTriangle size={13} class="text-amber-600 shrink-0" />
                            <span class="text-xs text-amber-800 flex-1">{$i18nT('pages.import.mixedContentDetected')}</span>
                            {#if f.analyzeResult}
                              <button class="btn btn-xs btn-primary" on:click={() => toggleAutoSplit(i, true)}>
                                {$i18nT('pages.import.acceptAutoSplit', { values: { count: f.analyzeResult.splits.length } })}
                              </button>
                            {:else}
                              <button class="btn btn-xs btn-ghost border border-amber-300" on:click={() => analyzeFile(i)} disabled={f.analyzing}>
                                {#if f.analyzing}<Loader2 size={11} class="animate-spin" />{:else}{$i18nT('pages.import.analyzeSplit')}{/if}
                              </button>
                            {/if}
                          </div>
                          {#if f.analyzeResult && !f.autoSplit}
                            <div class="mt-2 space-y-1">
                              {#each f.analyzeResult.splits as sp}
                                <div class="flex items-center gap-2 text-xs">
                                  <span class="px-1.5 py-0.5 rounded font-medium {roleChipClass(sp.role)}">{sp.role}</span>
                                  <span class="font-mono text-[var(--ink-500)] truncate">{f.graphIri}{sp.suggested_suffix}</span>
                                  <span class="ml-auto shrink-0 text-[var(--ink-400)]">{$i18nT('pages.import.nTriples', { values: { count: sp.triple_count } })}</span>
                                </div>
                              {/each}
                            </div>
                          {/if}
                        {/if}
                      </div>
                    {/if}

                    <!-- Preview panel (full file + search) -->
                    {#if f.showPreview}
                      {@const allLines = f.content.split('\n')}
                      {@const searchLower = f.previewSearch.toLowerCase()}
                      {@const matchedLines = searchLower
                        ? allLines.map((line, n) => ({ n: n + 1, line, match: line.toLowerCase().includes(searchLower) }))
                        : allLines.map((line, n) => ({ n: n + 1, line, match: false }))}
                      {@const displayLines = searchLower ? matchedLines.filter(l => l.match) : matchedLines}
                      <div class="border-t border-[var(--line-soft)] bg-slate-50 p-3 space-y-2">
                        <!-- Search bar -->
                        <div class="flex items-center gap-2">
                          <input
                            class="flex-1 text-xs py-1 px-2"
                            placeholder={$i18nT('pages.import.searchInFile')}
                            value={f.previewSearch}
                            on:input={e => setFilePreviewSearch(i, e.currentTarget.value)}
                          />
                          <span class="text-[0.68rem] text-[var(--ink-500)] shrink-0 whitespace-nowrap">
                            {#if searchLower}
                              {$i18nT('pages.import.linesOf', { values: { shown: displayLines.length, total: allLines.length } })}
                            {:else}
                              {$i18nT('pages.import.nLines', { values: { count: allLines.length } })}
                            {/if}
                          </span>
                        </div>
                        <!-- Line-numbered content -->
                        <div class="overflow-auto max-h-72 bg-white border border-[var(--line-soft)] rounded text-[0.68rem] font-mono leading-relaxed">
                          <table class="w-full border-collapse">
                            <tbody>
                              {#each displayLines as { n, line, match }}
                                <tr class="{match ? 'bg-yellow-50' : 'hover:bg-slate-50/50'}">
                                  <td class="select-none text-right px-2 py-0.5 text-[var(--ink-300)] border-r border-[var(--line-soft)] w-10 shrink-0">{n}</td>
                                  <td class="px-2 py-0.5 whitespace-pre break-all">{line}</td>
                                </tr>
                              {/each}
                              {#if displayLines.length === 0 && searchLower}
                                <tr><td colspan="2" class="px-3 py-2 text-[var(--ink-400)] italic">{$i18nT('pages.import.noLinesMatch', { values: { term: f.previewSearch } })}</td></tr>
                              {/if}
                            </tbody>
                          </table>
                        </div>
                        <!-- Parsed triple table for .nt files -->
                        {#if isNt && f.parsedPreview?.length > 0 && !searchLower}
                          <div>
                            <div class="text-xs font-semibold text-[var(--ink-600)] mb-1">
                              {$i18nT('pages.import.firstNTriplesParsed', { values: { count: f.parsedPreview.length } })}
                            </div>
                            <div class="overflow-x-auto">
                              <table class="w-full text-[0.68rem] border-collapse bg-white border border-[var(--line-soft)] rounded">
                                <thead>
                                  <tr class="bg-[var(--bg-accent-soft)]">
                                    <th class="text-left px-2 py-1 font-semibold border-b border-[var(--line-soft)]">{$i18nT('pages.import.subject')}</th>
                                    <th class="text-left px-2 py-1 font-semibold border-b border-[var(--line-soft)]">{$i18nT('pages.import.predicate')}</th>
                                    <th class="text-left px-2 py-1 font-semibold border-b border-[var(--line-soft)]">{$i18nT('pages.import.object')}</th>
                                  </tr>
                                </thead>
                                <tbody>
                                  {#each f.parsedPreview as row}
                                    <tr class="border-b border-[var(--line-soft)]/50 hover:bg-slate-50/60">
                                      <td class="px-2 py-1 font-mono break-all max-w-[200px] truncate" title={row.s?.value}>{row.s?.value ?? ''}</td>
                                      <td class="px-2 py-1 font-mono break-all max-w-[200px] truncate" title={row.p?.value}>{row.p?.value ?? ''}</td>
                                      <td class="px-2 py-1 font-mono break-all max-w-[200px] truncate" title={row.o?.value}>{row.o?.value ?? ''}</td>
                                    </tr>
                                  {/each}
                                </tbody>
                              </table>
                            </div>
                          </div>
                        {/if}
                      </div>
                    {/if}
                  </div>
                {/each}
              </div>

              <!-- Content-kind routing banner -->
              {#if dominantKind && !kindWarningDismissed}
                <div class="rounded-xl border p-3 flex items-start gap-3
                  {dominantKind === 'vocabulary' ? 'border-purple-200 bg-purple-50' : 'border-amber-200 bg-amber-50'}">
                  <AlertTriangle size={16} class="{dominantKind === 'vocabulary' ? 'text-purple-600' : 'text-amber-600'} shrink-0 mt-0.5" />
                  <div class="flex-1 min-w-0 text-sm">
                    <strong class="{dominantKind === 'vocabulary' ? 'text-purple-900' : 'text-amber-900'}">
                      {dominantKind === 'vocabulary'
                        ? $i18nT('pages.import.looksLikeVocabulary')
                        : $i18nT('pages.import.looksLikeModel')}
                    </strong>
                    <p class="text-xs mt-0.5 {dominantKind === 'vocabulary' ? 'text-purple-800' : 'text-amber-800'}">
                      {dominantKind === 'vocabulary'
                        ? $i18nT('pages.import.vocabularyRegistryHint')
                        : $i18nT('pages.import.modelRegistryHint')}
                    </p>
                  </div>
                  <div class="flex gap-1.5 shrink-0">
                    <button
                      class="btn btn-sm {dominantKind === 'vocabulary' ? 'bg-purple-600 hover:bg-purple-700 text-white border-0' : 'bg-amber-600 hover:bg-amber-700 text-white border-0'}"
                      on:click={() => navigate(dominantKind === 'vocabulary' ? '/models?kind=vocabulary' : '/models')}
                    >
                      {dominantKind === 'vocabulary' ? $i18nT('pages.import.goToVocabularyRegistry') : $i18nT('pages.import.goToModelRegistry')}
                    </button>
                    <button class="btn btn-sm btn-ghost" on:click={() => kindWarningDismissed = true}>
                      {$i18nT('pages.import.continueAnyway')}
                    </button>
                  </div>
                </div>
              {/if}

            {/if}
          {/if}

          <div class="flex justify-end pt-2">
            <button class="btn" on:click={goToStep2} disabled={!canStep1}>
              {$i18nT('system.next')} <ChevronRight size={16} />
            </button>
          </div>
        </div>
      {/if}

      <!-- ═══════════════════════════════════════════════════════════════════ -->
      <!-- Step 2: Owner & Dataset                                             -->
      <!-- ═══════════════════════════════════════════════════════════════════ -->
      {#if step === 2}
        <div class="space-y-5">
          <!-- Owner selection -->
          <div>
            <h3 class="text-base font-bold mb-3">{$i18nT('pages.import.ownerHeading')}</h3>
            <div class="grid grid-cols-1 sm:grid-cols-2 gap-2">
              <!-- Personal account -->
              <button
                class="flex items-center gap-3 p-4 rounded-xl border-2 text-left transition-all cursor-pointer
                  {ownerType === 'personal' ? 'border-[var(--brand-500)] bg-[var(--bg-accent-soft)]' : 'border-[var(--line-soft)] bg-white/50 hover:border-[var(--brand-300)]'}"
                on:click={selectPersonal}
              >
                <div class="w-9 h-9 rounded-full bg-[var(--bg-accent-soft)] flex items-center justify-center shrink-0">
                  <User size={18} class="text-[var(--brand-600)]" />
                </div>
                <div>
                  <div class="font-semibold text-sm">{$i18nT('pages.import.personalAccount')}</div>
                  <div class="text-xs text-[var(--ink-500)]">{currentUser?.username}</div>
                </div>
                {#if ownerType === 'personal'}
                  <Check size={16} class="ml-auto text-[var(--brand-500)]" />
                {/if}
              </button>

              <!-- Organisation cards -->
              {#each organisations as org}
                <button
                  class="flex items-center gap-3 p-4 rounded-xl border-2 text-left transition-all cursor-pointer
                    {ownerType === 'org' && selectedOrgId === org.id ? 'border-[var(--brand-500)] bg-[var(--bg-accent-soft)]' : 'border-[var(--line-soft)] bg-white/50 hover:border-[var(--brand-300)]'}"
                  on:click={() => selectOrg(org.id)}
                >
                  <div class="w-9 h-9 rounded-full bg-[var(--bg-accent-soft)] flex items-center justify-center shrink-0">
                    <Building2 size={18} class="text-[var(--brand-600)]" />
                  </div>
                  <div>
                    <div class="font-semibold text-sm">{org.name}</div>
                    <div class="text-xs text-[var(--ink-500)]">{$i18nT('pages.import.organisation')}</div>
                  </div>
                  {#if ownerType === 'org' && selectedOrgId === org.id}
                    <Check size={16} class="ml-auto text-[var(--brand-500)]" />
                  {/if}
                </button>
              {/each}
            </div>
          </div>

          <!-- Dataset selection -->
          <div>
            <h3 class="text-base font-bold mb-3">{$i18nT('pages.import.datasetHeading')}</h3>

            <!-- Mode tabs -->
            <div class="flex gap-2 mb-3">
              {#if ownerDatasets.length > 0}
                <button
                  class="flex items-center gap-2 px-3 py-2 rounded-lg text-sm font-medium transition-all cursor-pointer
                    {datasetMode === 'existing' ? 'bg-[var(--brand-500)] text-white' : 'bg-white/60 border border-[var(--line-soft)] text-[var(--ink-700)] hover:bg-white'}"
                  on:click={() => datasetMode = 'existing'}
                >
                  <Database size={14} />
                  {$i18nT('pages.import.selectDataset')}
                  <span class="ml-1 px-1.5 py-0.5 rounded-full text-[0.65rem] font-bold
                    {datasetMode === 'existing' ? 'bg-white/30' : 'bg-[var(--brand-100)] text-[var(--brand-700)]'}">
                    {ownerDatasets.length}
                  </span>
                </button>
              {/if}
              <button
                class="flex items-center gap-2 px-3 py-2 rounded-lg text-sm font-medium transition-all cursor-pointer
                  {datasetMode === 'new' ? 'bg-[var(--brand-500)] text-white' : 'bg-white/60 border border-[var(--line-soft)] text-[var(--ink-700)] hover:bg-white'}"
                on:click={() => datasetMode = 'new'}
              >
                <Plus size={14} />
                {$i18nT('pages.import.createNewDataset')}
              </button>
            </div>

            {#if datasetMode === 'existing'}
              {#if ownerDatasets.length === 0}
                <p class="text-sm text-[var(--ink-500)] py-4">{$i18nT('pages.import.noDatasets')}</p>
              {:else}
                <div class="grid gap-2 max-h-72 overflow-y-auto pr-1">
                  {#each ownerDatasets as ds}
                    <button
                      class="flex items-center gap-3 p-3 rounded-xl border-2 text-left transition-all cursor-pointer
                        {selectedDatasetId === ds.id ? 'border-[var(--brand-500)] bg-[var(--bg-accent-soft)]' : 'border-[var(--line-soft)] bg-white/50 hover:border-[var(--brand-300)]'}"
                      on:click={() => selectedDatasetId = ds.id}
                    >
                      <Database size={18} class="shrink-0 text-[var(--ink-400)]" />
                      <div class="min-w-0 flex-1">
                        <div class="font-semibold text-sm">{ds.name}</div>
                        {#if ds.description}
                          <div class="text-xs text-[var(--ink-500)] truncate">{ds.description}</div>
                        {/if}
                      </div>
                      <!-- Visibility badge -->
                      <span class="text-[0.65rem] px-1.5 py-0.5 rounded font-medium shrink-0
                        {ds.visibility === 'public' ? 'bg-green-100 text-green-700' : ds.visibility === 'members' ? 'bg-blue-100 text-blue-700' : 'bg-slate-100 text-slate-600'}">
                        {ds.visibility === 'public' ? $i18nT('pages.import.visPublic') : ds.visibility === 'members' ? $i18nT('pages.import.visMembers') : $i18nT('pages.import.visPrivate')}
                      </span>
                      {#if isAdminValue}
                        {@const ownerName = ds.owner_type === 'organisation'
                          ? (organisations.find(o => o.id === ds.owner_id)?.name ?? String(ds.owner_id))
                          : (adminUserMap[String(ds.owner_id)] ?? String(ds.owner_id))}
                        <span class="text-[0.65rem] px-1.5 py-0.5 rounded font-medium bg-purple-100 text-purple-700 shrink-0">
                          {ownerName}
                        </span>
                      {/if}
                      {#if selectedDatasetId === ds.id}
                        <Check size={18} class="text-[var(--brand-500)] shrink-0" />
                      {/if}
                    </button>
                  {/each}
                </div>
              {/if}
            {:else}
              <div class="space-y-3 p-4 rounded-xl bg-white/40 border border-[var(--line-soft)]">
                <div class="form-group">
                  <label for="new-ds-name">{$i18nT('pages.import.datasetName')}</label>
                  <input id="new-ds-name" bind:value={newDatasetName} class="{datasetNameTaken ? 'border-red-400' : ''}" />
                  {#if datasetNameTaken}
                    <p class="text-xs text-red-600 mt-1">{$i18nT('pages.import.datasetNameTaken')}</p>
                  {/if}
                </div>
                <div class="form-group">
                  <label for="new-ds-desc">{$i18nT('pages.import.datasetDescription')}</label>
                  <input id="new-ds-desc" bind:value={newDatasetDesc} />
                </div>
                <div class="form-group">
                  <span class="group-label">{$i18nT('pages.import.datasetVisibility')}</span>
                  <div class="flex gap-2">
                    {#each VIS_OPTIONS as { val, Icon, labelKey }}
                      <button
                        class="flex items-center gap-1.5 px-3 py-2 rounded-lg text-sm transition-all cursor-pointer
                          {newDatasetVis === val ? 'bg-[var(--brand-500)] text-white' : 'bg-white/80 border border-[var(--line-soft)] text-[var(--ink-700)] hover:bg-white'}"
                        on:click={() => newDatasetVis = val}
                      >
                        <svelte:component this={Icon} size={14} />
                        {$i18nT(`pages.import.${labelKey}`)}
                      </button>
                    {/each}
                  </div>
                </div>
              </div>
            {/if}
          </div>

          <div class="flex justify-between pt-2">
            <button class="btn btn-ghost" on:click={() => step = 1}>
              <ChevronLeft size={16} /> {$i18nT('system.back')}
            </button>
            <button class="btn" on:click={goToStep3} disabled={!canStep2}>
              {$i18nT('system.next')} <ChevronRight size={16} />
            </button>
          </div>
        </div>
      {/if}

      <!-- ═══════════════════════════════════════════════════════════════════ -->
      <!-- Step 3: Review & Import                                             -->
      <!-- ═══════════════════════════════════════════════════════════════════ -->
      {#if step === 3}
        <div class="space-y-5">
          {#if !importResult}
            <!-- Loading indicator while fetching dataset graphs/details -->
            {#if !useSprarqlUpdate && loadingDatasetGraphs}
              <div class="flex items-center gap-2 text-sm text-[var(--ink-500)]">
                <Loader2 size={14} class="animate-spin" /> {$i18nT('system.loading')}
              </div>
            {/if}

            <!-- ── Optional Validation (only when dataset has shapes configured) -->
            {#if selectedDatasetShapesIri && datasetMode !== 'new'}
              <div>
                <h3 class="text-base font-bold mb-3">{$i18nT('pages.import.validateOptional')}</h3>
                <label class="di-toggle" style="margin-bottom:0.75rem">
                  <input type="checkbox" bind:checked={doValidate} />
                  <span class="di-track"><span class="di-thumb"></span></span>
                  <span class="di-label">{$i18nT('pages.import.runShaclBefore')}</span>
                </label>

                {#if doValidate}
                  <div class="flex gap-2 items-end mb-2">
                    <div class="form-group flex-1" style="margin-bottom:0">
                      <label for="import-validation-ds">{$i18nT('pages.import.shaclDataset')}</label>
                      <Select
                        id="import-validation-ds"
                        bind:value={validationDatasetId}
                        placeholder={$i18nT('pages.import.selectDatasetForShacl')}
                        options={[{ value: '', label: $i18nT('pages.import.selectDatasetForShacl') }, ...datasets.map(ds => ({ value: ds.id, label: ds.name }))]}
                      />
                    </div>
                    <button class="btn btn-sm" on:click={runPreValidation} disabled={!validationDatasetId || validating}>
                      {#if validating}<Loader2 size={14} class="animate-spin" />{/if}
                      {validating ? $i18nT('pages.import.validating') : $i18nT('pages.import.runPreValidation')}
                    </button>
                  </div>
                  {#if preValidationResult}
                    <div class="flex items-center gap-2 p-3 rounded-xl {preValidationResult.error ? 'bg-red-50 text-red-800' : preValidationResult.conforms ? 'bg-green-50 text-green-800' : 'bg-amber-50 text-amber-800'}">
                      {#if preValidationResult.error}
                        <AlertTriangle size={18} /> {preValidationResult.error}
                      {:else if preValidationResult.conforms}
                        <Check size={18} /> {$i18nT('pages.import.dataConforms')}
                      {:else}
                        <AlertTriangle size={18} /> {$i18nT('pages.import.issuesFound', { values: { count: preValidationResult.results_count } })}
                      {/if}
                    </div>
                  {/if}
                {/if}
              </div>
            {/if}

            <!-- ── Import Summary ──────────────────────────────────────────── -->
            <div>
              <h3 class="text-base font-bold mb-3">{$i18nT('pages.import.importSummary')}</h3>
              <div class="p-4 rounded-xl bg-white/50 border border-[var(--line-soft)] space-y-3 text-sm">
                {#if useSprarqlUpdate}
                  <div class="flex gap-2">
                    <span class="font-medium text-[var(--ink-500)] w-24 shrink-0">{$i18nT('pages.import.type')}:</span>
                    {$i18nT('pages.import.sparqlUpdate')}
                  </div>
                {:else}
                  <div>
                    <span class="font-medium text-[var(--ink-500)]">{$i18nT('pages.import.files')} ({files.length}):</span>
                    <div class="mt-1.5 space-y-1 max-h-64 overflow-y-auto">
                      {#each files as f, i}
                        {@const isQuad = f.file.name.toLowerCase().endsWith('.nq') || f.file.name.toLowerCase().endsWith('.trig')}
                        <div class="p-2 rounded-lg bg-white/60 border border-[var(--line-soft)] space-y-2">
                          <div class="flex items-center gap-2">
                            <FileText size={14} class="text-[var(--brand-500)] shrink-0" />
                            <span class="text-xs font-medium truncate flex-1">{f.file.name}</span>
                            <span class="text-[0.65rem] text-[var(--ink-400)]">{(f.file.size / 1024).toFixed(1)} KB</span>
                            {#if f.format}
                              <span class="text-[0.6rem] px-1.5 py-0.5 rounded bg-[var(--bg-accent-soft)] text-[var(--brand-600)] font-medium">{f.format.label}</span>
                            {/if}
                            <span class="text-[0.6rem] text-[var(--ink-400)] font-mono truncate max-w-[200px]" title={isQuad ? Object.values(f.graphIriRenameMap).filter(Boolean).join(', ') : f.graphIri}>
                              → {isQuad ? Object.values(f.graphIriRenameMap).filter(Boolean).join(', ') || $i18nT('pages.import.embedded') : f.graphIri || '—'}
                            </span>
                          </div>
                          <!-- Per-file write mode: merge (POST) vs replace (PUT) -->
                          <div class="flex items-center gap-2 pl-6">
                            <div class="inline-flex rounded-lg border border-[var(--line-soft)] overflow-hidden text-[0.7rem] font-medium">
                              <button
                                type="button"
                                class="px-2.5 py-1 cursor-pointer transition-colors {!f.replace ? 'bg-[var(--brand-500)] text-white' : 'bg-white/60 text-[var(--ink-600)] hover:bg-white'}"
                                on:click={() => setFileReplace(i, false)}
                              >{$i18nT('pages.import.merge')}</button>
                              <button
                                type="button"
                                class="px-2.5 py-1 cursor-pointer transition-colors {f.replace ? 'bg-red-500 text-white' : 'bg-white/60 text-[var(--ink-600)] hover:bg-white'}"
                                on:click={() => setFileReplace(i, true)}
                              >{$i18nT('pages.import.replace')}</button>
                            </div>
                            {#if f.replace && datasetMode !== 'new'}
                              {@const gcount = isQuad ? (f.detectedGraphIris?.length || 1) : 1}
                              <span class="text-[0.65rem] text-amber-700 inline-flex items-center gap-1">
                                <Shield size={11} /> {gcount > 1 ? $i18nT('pages.import.replaceArchiveNotePlural') : $i18nT('pages.import.replaceArchiveNote')}
                              </span>
                            {/if}
                          </div>
                        </div>
                      {/each}
                    </div>
                  </div>
                  {#if anyReplaceExisting}
                    <div class="mt-2 p-3 rounded-xl bg-amber-50 border border-amber-200 space-y-2">
                      <div class="flex items-center gap-2 flex-wrap">
                        <GitBranch size={14} class="text-amber-700 shrink-0" />
                        <span class="text-xs font-medium text-[var(--ink-700)]">{$i18nT('pages.import.versionBumpLabel')}:</span>
                        <div class="inline-flex rounded-lg border border-amber-300 overflow-hidden text-[0.7rem] font-medium">
                          {#each ['patch', 'minor', 'major'] as level}
                            <button
                              type="button"
                              class="px-2.5 py-1 cursor-pointer transition-colors {versionBump === level ? 'bg-amber-500 text-white' : 'bg-white/70 text-[var(--ink-600)] hover:bg-white'}"
                              on:click={() => versionBump = level}
                            >{$i18nT('pages.import.bump' + level[0].toUpperCase() + level.slice(1))}</button>
                          {/each}
                        </div>
                      </div>
                      <p class="text-[0.65rem] text-[var(--ink-500)] flex items-start gap-1">
                        <Info size={11} class="shrink-0 mt-0.5" /> {$i18nT('pages.import.versionBumpHint')}
                      </p>
                    </div>
                  {/if}
                {/if}
                <div class="flex gap-2">
                  <span class="font-medium text-[var(--ink-500)] w-24 shrink-0">{$i18nT('pages.import.owner')}:</span>
                  {ownerLabel}
                </div>
                <div class="flex gap-2">
                  <span class="font-medium text-[var(--ink-500)] w-24 shrink-0">{$i18nT('pages.import.dataset')}:</span>
                  {datasetLabel || '—'}
                </div>
              </div>
            </div>

            <!-- Pre-flight: quad embedded graphs whose write target falls outside
                 this dataset (the server's per-graph boundary would 403 them).
                 One click re-homes them under the dataset namespace. -->
            {#if !importResult && !importing && selectedDatasetIri && foreignQuadTargets.length > 0}
              <div class="p-3 rounded-xl bg-amber-50 border border-amber-200">
                <div class="flex items-start gap-2">
                  <AlertTriangle size={15} class="text-amber-700 shrink-0 mt-0.5" />
                  <div class="flex-1 min-w-0">
                    <p class="text-sm font-semibold text-[var(--ink-700)]">{$i18nT('pages.import.preflightForeignTitle')}</p>
                    <p class="text-xs text-[var(--ink-500)] mt-0.5">{$i18nT('pages.import.preflightForeignBody', { values: { count: foreignQuadTargets.length } })}</p>
                  </div>
                  <button class="btn btn-sm shrink-0 whitespace-nowrap" on:click={namespaceForeignQuadTargets}>
                    {$i18nT('pages.import.preflightNamespaceBtn')}
                  </button>
                </div>
              </div>
            {/if}

            <!-- Pre-flight: distinct embedded graphs that would collapse into one
                 target (silent merge / data loss). Not auto-fixable — rename them. -->
            {#if !importResult && !importing && mergedQuadTargets.length > 0}
              <div class="p-3 rounded-xl bg-red-50 border border-red-200">
                <p class="text-xs text-red-700 flex items-start gap-2">
                  <AlertTriangle size={14} class="shrink-0 mt-0.5" />
                  <span>{$i18nT('pages.import.preflightMergeWarning', { values: { count: mergedQuadTargets.reduce((a, m) => a + m.count, 0) } })}</span>
                </p>
              </div>
            {/if}

            {#if importError && !importResult}
              <div class="error">{importError}</div>
            {/if}

            <!-- Progress bar during import -->
            {#if importing && !useSprarqlUpdate && importProgress.total > 0}
              <div class="space-y-2">
                <div class="flex items-center justify-between text-sm">
                  <span class="text-[var(--ink-600)] font-medium truncate flex-1">
                    {$i18nT('pages.import.uploadingFile', { values: { file: importProgress.currentFile } })}
                  </span>
                  <span class="text-[var(--ink-500)] shrink-0 ml-2">
                    {importProgress.current} / {importProgress.total}
                  </span>
                </div>
                <div class="w-full h-2 bg-[var(--line-soft)] rounded-full overflow-hidden">
                  <div
                    class="h-full bg-[var(--brand-500)] rounded-full transition-all duration-300"
                    style="width: {(importProgress.current / importProgress.total * 100).toFixed(1)}%"
                  ></div>
                </div>
                <!-- Per-file results so far -->
                {#if fileResults.length > 0}
                  <div class="max-h-32 overflow-y-auto space-y-1 mt-2">
                    {#each fileResults as fr}
                      <div class="flex items-center gap-2 text-xs px-2 py-1 rounded {fr.status === 'ok' ? 'bg-green-50 text-green-700' : 'bg-red-50 text-red-700'}">
                        {#if fr.status === 'ok'}
                          <Check size={12} />
                        {:else}
                          <AlertTriangle size={12} />
                        {/if}
                        <span class="truncate">{fr.name}</span>
                        {#if fr.status === 'error'}
                          <span class="truncate opacity-75">— {fr.error}</span>
                        {/if}
                      </div>
                    {/each}
                  </div>
                {/if}
              </div>
            {/if}

            <button
              class="btn w-full py-3 text-base"
              on:click={runImport}
              disabled={importing || !canStep3}
            >
              {#if importing}<Loader2 size={18} class="animate-spin" />{/if}
              {importing ? $i18nT('pages.import.importing') : $i18nT('pages.import.importNow')}
            </button>
          {:else}
            <!-- Success -->
            <div class="py-6 space-y-4">
              <div class="text-center">
                <div class="w-16 h-16 rounded-full {importResult.failedCount > 0 ? 'bg-amber-100' : 'bg-green-100'} flex items-center justify-center mx-auto mb-4">
                  {#if importResult.failedCount > 0}
                    <AlertTriangle size={32} class="text-amber-600" />
                  {:else}
                    <Check size={32} class="text-green-600" />
                  {/if}
                </div>
                <h3 class="text-lg font-bold mb-2">
                  {importResult.sparql ? $i18nT('pages.import.sparqlSuccess') : $i18nT('pages.import.importSuccess')}
                </h3>
                {#if importResult.failedCount > 0}
                  <p class="text-sm text-amber-600">{$i18nT('pages.import.succeededFailed', { values: { succeeded: importResult.successCount, failed: importResult.failedCount } })}</p>
                {/if}
              </div>

              <!-- Versioning outcome (replace into an existing dataset) -->
              {#if importResult.versionOutcome?.new_version}
                <div class="rounded-xl border border-emerald-200 bg-emerald-50 px-4 py-3 flex items-start gap-2">
                  <GitBranch size={16} class="text-emerald-700 shrink-0 mt-0.5" />
                  <p class="text-sm text-emerald-900">{$i18nT('pages.import.versionPublished', { values: { version: importResult.versionOutcome.new_version } })}</p>
                </div>
              {:else if importResult.versionOutcome?.draft_version}
                <div class="rounded-xl border border-amber-200 bg-amber-50 px-4 py-3 flex items-start gap-2">
                  <Info size={16} class="text-amber-700 shrink-0 mt-0.5" />
                  <p class="text-sm text-amber-900">{$i18nT('pages.import.versionDraftIdentical', { values: { version: importResult.versionOutcome.draft_version } })}</p>
                </div>
              {/if}

              <!-- Per-file results table -->
              {#if importResult.fileResults?.length > 0}
                <div class="rounded-xl border border-[var(--line-soft)] overflow-hidden">
                  <div class="max-h-56 overflow-y-auto">
                    {#each importResult.fileResults as fr}
                      <div class="flex items-center gap-2 px-3 py-2 text-sm border-b border-[var(--line-soft)] last:border-b-0
                        {fr.status === 'ok' ? 'bg-green-50/50' : 'bg-red-50/50'}">
                        {#if fr.status === 'ok'}
                          <Check size={14} class="text-green-600 shrink-0" />
                        {:else}
                          <AlertTriangle size={14} class="text-red-500 shrink-0" />
                        {/if}
                        <span class="font-medium truncate">{fr.name}</span>
                        {#if fr.status === 'ok' && fr.graphIri}
                          <span class="text-xs text-[var(--ink-400)] font-mono truncate ml-auto max-w-[250px]" title={fr.graphIri}>→ {fr.graphIri}</span>
                        {/if}
                        {#if fr.status === 'error'}
                          <span class="text-xs text-red-600 truncate ml-auto max-w-[250px]" title={fr.error}>{fr.error}</span>
                        {/if}
                      </div>
                    {/each}
                  </div>
                </div>
              {/if}

              <!-- SHACL shapes auto-detect card -->
              {#if shapesDetectResult?.shapesDetected && !shapesLinkDone && !shapesLinkDismissed}
                <div class="rounded-xl border border-yellow-300 bg-yellow-50 p-4 space-y-3">
                  <div class="flex items-center gap-2">
                    <Shield size={18} class="text-yellow-600 shrink-0" />
                    <div>
                      <h4 class="text-sm font-semibold text-yellow-900">{$i18nT('pages.import.shaclShapesDetected')}</h4>
                      <p class="text-xs text-yellow-700">{$i18nT('pages.import.shapesFoundPromptMulti', { values: { count: shapesDetectResult.totalShapeCount, graphs: shapesDetectResult.shapeGraphs.length } })}</p>
                    </div>
                  </div>
                  <ul class="space-y-1">
                    {#each shapesDetectResult.shapeGraphs as sg (sg.graphIri)}
                      <li class="flex items-center gap-2 text-xs text-yellow-800">
                        <code class="font-mono truncate min-w-0" title={sg.graphIri}>{sg.graphIri}</code>
                        <span class="shrink-0 opacity-75">{$i18nT('pages.import.shapesInGraph', { values: { count: sg.shapeCount } })}</span>
                      </li>
                    {/each}
                  </ul>
                  {#if shapesDetectResult.suggestedDatasets?.length > 0}
                    <div class="flex flex-wrap items-center gap-2">
                      <Select
                        class="flex-1 min-w-0"
                        size="sm"
                        bind:value={shapesLinkTargetId}
                        placeholder={$i18nT('pages.import.selectDatasetToLink')}
                        options={[{ value: '', label: $i18nT('pages.import.selectDatasetToLink') }, ...shapesDetectResult.suggestedDatasets.map(ds => ({ value: ds.id, label: ds.has_shapes ? `${ds.name} ${$i18nT('pages.import.alreadyHasShapes')}` : ds.name }))]}
                      />
                      <button class="btn btn-sm"
                        disabled={!shapesLinkTargetId || shapesLinking}
                        on:click={async () => {
                          shapesLinking = true;
                          shapesLinkError = '';
                          try {
                            // Link the graph where shapes were actually detected
                            // (not the first imported graph — they can differ).
                            const graphIri = shapesDetectResult.shapeGraphs[0]?.graphIri || '';
                            await updateDatasetShacl(shapesLinkTargetId, { shacl_on_write: false, shapes_graph_iri: graphIri });
                            shapesLinkDone = true;
                          } catch (e) {
                            shapesLinkError = $i18nT('pages.import.shapesLinkFailed', { values: { message: e.message || '' } });
                          } finally {
                            shapesLinking = false;
                          }
                        }}>
                        {#if shapesLinking}<Loader2 size={13} class="animate-spin" />{:else}<Shield size={13} />{/if}
                        {$i18nT('pages.import.linkShapes')}
                      </button>
                      <button class="btn btn-sm btn-ghost text-xs" on:click={() => shapesLinkDismissed = true}>{$i18nT('pages.import.dismiss')}</button>
                    </div>
                    {#if shapesLinkError}
                      <p class="text-xs text-red-600">{shapesLinkError}</p>
                    {/if}
                  {:else}
                    <p class="text-xs text-yellow-700">{$i18nT('pages.import.noDatasetsWithoutShapes')}</p>
                    <button class="btn btn-sm btn-ghost text-xs" on:click={() => shapesLinkDismissed = true}>{$i18nT('pages.import.dismiss')}</button>
                  {/if}
                </div>
              {/if}
              {#if shapesLinkDone && shapesDetectResult?.shapesDetected}
                <div class="flex items-center gap-2 p-3 rounded-xl bg-green-50 border border-green-200 text-green-800 text-sm">
                  <Check size={15} />
                  <span class="flex-1 min-w-0">{$i18nT('pages.import.shapesLinkedSuccess')}</span>
                  <Link to="/shacl/shapes" class="btn btn-sm btn-ghost shrink-0">{$i18nT('pages.import.openInShaclStudio')}</Link>
                </div>
              {/if}

              <!-- Register graphs in SPARQL services -->
              {#if selectedDatasetId && importResult.successCount > 0 && !importResult.sparql}
                {#await (async () => { if (availableServices.length === 0) { try { availableServices = (await listServices(selectedDatasetId)).filter(s => s.is_active); } catch(_) {} } return true; })() then}
                  {#if availableServices.length > 0 && !serviceGraphResult}
                    <div class="rounded-xl border border-[var(--line-soft)] p-4 space-y-3">
                      <div class="flex items-center justify-between">
                        <h4 class="text-sm font-semibold">{$i18nT('pages.import.registerInServices') || 'Register in SPARQL Services'}</h4>
                        <div class="flex gap-1">
                          <button class="btn btn-xs btn-ghost" on:click={() => { selectedServiceIds = new Set(availableServices.map(s => s.id)); selectedServiceIds = selectedServiceIds; }}>
                            {$i18nT('system.selectAll') || 'Select all'}
                          </button>
                          <button class="btn btn-xs btn-ghost" on:click={() => { selectedServiceIds = new Set(); selectedServiceIds = selectedServiceIds; }}>
                            {$i18nT('system.deselectAll') || 'Deselect all'}
                          </button>
                        </div>
                      </div>
                      <div class="space-y-1">
                        {#each availableServices as svc}
                          <label class="flex items-center gap-2 text-sm cursor-pointer px-2 py-1 rounded hover:bg-[var(--bg-soft)]">
                            <input type="checkbox" checked={selectedServiceIds.has(svc.id)}
                              on:change={() => {
                                if (selectedServiceIds.has(svc.id)) selectedServiceIds.delete(svc.id);
                                else selectedServiceIds.add(svc.id);
                                selectedServiceIds = selectedServiceIds;
                              }} />
                            <span class="font-medium">{svc.name}</span>
                            <code class="text-xs text-[var(--ink-400)]">{svc.slug}</code>
                          </label>
                        {/each}
                      </div>
                      <button class="btn btn-sm" disabled={selectedServiceIds.size === 0 || registeringServiceGraphs}
                        on:click={async () => {
                          registeringServiceGraphs = true;
                          let registered = 0, failed = 0;
                          const graphIris = (importResult.fileResults || []).filter(r => r.status === 'ok' && r.graphIri).map(r => r.graphIri);
                          for (const svcId of selectedServiceIds) {
                            for (const iri of graphIris) {
                              try { await addServiceGraph(selectedDatasetId, svcId, { graph_iri: iri }); registered++; } catch(_) { failed++; }
                            }
                          }
                          serviceGraphResult = { success: failed === 0, registered, failed };
                          registeringServiceGraphs = false;
                        }}>
                        {#if registeringServiceGraphs}<Loader2 size={14} class="animate-spin" />{/if}
                        {$i18nT('pages.import.registerGraphs') || 'Register graphs in selected services'}
                      </button>
                    </div>
                  {/if}
                  {#if serviceGraphResult}
                    <div class="flex items-center gap-2 p-3 rounded-xl text-sm
                      {serviceGraphResult.success ? 'bg-green-50 border border-green-200 text-green-800' : 'bg-amber-50 border border-amber-200 text-amber-800'}">
                      <Check size={15} />
                      {$i18nT('pages.import.graphServiceRegistered', { values: { count: serviceGraphResult.registered } })}{serviceGraphResult.failed > 0 ? $i18nT('pages.import.graphServiceFailedSuffix', { values: { count: serviceGraphResult.failed } }) : ''}
                    </div>
                  {/if}
                {/await}
              {/if}

              {#if importError}
                <div class="error">{importError}</div>
              {/if}
              <!-- SPARQL Update result: show what was inserted/deleted -->
              {#if importResult.sparql && importResult.preview}
                {@const prev = importResult.preview}
                {#if prev.isPatternBased}
                  <div class="flex items-center gap-2 p-3 rounded-xl bg-amber-50 border border-amber-200 text-amber-800 text-sm">
                    <Check size={15} /> {$i18nT('pages.import.whereUpdateExecuted')}
                  </div>
                {:else}
                  {#if prev.inserts.length > 0}
                    <div class="rounded-xl border border-emerald-200 bg-emerald-50 overflow-hidden">
                      <div class="flex items-center gap-1.5 px-3 py-2 text-xs font-semibold text-emerald-800 bg-emerald-100 border-b border-emerald-200">
                        <Check size={13} /> {$i18nT('pages.import.insertedTriples', { values: { count: prev.inserts.length } })}
                      </div>
                      <div class="overflow-x-auto">
                        <table class="w-full text-[0.68rem] font-mono border-collapse">
                          <thead><tr class="text-left text-emerald-700">
                            <th class="px-2 py-1 border-b border-emerald-200">{$i18nT('pages.import.subject')}</th>
                            <th class="px-2 py-1 border-b border-emerald-200">{$i18nT('pages.import.predicate')}</th>
                            <th class="px-2 py-1 border-b border-emerald-200">{$i18nT('pages.import.object')}</th>
                          </tr></thead>
                          <tbody>
                            {#each prev.inserts as row}
                              <tr class="border-b border-emerald-100"><td class="px-2 py-1 max-w-[220px] truncate" title={row.s?.value}>{row.s?.value ?? ''}</td><td class="px-2 py-1 max-w-[220px] truncate" title={row.p?.value}>{row.p?.value ?? ''}</td><td class="px-2 py-1 max-w-[220px] truncate" title={row.o?.value}>{row.o?.value ?? ''}</td></tr>
                            {/each}
                          </tbody>
                        </table>
                      </div>
                    </div>
                  {/if}
                  {#if prev.deletes.length > 0}
                    <div class="rounded-xl border border-red-200 bg-red-50 overflow-hidden">
                      <div class="flex items-center gap-1.5 px-3 py-2 text-xs font-semibold text-red-800 bg-red-100 border-b border-red-200">
                        <X size={13} /> {$i18nT('pages.import.deletedTriples', { values: { count: prev.deletes.length } })}
                      </div>
                      <div class="overflow-x-auto">
                        <table class="w-full text-[0.68rem] font-mono border-collapse">
                          <thead><tr class="text-left text-red-700">
                            <th class="px-2 py-1 border-b border-red-200">{$i18nT('pages.import.subject')}</th>
                            <th class="px-2 py-1 border-b border-red-200">{$i18nT('pages.import.predicate')}</th>
                            <th class="px-2 py-1 border-b border-red-200">{$i18nT('pages.import.object')}</th>
                          </tr></thead>
                          <tbody>
                            {#each prev.deletes as row}
                              <tr class="border-b border-red-100"><td class="px-2 py-1 max-w-[220px] truncate" title={row.s?.value}>{row.s?.value ?? ''}</td><td class="px-2 py-1 max-w-[220px] truncate" title={row.p?.value}>{row.p?.value ?? ''}</td><td class="px-2 py-1 max-w-[220px] truncate" title={row.o?.value}>{row.o?.value ?? ''}</td></tr>
                            {/each}
                          </tbody>
                        </table>
                      </div>
                    </div>
                  {/if}
                {/if}
              {/if}
              {#if importResult.graphIri}
                <p class="text-sm text-[var(--ink-500)] mb-4">
                  {$i18nT('pages.import.fileUploaded', { values: { graph: importResult.graphIri } })}
                </p>
                <div class="flex gap-2 justify-center flex-wrap">
                  {#if importResult.datasetId}
                    <Link to="/datasets/{importResult.datasetId}" class="btn btn-sm">
                      <Database size={14} />
                      {$i18nT('pages.import.viewDataset')}
                    </Link>
                  {/if}
                  <a href="/browse?graph={encodeURIComponent(importResult.graphIri)}" class="btn btn-sm btn-ghost">
                    <BarChart3 size={14} />
                    {$i18nT('pages.import.browseGraph')}
                  </a>
                  <a href="/browse?view=graph&subject={encodeURIComponent(importResult.graphIri)}" class="btn btn-sm btn-ghost">
                    <ExternalLink size={14} />
                    {$i18nT('pages.import.visualize')}
                  </a>
                </div>
              {/if}
            </div>
          {/if}

          <div class="flex justify-between pt-2">
            {#if importResult}
              <button class="btn btn-ghost" on:click={resetWizard}>
                <RefreshCw size={14} /> {$i18nT('pages.import.importAnother')}
              </button>
            {:else}
              <button class="btn btn-ghost" on:click={() => step = 2}>
                <ChevronLeft size={16} /> {$i18nT('system.back')}
              </button>
            {/if}
          </div>
        </div>
      {/if}
    </div>
  {/if}
</div>

<style>
  .di-toggle { position: relative; display: inline-flex; align-items: center; gap: 0.5rem; cursor: pointer; user-select: none; font-size: 0.875rem; color: var(--ink-700); }
  .di-toggle input { position: absolute; opacity: 0; width: 0; height: 0; }
  .di-track { width: 36px; height: 20px; background: #ccc; border-radius: 10px; transition: background 0.2s; position: relative; flex-shrink: 0; }
  .di-toggle input:checked + .di-track { background: var(--brand-600, #4a90d9); }
  .di-thumb { position: absolute; top: 3px; left: 3px; width: 14px; height: 14px; background: #fff; border-radius: 50%; transition: transform 0.2s; box-shadow: 0 1px 3px rgba(0,0,0,0.2); }
  .di-toggle input:checked + .di-track .di-thumb { transform: translateX(16px); }

  :global(:is([data-theme="dark"], .dark)) .di-track { background: var(--line-strong); }

  /* ─── Dark-mode overrides ────────────────────────────────────────────────
     This wizard is built from hardcoded Tailwind light utilities (bg-white,
     bg-*-50/100 tints, dark text, light borders) that don't adapt to the dark
     theme. Remap them to dark surfaces / desaturated tints / light ink, scoped
     to .di-page so we don't disturb the same utilities elsewhere. Selectors
     live inside :global() because the targets are global utility classes. */
  /* White surfaces → dark panels */
  :global(:is([data-theme="dark"], .dark) .di-page .bg-white),
  :global(:is([data-theme="dark"], .dark) .di-page .bg-white\/90),
  :global(:is([data-theme="dark"], .dark) .di-page .bg-white\/80),
  :global(:is([data-theme="dark"], .dark) .di-page .bg-white\/70) { background-color: var(--bg-strong) !important; }
  :global(:is([data-theme="dark"], .dark) .di-page .bg-white\/60),
  :global(:is([data-theme="dark"], .dark) .di-page .bg-white\/50),
  :global(:is([data-theme="dark"], .dark) .di-page .bg-white\/40),
  :global(:is([data-theme="dark"], .dark) .di-page .bg-white\/30),
  :global(:is([data-theme="dark"], .dark) .di-page .bg-slate-50),
  :global(:is([data-theme="dark"], .dark) .di-page .bg-slate-100),
  :global(:is([data-theme="dark"], .dark) .di-page .bg-slate-50\/50),
  :global(:is([data-theme="dark"], .dark) .di-page .bg-slate-50\/60) { background-color: rgba(255,255,255,0.04) !important; }

  /* Semantic tint backgrounds → low-alpha dark tints */
  :global(:is([data-theme="dark"], .dark) .di-page .bg-emerald-50),
  :global(:is([data-theme="dark"], .dark) .di-page .bg-emerald-100),
  :global(:is([data-theme="dark"], .dark) .di-page .bg-green-50),
  :global(:is([data-theme="dark"], .dark) .di-page .bg-green-100) { background-color: rgba(16,185,129,0.16) !important; }
  :global(:is([data-theme="dark"], .dark) .di-page .bg-amber-50),
  :global(:is([data-theme="dark"], .dark) .di-page .bg-amber-100),
  :global(:is([data-theme="dark"], .dark) .di-page .bg-amber-200),
  :global(:is([data-theme="dark"], .dark) .di-page .bg-amber-100\/90) { background-color: rgba(245,158,11,0.16) !important; }
  :global(:is([data-theme="dark"], .dark) .di-page .bg-red-50),
  :global(:is([data-theme="dark"], .dark) .di-page .bg-red-100) { background-color: rgba(239,68,68,0.16) !important; }
  :global(:is([data-theme="dark"], .dark) .di-page .bg-blue-100) { background-color: rgba(59,130,246,0.18) !important; }
  :global(:is([data-theme="dark"], .dark) .di-page .bg-purple-50),
  :global(:is([data-theme="dark"], .dark) .di-page .bg-purple-100) { background-color: rgba(124,58,237,0.18) !important; }
  :global(:is([data-theme="dark"], .dark) .di-page .bg-orange-100) { background-color: rgba(245,158,11,0.18) !important; }

  /* Text colours → lighten for dark surfaces */
  :global(:is([data-theme="dark"], .dark) .di-page .text-emerald-600),
  :global(:is([data-theme="dark"], .dark) .di-page .text-emerald-700),
  :global(:is([data-theme="dark"], .dark) .di-page .text-emerald-800),
  :global(:is([data-theme="dark"], .dark) .di-page .text-emerald-900),
  :global(:is([data-theme="dark"], .dark) .di-page .text-green-600),
  :global(:is([data-theme="dark"], .dark) .di-page .text-green-700),
  :global(:is([data-theme="dark"], .dark) .di-page .text-green-800) { color: #6ee7b7 !important; }
  :global(:is([data-theme="dark"], .dark) .di-page .text-amber-600),
  :global(:is([data-theme="dark"], .dark) .di-page .text-amber-700),
  :global(:is([data-theme="dark"], .dark) .di-page .text-amber-800),
  :global(:is([data-theme="dark"], .dark) .di-page .text-amber-900) { color: #fcd34d !important; }
  :global(:is([data-theme="dark"], .dark) .di-page .text-red-500),
  :global(:is([data-theme="dark"], .dark) .di-page .text-red-600),
  :global(:is([data-theme="dark"], .dark) .di-page .text-red-700),
  :global(:is([data-theme="dark"], .dark) .di-page .text-red-800) { color: #fca5a5 !important; }
  :global(:is([data-theme="dark"], .dark) .di-page .text-blue-700),
  :global(:is([data-theme="dark"], .dark) .di-page .text-blue-800) { color: #93c5fd !important; }
  :global(:is([data-theme="dark"], .dark) .di-page .text-purple-600),
  :global(:is([data-theme="dark"], .dark) .di-page .text-purple-700),
  :global(:is([data-theme="dark"], .dark) .di-page .text-purple-800),
  :global(:is([data-theme="dark"], .dark) .di-page .text-purple-900) { color: #c4b5fd !important; }
  :global(:is([data-theme="dark"], .dark) .di-page .text-slate-600),
  :global(:is([data-theme="dark"], .dark) .di-page .text-slate-700) { color: var(--ink-600) !important; }

  /* Borders → low-alpha tinted strokes */
  :global(:is([data-theme="dark"], .dark) .di-page .border-emerald-100),
  :global(:is([data-theme="dark"], .dark) .di-page .border-emerald-200),
  :global(:is([data-theme="dark"], .dark) .di-page .border-green-200) { border-color: rgba(16,185,129,0.32) !important; }
  :global(:is([data-theme="dark"], .dark) .di-page .border-amber-100),
  :global(:is([data-theme="dark"], .dark) .di-page .border-amber-200),
  :global(:is([data-theme="dark"], .dark) .di-page .border-amber-300) { border-color: rgba(245,158,11,0.32) !important; }
  :global(:is([data-theme="dark"], .dark) .di-page .border-red-100),
  :global(:is([data-theme="dark"], .dark) .di-page .border-red-200),
  :global(:is([data-theme="dark"], .dark) .di-page .border-red-400) { border-color: rgba(239,68,68,0.34) !important; }
  :global(:is([data-theme="dark"], .dark) .di-page .border-purple-200) { border-color: rgba(124,58,237,0.34) !important; }
</style>
