<script>
  import { onMount, onDestroy, createEventDispatcher } from 'svelte';
  import { EditorView, keymap, lineNumbers, highlightActiveLine, drawSelection, hoverTooltip } from '@codemirror/view';
  import { EditorState, Compartment } from '@codemirror/state';
  import { defaultKeymap, history, historyKeymap, indentWithTab } from '@codemirror/commands';
  import { closeBrackets, closeBracketsKeymap, completionKeymap } from '@codemirror/autocomplete';
  import { indentOnInput, bracketMatching, foldKeymap } from '@codemirror/language';
  import { searchKeymap, highlightSelectionMatches } from '@codemirror/search';
  import { lintKeymap } from '@codemirror/lint';
  import { sparqlLanguage, sparqlAutocomplete } from '../lib/sparql-mode.js';
  import { turtleLanguage } from '../lib/turtle-mode.js';
  import { ontologyAwareAutocomplete } from '../lib/ontology/sparqlCompletion.js';
  import { sparqlLinter } from '../lib/ontology/sparqlLint.js';
  import { shortenIRI } from '../lib/rdf-utils.js';
  import { NAMESPACES } from '../lib/ontology/vocabularies.js';
  import { extractDeclaredPrefixes, lookupPrefixSync } from '../lib/ontology/prefixService.js';
  import { buildEditorTheme, resolveDark, onThemeChange } from '../lib/ontology/editorTheme.js';
  import { formatSparql } from '../lib/ontology/sparqlFormat.js';
  import { Sparkles } from 'lucide-svelte';
  import { t as i18nT } from 'svelte-i18n';

  export let query = '';
  export let mode = 'sparql';
  export let readonly = false;
  export let height = '280px';
  /** Optional prefix map (e.g. from the loaded ontology). */
  export let ontologyPrefixes = null;
  /** Optional ontology vocabulary: Array<{iri,label?,comment?,kind?}>. */
  export let ontologyTerms = null;
  /** Enable sparqljs-based linting. */
  export let lint = false;
  /** Optional async fetcher: (sparqlQueryString) => Promise<{head,results}>. Enables live IRI hover info. */
  export let sparqlFetcher = null;
  /** Optional graph IRI(s) to scope hover lookup queries to. */
  export let graphIris = null;
  /** Theme preference: 'auto' (follow app/OS) | 'light' | 'dark'. */
  /** @type {import('../lib/ontology/editorTheme.ts').ThemePref} */
  export let theme = 'auto';
  /** Show the floating in-editor "Format" button (SPARQL mode, editable only). */
  export let showFormat = true;

  const dispatch = createEventDispatcher();

  let container;
  let view;
  const completionCompartment = new Compartment();
  const lintCompartment = new Compartment();
  const themeCompartment = new Compartment();

  let isDark = resolveDark(theme);
  let unsubTheme = null;
  function recomputeDark() {
    const next = resolveDark(theme);
    if (next !== isDark) isDark = next;
  }
  // Re-resolve when the `theme` prop changes; reconfigure when the resolved
  // mode (or height) changes.
  $: { theme; recomputeDark(); }
  $: if (view) view.dispatch({ effects: themeCompartment.reconfigure(buildEditorTheme(isDark, height)) });

  $: completionExt = mode === 'sparql'
    ? ontologyAwareAutocomplete({
        prefixes: ontologyPrefixes || {},
        terms: ontologyTerms || [],
      })
    : sparqlAutocomplete;

  $: lintExt = (mode === 'sparql' && lint)
    ? sparqlLinter({
        knownIris: new Set((ontologyTerms || []).map(t => t.iri)),
        resolvePrefix: (p) => (ontologyPrefixes && ontologyPrefixes[p]) || NAMESPACES[p] || lookupPrefixSync(p) || null,
      })
    : [];

  // Reconfigure compartments when the ontology changes
  $: if (view) {
    view.dispatch({ effects: completionCompartment.reconfigure(completionExt) });
  }
  $: if (view) {
    view.dispatch({ effects: lintCompartment.reconfigure(lintExt) });
  }

  // Cache live fetches across hovers to avoid re-querying
  const liveInfoCache = new Map(); // iri -> Promise<{label?,comment?,types?,domain?,range?}>

  function resolveIriAt(view, pos) {
    const text = view.state.doc.toString();
    const iriMatch = findEnclosing(text, pos, /<[^>\s]+>/g);
    if (iriMatch) {
      return { iri: iriMatch.text.slice(1, -1), from: iriMatch.from, to: iriMatch.to };
    }
    const pnMatch = findEnclosing(text, pos, /[a-zA-Z_][\w-]*:[a-zA-Z_][\w-]*/g);
    if (pnMatch) {
      const [prefix, local] = pnMatch.text.split(':');
      const declared = extractDeclaredPrefixes(text);
      const ns = declared[prefix]
        || (ontologyPrefixes && ontologyPrefixes[prefix])
        || NAMESPACES[prefix]
        || lookupPrefixSync(prefix);
      if (ns) return { iri: ns + local, from: pnMatch.from, to: pnMatch.to, prefix, local };
    }
    return null;
  }

  function fetchIriInfo(iri) {
    if (liveInfoCache.has(iri)) return liveInfoCache.get(iri);
    if (!sparqlFetcher) return Promise.resolve(null);
    const froms = Array.isArray(graphIris) && graphIris.length
      ? graphIris.map(g => `FROM <${g}>`).join('\n')
      : '';
    const q = `PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
PREFIX rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#>
SELECT ?label ?comment ?type ?domain ?range
${froms}
WHERE {
  { <${iri}> rdfs:label ?label } UNION
  { <${iri}> rdfs:comment ?comment } UNION
  { <${iri}> a ?type } UNION
  { <${iri}> rdfs:domain ?domain } UNION
  { <${iri}> rdfs:range ?range }
} LIMIT 50`;
    const p = (async () => {
      try {
        const res = await sparqlFetcher(q);
        const bindings = res?.results?.bindings || [];
        const info = { types: [], domains: [], ranges: [], labels: [], comments: [] };
        for (const b of bindings) {
          if (b.label) info.labels.push(b.label);
          if (b.comment) info.comments.push(b.comment);
          if (b.type) info.types.push(b.type.value);
          if (b.domain) info.domains.push(b.domain.value);
          if (b.range) info.ranges.push(b.range.value);
        }
        info.label = pickLang(info.labels)?.value;
        info.comment = pickLang(info.comments)?.value;
        return info;
      } catch { return null; }
    })();
    liveInfoCache.set(iri, p);
    return p;
  }

  function pickLang(arr) {
    if (!arr.length) return null;
    return arr.find(x => (x['xml:lang'] || x.lang) === 'en')
        || arr.find(x => !(x['xml:lang'] || x.lang))
        || arr[0];
  }

  function iriHoverTooltip(terms) {
    const byIri = new Map(terms.map(t => [t.iri, t]));
    return hoverTooltip(async (view, pos) => {
      const hit = resolveIriAt(view, pos);
      if (!hit) return null;
      const localTerm = byIri.get(hit.iri);
      return {
        pos: hit.from,
        end: hit.to,
        above: true,
        create() {
          const dom = document.createElement('div');
          dom.className = 'cm-ontology-tt';
          dom.innerHTML = renderTooltip(hit.iri, localTerm, null, true);
          fetchIriInfo(hit.iri).then(info => {
            dom.innerHTML = renderTooltip(hit.iri, localTerm, info, false);
          });
          return { dom };
        },
      };
    });
  }

  function renderTooltip(iri, localTerm, liveInfo, loading) {
    const label = liveInfo?.label || localTerm?.label || shortenIRI(iri);
    const comment = liveInfo?.comment || localTerm?.comment;
    const kind = localTerm?.kind;
    const types = liveInfo?.types?.slice(0, 4) || [];
    const domains = liveInfo?.domains?.slice(0, 3) || [];
    const ranges = liveInfo?.ranges?.slice(0, 3) || [];
    return `
      <div class="tt-label">${escapeHtml(label)}</div>
      <div class="tt-iri">${escapeHtml(iri)}</div>
      ${kind ? `<div class="tt-kind">${escapeHtml(kind)}</div>` : ''}
      ${types.length ? `<div class="tt-row"><span class="tt-k">a</span> ${types.map(t => `<span class="tt-chip">${escapeHtml(shortenIRI(t))}</span>`).join(' ')}</div>` : ''}
      ${domains.length ? `<div class="tt-row"><span class="tt-k">domain</span> ${domains.map(t => `<span class="tt-chip">${escapeHtml(shortenIRI(t))}</span>`).join(' ')}</div>` : ''}
      ${ranges.length ? `<div class="tt-row"><span class="tt-k">range</span> ${ranges.map(t => `<span class="tt-chip">${escapeHtml(shortenIRI(t))}</span>`).join(' ')}</div>` : ''}
      ${comment ? `<div class="tt-comment">${escapeHtml(String(comment).slice(0, 500))}</div>` : ''}
      ${loading && sparqlFetcher ? `<div class="tt-loading">${escapeHtml($i18nT('system.loading'))}</div>` : ''}
    `;
  }

  function findEnclosing(text, pos, re) {
    re.lastIndex = 0;
    let m;
    while ((m = re.exec(text))) {
      if (pos >= m.index && pos <= m.index + m[0].length) {
        return { text: m[0], from: m.index, to: m.index + m[0].length };
      }
      if (m.index > pos) break;
    }
    return null;
  }

  function escapeHtml(s) {
    return String(s || '').replace(/[&<>"']/g, c => ({
      '&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;',
    }[c]));
  }

  onMount(() => {
    const language = mode === 'turtle'
      ? [turtleLanguage]
      : [sparqlLanguage];

    const executeKeymap = keymap.of([
      { key: 'Ctrl-Enter', mac: 'Cmd-Enter', run() { dispatch('execute', view.state.doc.toString()); return true; } },
    ]);

    const extensions = [
      lineNumbers(),
      highlightActiveLine(),
      history(),
      drawSelection(),
      indentOnInput(),
      bracketMatching(),
      closeBrackets(),
      highlightSelectionMatches(),
      keymap.of([
        ...closeBracketsKeymap,
        ...defaultKeymap,
        ...historyKeymap,
        ...foldKeymap,
        ...completionKeymap,
        ...searchKeymap,
        ...lintKeymap,
        indentWithTab,
      ]),
      executeKeymap,
      ...language,
      completionCompartment.of(completionExt),
      lintCompartment.of(lintExt),
      ...(mode === 'sparql' ? [iriHoverTooltip(ontologyTerms || [])] : []),
      EditorView.updateListener.of((update) => {
        if (update.docChanged) {
          const newVal = update.state.doc.toString();
          if (newVal !== query) {
            query = newVal;
            dispatch('change', newVal);
          }
        }
      }),
      themeCompartment.of(buildEditorTheme(isDark, height)),
    ];

    if (readonly) extensions.push(EditorState.readOnly.of(true));

    view = new EditorView({
      state: EditorState.create({ doc: query, extensions }),
      parent: container,
    });
    unsubTheme = onThemeChange(recomputeDark);
  });

  onDestroy(() => { if (unsubTheme) unsubTheme(); if (view) view.destroy(); });

  $: if (view && query !== view.state.doc.toString()) {
    view.dispatch({
      changes: { from: 0, to: view.state.doc.length, insert: query },
    });
  }

  /** Insert text at the current cursor, replacing any selection. */
  export function insertAtCursor(text) {
    if (!view) return;
    const insert = text.endsWith('\n') ? text : text + '\n';
    const sel = view.state.selection.main;
    view.dispatch({
      changes: { from: sel.from, to: sel.to, insert },
      selection: { anchor: sel.from + insert.length },
      scrollIntoView: true,
    });
    view.focus();
  }

  /** Select and scroll to the first occurrence of `substr`. Returns true if found. */
  export function scrollToText(substr) {
    if (!view || !substr) return false;
    const idx = view.state.doc.toString().indexOf(substr);
    if (idx < 0) return false;
    view.dispatch({
      selection: { anchor: idx, head: idx + substr.length },
      scrollIntoView: true,
    });
    view.focus();
    return true;
  }

  /** Pretty-print the current SPARQL document in place (idempotent, never throws). */
  export function formatDoc() {
    if (!view) return;
    const cur = view.state.doc.toString();
    const next = formatSparql(cur);
    if (next !== cur) {
      view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: next } });
    }
    view.focus();
  }
</script>

<div class="cm-host">
  {#if showFormat && mode === 'sparql' && !readonly}
    <button type="button" class="btn btn-sm btn-ghost cm-format-btn" on:click={formatDoc} title={$i18nT('components.sparqlEditorCM.formatQueryTooltip')}>
      <Sparkles size={14} /> {$i18nT('components.sparqlEditorCM.formatButton')}
    </button>
  {/if}
  <div bind:this={container} class="cm-wrapper"></div>
</div>

<style>
  .cm-host { position: relative; width: 100%; box-sizing: border-box; }
  .cm-wrapper { width: 100%; box-sizing: border-box; }
  .cm-format-btn {
    position: absolute;
    top: 6px;
    right: 8px;
    z-index: 5;
    opacity: 0.9;
    background: var(--bg-elevated, rgba(255, 255, 255, 0.9));
    backdrop-filter: blur(4px);
  }
  .cm-format-btn:hover { opacity: 1; }
</style>
