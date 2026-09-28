<script>
  // Shared, presentational data-table used by BOTH the SPARQL editor results
  // (mode="bindings") and the Triple Browser (mode="triples"). It only renders
  // the rows it is handed — PAGINATION, fetching and filtering stay with the
  // parent. The markup/CSS are lifted from TripleBrowser.svelte and
  // SparqlEditor.svelte so the two consumers stay pixel-consistent.
  import { createEventDispatcher } from 'svelte';
  import { shortenIRI, toNTriples } from '../lib/rdf-utils.js';
  import RdfTerm from './RdfTerm.svelte';
  import TermPopover from './ontology/TermPopover.svelte';
  import { Check, Copy } from 'lucide-svelte';
  import { t } from 'svelte-i18n';
  import { copyOrWarn } from '../lib/clipboard.js';

  /**
   * @typedef {Object} RdfTermLike
   * @property {string} [type]
   * @property {any} [value]
   * @property {string} [language]
   * @property {string} [datatype]
   */
  /**
   * @typedef {Object} TripleLike
   * @property {RdfTermLike} subject
   * @property {RdfTermLike} predicate
   * @property {RdfTermLike} object
   * @property {RdfTermLike} [graph]
   */

  /** Which dataset shape to render. */
  /** @type {'bindings' | 'triples'} */
  export let mode = 'bindings';

  // ── bindings mode (SPARQL SELECT) ──────────────────────────────────────────
  /** Projected variable names, rendered as `?var` headers. @type {string[]} */
  export let vars = [];
  /** One object per result row, keyed by variable name. @type {Array<Record<string, RdfTermLike>>} */
  export let bindings = [];

  // ── triples mode (Triple Browser) ──────────────────────────────────────────
  /** @type {TripleLike[]} */
  export let triples = [];

  // ── shared ──────────────────────────────────────────────────────────────────
  /** Show a subtle loading veil over the table body. */
  export let loading = false;
  /** Message shown when there are no rows (and we're not loading). Falls back to
   * a localized default when the parent doesn't supply one. */
  export let emptyText = '';
  /** Max height of the scroll viewport; the header stays sticky above it. */
  export let maxHeight = '65vh';

  const dispatch = createEventDispatcher();

  // Hash-based predicate namespace → hue, replicated from TripleBrowser.svelte
  // (lines ~25-41) so predicate badges colour identically to the graph view:
  // a stable hue per namespace, with a light tint for the badge background.
  function strHue(str) {
    let h = 0;
    for (let i = 0; i < str.length; i++) h = (h * 31 + str.charCodeAt(i)) & 0xffffffff;
    return Math.abs(h) % 360;
  }
  function nsOf(iri) {
    const hash = iri.lastIndexOf('#');
    const slash = iri.lastIndexOf('/');
    return iri.slice(0, Math.max(hash, slash) + 1) || iri;
  }
  function predicateColor(iri) {
    if (!iri) return '#6d28d9';
    return `hsl(${strHue(nsOf(iri))},52%,38%)`;
  }
  function predicateBg(iri) {
    if (!iri) return '#ede9fe';
    return `hsl(${strHue(nsOf(iri))},65%,95%)`;
  }

  // Copy one row as a single N-Triples line. We reuse the shared serializer
  // (toNTriples) from rdf-utils instead of replicating TripleBrowser's local
  // tripleLine() — same output, escaping, and xsd:string-datatype handling.
  function copyTriple(tr) {
    const line = toNTriples([tr]);
    dispatch('copy', { triple: tr, text: line });
    void copyOrWarn(line);
  }

  // Per-term copying. Every cell shows the PREFIXED form (rdf:type, not the IRI),
  // so selecting the text by hand never yields what someone wants to paste; and
  // the predicate and graph cells are chips rather than RdfTerms, so before this
  // they carried no copy control at all. One identical button therefore lives
  // here, in all four columns, and RdfTerm's own is suppressed in this table so a
  // cell never grows two of them.
  function copyTextOf(term) {
    if (!term || typeof term.value !== 'string') return '';
    // A blank node only means anything in its N-Triples form.
    return term.type === 'bnode' ? `_:${term.value}` : term.value;
  }
  const isIri = (term) => term?.type === 'uri' || term?.type === 'iri';
  const copyLabelOf = (term) =>
    isIri(term) ? $t('components.dataTable.copyIri') : $t('components.rdfTerm.copyValue');

  // Which cell last confirmed a copy, as `row:column`, so the tick replaces the
  // icon in that one button only.
  let copiedCell = '';
  let copiedTimer;
  async function copyTerm(cell, term) {
    const text = copyTextOf(term);
    if (!text) return;
    if (await copyOrWarn(text)) {
      copiedCell = cell;
      clearTimeout(copiedTimer);
      copiedTimer = setTimeout(() => (copiedCell = ''), 1500);
    }
  }

  $: isEmpty = mode === 'triples'
    ? (!triples || triples.length === 0)
    : (!bindings || bindings.length === 0);

  // Staggered row entrance: replay whenever the data array identity changes (a
  // page change, new query, or filter hands us a fresh array). Rows are keyed by
  // a generation counter so Svelte recreates them — which re-triggers the CSS
  // entrance — but only for reasonably-sized pages. Very large result sets render
  // with stable index keys (no animation) to avoid a costly full remount.
  let _gen = 0;
  let _ref = null;
  $: {
    const cur = mode === 'triples' ? triples : bindings;
    if (cur !== _ref) { _ref = cur; _gen += 1; }
  }
  $: rowCount = mode === 'triples' ? (triples?.length || 0) : (bindings?.length || 0);
  $: animateRows = rowCount > 0 && rowCount <= 150;
  const rowKey = (i) => (animateRows ? _gen * 100003 + i : i);
  const rowDelay = (i) => (animateRows ? `animation-delay:${Math.min(i * 16, 200)}ms` : '');
</script>

<div class="table-scroll" style="max-height: {maxHeight}" class:is-loading={loading}>
  {#if mode === 'triples'}
    <table>
      <colgroup>
        <col style="width: 28%" />
        <col style="width: 18%" />
        <col style="width: 28%" />
        <col style="width: 16%" />
        <col style="width: 34px" />
      </colgroup>
      <thead>
        <tr>
          <th>{$t('components.dataTable.subject')}</th>
          <th>{$t('components.dataTable.predicate')}</th>
          <th>{$t('components.dataTable.object')}</th>
          <th>{$t('components.dataTable.graph')}</th>
          <th class="actions-col-header"></th>
        </tr>
      </thead>
      <tbody>
        {#each triples as tr, i (rowKey(i))}
          <tr class="triple-row" class:row-in={animateRows} style={rowDelay(i)}>
            <td class="term-cell">
              <div class="cell">
                <span class="cell-body">
                  <RdfTerm term={tr.subject} graph={tr.graph?.value || ''} copyable={false} />
                </span>
                {#if copyTextOf(tr.subject)}
                  <button
                    class="copy-iri"
                    class:copied={copiedCell === `${i}:s`}
                    title={copiedCell === `${i}:s` ? $t('system.copied') : copyLabelOf(tr.subject)}
                    aria-label={copyLabelOf(tr.subject)}
                    on:click|stopPropagation={() => copyTerm(`${i}:s`, tr.subject)}
                  >{#if copiedCell === `${i}:s`}<Check size={12} />{:else}<Copy size={12} />{/if}</button>
                {/if}
              </div>
            </td>
            <td class="pred-cell">
              <div class="cell">
                <span class="cell-body">
                  <TermPopover iri={tr.predicate?.value || ''} variant="rich">
                    <span
                      class="predicate"
                      title={tr.predicate?.value}
                      style="color:{predicateColor(tr.predicate?.value)}; background:{predicateBg(tr.predicate?.value)}"
                    >{shortenIRI(tr.predicate?.value || '')}</span>
                  </TermPopover>
                </span>
                {#if copyTextOf(tr.predicate)}
                  <button
                    class="copy-iri"
                    class:copied={copiedCell === `${i}:p`}
                    title={copiedCell === `${i}:p` ? $t('system.copied') : copyLabelOf(tr.predicate)}
                    aria-label={copyLabelOf(tr.predicate)}
                    on:click|stopPropagation={() => copyTerm(`${i}:p`, tr.predicate)}
                  >{#if copiedCell === `${i}:p`}<Check size={12} />{:else}<Copy size={12} />{/if}</button>
                {/if}
              </div>
            </td>
            <td class="term-cell">
              <div class="cell">
                <span class="cell-body">
                  {#if tr.object?.type === 'uri' || tr.object?.type === 'iri'}
                    <TermPopover iri={tr.object.value} variant="compact" trigger="hover">
                      <RdfTerm term={tr.object} graph={tr.graph?.value || ''} copyable={false} />
                    </TermPopover>
                  {:else}
                    <RdfTerm term={tr.object} graph={tr.graph?.value || ''} copyable={false} />
                  {/if}
                </span>
                {#if copyTextOf(tr.object)}
                  <button
                    class="copy-iri"
                    class:copied={copiedCell === `${i}:o`}
                    title={copiedCell === `${i}:o` ? $t('system.copied') : copyLabelOf(tr.object)}
                    aria-label={copyLabelOf(tr.object)}
                    on:click|stopPropagation={() => copyTerm(`${i}:o`, tr.object)}
                  >{#if copiedCell === `${i}:o`}<Check size={12} />{:else}<Copy size={12} />{/if}</button>
                {/if}
              </div>
            </td>
            <td class="graph-cell">
              <div class="cell">
                <span class="cell-body">
                  {#if tr.graph?.value}
                    <span class="graph-tag" title={tr.graph.value}>{shortenIRI(tr.graph.value)}</span>
                  {:else}
                    <span class="graph-default">{$t('components.dataTable.defaultGraph')}</span>
                  {/if}
                </span>
                {#if copyTextOf(tr.graph)}
                  <button
                    class="copy-iri"
                    class:copied={copiedCell === `${i}:g`}
                    title={copiedCell === `${i}:g` ? $t('system.copied') : copyLabelOf(tr.graph)}
                    aria-label={copyLabelOf(tr.graph)}
                    on:click|stopPropagation={() => copyTerm(`${i}:g`, tr.graph)}
                  >{#if copiedCell === `${i}:g`}<Check size={12} />{:else}<Copy size={12} />{/if}</button>
                {/if}
              </div>
            </td>
            <td class="actions-col">
              <button class="row-action" title={$t('components.dataTable.copyNTriple')} on:click={() => copyTriple(tr)}>
                <Copy size={13} />
              </button>
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
  {:else}
    <table>
      <thead>
        <tr>
          {#each vars as v}
            <th>?{v}</th>
          {/each}
        </tr>
      </thead>
      <tbody>
        {#each bindings as row, i (rowKey(i))}
          <tr class="triple-row" class:row-in={animateRows} style={rowDelay(i)}>
            {#each vars as v}
              <td class="term-cell">
                <RdfTerm term={row[v] || null} />
              </td>
            {/each}
          </tr>
        {/each}
      </tbody>
    </table>
  {/if}

  {#if isEmpty && !loading}
    <p class="empty-state">{emptyText || $t('components.dataTable.noResults')}</p>
  {/if}
</div>

<style>
  /* ─── Table ──────────────────────────────────────────────────────────────── */
  /* Lifted from TripleBrowser.svelte (~1937-1972) — the most complete of the two
     table looks — so both the SPARQL results and the browser are a drop-in match. */
  .table-scroll { overflow-x: auto; overflow-y: auto; position: relative; }
  table { border-collapse: collapse; width: 100%; table-layout: fixed; }
  th {
    background: #f8fafc; font-size: 0.68rem; font-weight: 700;
    text-transform: uppercase; color: #94a3b8; letter-spacing: 0.5px;
    padding: 0.5rem 0.75rem; border-bottom: 2px solid #e2e8f0;
    position: sticky; top: 0; z-index: 1; text-align: left; white-space: nowrap;
  }
  /* A term is an identifier, and half of one is not a shorter identifier — it
     is the wrong one. So nothing here is trimmed: a term too wide for its
     column wraps onto another line inside it. The column widths stay (the
     table is `table-layout: fixed`, so they are what keeps four columns on
     screen without a horizontal scrollbar); it is the clipping that goes.
     `anywhere` rather than `break-word` because an IRI has no spaces to break
     at, and a row of terms is read top-aligned once they are several lines. */
  td {
    padding: 0.4rem 0.75rem; border-bottom: 1px solid #f0f0f0;
    vertical-align: top; max-width: 280px;
    overflow: visible; text-overflow: clip;
    white-space: normal; overflow-wrap: anywhere;
  }
  .triple-row:hover td { background: #f8faff; }

  /* A row of [term | copy]: the copy button keeps its place at the end of the
     cell, and stays on the first line when the term beside it wraps. */
  .cell { display: flex; align-items: flex-start; gap: 0.3rem; min-width: 0; }
  .cell-body {
    flex: 1 1 auto; min-width: 0;
    overflow: visible; text-overflow: clip; overflow-wrap: anywhere;
  }

  /* Quiet but present, in all four columns: the copy affordance used to be
     invisible until hover, which is why it was reported as missing. */
  .copy-iri {
    flex: 0 0 auto;
    display: inline-flex; align-items: center; justify-content: center;
    width: 20px; height: 20px; padding: 0;
    background: none; border: none; border-radius: 4px;
    color: #cbd5e1; cursor: pointer; opacity: 0.6;
    transition: color 0.1s, opacity 0.1s, background 0.1s;
  }
  .triple-row:hover .copy-iri { opacity: 1; }
  .copy-iri:hover, .copy-iri:focus-visible { color: #4a90d9; background: #e8f2fc; opacity: 1; }
  .copy-iri:focus-visible { outline: 2px solid #4a90d9; outline-offset: 1px; }
  .copy-iri.copied { color: #4caf50; opacity: 1; }

  .term-cell { max-width: 280px; }
  .pred-cell { max-width: 180px; }
  .graph-cell { max-width: 140px; }
  .actions-col-header { width: 30px; }
  .actions-col { width: 30px; padding: 0; text-align: center; }

  /* The predicate chip is a chip around a term, so it wraps with it rather
     than cutting it: a chip reading `dcat:byteSi…` names nothing. */
  .predicate {
    font-size: 0.78rem; font-weight: 600; padding: 1px 6px;
    border-radius: 4px; white-space: normal; display: inline-block;
    max-width: 100%; overflow: visible; text-overflow: clip;
    overflow-wrap: anywhere;
  }
  .graph-tag {
    font-size: 0.75rem; color: #888; background: #f0f0f0; padding: 1px 5px;
    border-radius: 3px; font-family: monospace; overflow-wrap: anywhere;
  }
  .graph-default { font-size: 0.75rem; color: #bbb; font-style: italic; }

  .row-action {
    background: none; border: none; cursor: pointer; color: #bbb;
    font-size: 0.82rem; padding: 2px 4px; border-radius: 3px;
    opacity: 0; transition: opacity 0.1s, color 0.1s;
    display: inline-flex; align-items: center;
  }
  .triple-row:hover .row-action { opacity: 1; }
  .row-action:hover { color: #4a90d9; background: #e8f2fc; }

  /* Staggered row entrance (keyframe `rowIn` is global, in app.css). The global
     prefers-reduced-motion rule collapses this to an instant paint. */
  .row-in {
    animation-name: rowIn;
    animation-duration: var(--dur-base);
    animation-timing-function: var(--ease-out);
    animation-fill-mode: both;
  }

  /* Subtle loading veil — the parent owns the spinner/state; we just dim. */
  .is-loading tbody { opacity: 0.5; transition: opacity 0.15s; }

  .empty-state {
    color: #888; text-align: center; padding: 2rem; font-size: 0.9rem;
  }

  /* ─── Dark mode ──────────────────────────────────────────────────────────── */
  /* Mirrors TripleBrowser's dark overrides (~2172-2178). */
  :global(:is([data-theme="dark"], .dark)) th { background: var(--bg-strong); border-bottom-color: var(--line-strong); }
  :global(:is([data-theme="dark"], .dark)) td { border-bottom-color: var(--line-soft); }
  :global(:is([data-theme="dark"], .dark)) .triple-row:hover td { background: rgba(126,214,208,0.06); }
  :global(:is([data-theme="dark"], .dark)) .graph-tag { color: var(--ink-600); background: rgba(255,255,255,0.06); }
  :global(:is([data-theme="dark"], .dark)) .graph-default { color: var(--ink-500); }
  :global(:is([data-theme="dark"], .dark)) .copy-iri { color: var(--ink-500); }
  :global(:is([data-theme="dark"], .dark)) .copy-iri:hover,
  :global(:is([data-theme="dark"], .dark)) .copy-iri:focus-visible { color: #60a5fa; background: rgba(59,130,246,0.15); }
  :global(:is([data-theme="dark"], .dark)) .copy-iri.copied { color: #5fd39a; }
  :global(:is([data-theme="dark"], .dark)) .row-action { color: var(--ink-500); }
  :global(:is([data-theme="dark"], .dark)) .row-action:hover { color: #60a5fa; background: rgba(59,130,246,0.15); }
  :global(:is([data-theme="dark"], .dark)) .empty-state { color: var(--ink-600); }
</style>
