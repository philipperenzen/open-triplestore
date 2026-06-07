<script>
  // Shared, presentational data-table used by BOTH the SPARQL editor results
  // (mode="bindings") and the Triple Browser (mode="triples"). It only renders
  // the rows it is handed — PAGINATION, fetching and filtering stay with the
  // parent. The markup/CSS are lifted from TripleBrowser.svelte and
  // SparqlEditor.svelte so the two consumers stay pixel-consistent.
  import { createEventDispatcher } from 'svelte';
  import { shortenIRI, toNTriples } from '../lib/rdf-utils.js';
  import RdfTerm from './RdfTerm.svelte';
  import { Copy } from 'lucide-svelte';
  import { t } from 'svelte-i18n';

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
    if (typeof navigator !== 'undefined' && navigator.clipboard) {
      navigator.clipboard.writeText(line).catch(() => {});
    }
  }

  $: isEmpty = mode === 'triples'
    ? (!triples || triples.length === 0)
    : (!bindings || bindings.length === 0);
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
        {#each triples as tr}
          <tr class="triple-row">
            <td class="term-cell">
              <RdfTerm term={tr.subject} graph={tr.graph?.value || ''} />
            </td>
            <td class="pred-cell">
              <span
                class="predicate"
                title={tr.predicate?.value}
                style="color:{predicateColor(tr.predicate?.value)}; background:{predicateBg(tr.predicate?.value)}"
              >{shortenIRI(tr.predicate?.value || '')}</span>
            </td>
            <td class="term-cell">
              <RdfTerm term={tr.object} graph={tr.graph?.value || ''} />
            </td>
            <td class="graph-cell">
              {#if tr.graph?.value}
                <span class="graph-tag" title={tr.graph.value}>{shortenIRI(tr.graph.value)}</span>
              {:else}
                <span class="graph-default">{$t('components.dataTable.defaultGraph')}</span>
              {/if}
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
        {#each bindings as row}
          <tr class="triple-row">
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
  td {
    padding: 0.4rem 0.75rem; border-bottom: 1px solid #f0f0f0;
    vertical-align: middle; max-width: 280px;
    overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
  }
  .triple-row:hover td { background: #f8faff; }

  .term-cell { max-width: 280px; }
  .pred-cell { max-width: 180px; }
  .graph-cell { max-width: 140px; }
  .actions-col-header { width: 30px; }
  .actions-col { width: 30px; padding: 0; text-align: center; }

  .predicate {
    font-size: 0.78rem; font-weight: 600; padding: 1px 6px;
    border-radius: 4px; white-space: nowrap; display: inline-block;
    max-width: 100%; overflow: hidden; text-overflow: ellipsis;
  }
  .graph-tag { font-size: 0.75rem; color: #888; background: #f0f0f0; padding: 1px 5px; border-radius: 3px; font-family: monospace; }
  .graph-default { font-size: 0.75rem; color: #bbb; font-style: italic; }

  .row-action {
    background: none; border: none; cursor: pointer; color: #bbb;
    font-size: 0.82rem; padding: 2px 4px; border-radius: 3px;
    opacity: 0; transition: opacity 0.1s, color 0.1s;
    display: inline-flex; align-items: center;
  }
  .triple-row:hover .row-action { opacity: 1; }
  .row-action:hover { color: #4a90d9; background: #e8f2fc; }

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
  :global(:is([data-theme="dark"], .dark)) .row-action { color: var(--ink-500); }
  :global(:is([data-theme="dark"], .dark)) .row-action:hover { color: #60a5fa; background: rgba(59,130,246,0.15); }
  :global(:is([data-theme="dark"], .dark)) .empty-state { color: var(--ink-600); }
</style>
