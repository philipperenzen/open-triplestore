<script lang="ts">
  // Instances of the selected class: those inside the loaded model graph(s),
  // read from the parsed store, and those in data the caller may read, looked
  // up over /sparql. The server scopes every /sparql query to the graphs the
  // caller is authorised for, so this lookup sees exactly what the caller
  // could query by hand and nothing more.
  import { onDestroy } from 'svelte';
  import { t } from 'svelte-i18n';
  import type { SchemaModel } from '../../lib/ontology/schema-model';
  import { classInstances } from '../../lib/ontology/class-details';
  import { sparqlQuery } from '../../lib/api';
  import { shortenIRI } from '../../lib/rdf-utils';
  import { isBetterLang } from '../../lib/ontology/termDisplay';
  import { Link } from '../../lib/router/index';

  export let model: SchemaModel;
  export let classIri: string;
  /** Graph passed along to the resource page for instances from the model. */
  export let resourceGraph: string = '';

  const PAGE = 50;

  // ---- instances inside the model ------------------------------------------
  let includeSubclasses = false;
  let shown = PAGE;
  let lastIri = '';
  $: if (classIri !== lastIri) {
    lastIri = classIri;
    shown = PAGE;
    includeSubclasses = false;
  }
  $: inModel = classInstances(model, classIri);
  $: hasSubclassInstances = inModel.withSubclasses.length > inModel.direct.length;
  $: modelList = includeSubclasses ? inModel.withSubclasses : inModel.direct;
  $: modelRemaining = Math.max(0, modelList.length - shown);

  const label = (iri: string) => model.labels.get(iri) || '';
  const modelHref = (iri: string) => {
    const qs = new URLSearchParams({ iri });
    if (resourceGraph) qs.set('graph', resourceGraph);
    return `/resource?${qs.toString()}`;
  };
  const dataHref = (iri: string) => `/resource?iri=${encodeURIComponent(iri)}`;

  // ---- instances in accessible data ----------------------------------------
  type Row = { iri: string; label: string };
  let status: 'loading' | 'done' | 'error' = 'loading';
  let total = 0;
  let rows: Row[] = [];
  let seq = 0;
  let ctrl: AbortController | null = null;

  // Characters that cannot appear inside an IRIREF; a class IRI holding one
  // would break out of `<…>`, so it is never sent.
  const UNSAFE_IRI = /[\s<>"{}|^`\\]/;

  function query(iri: string): string {
    // One request: the total, plus the first PAGE subjects with their labels.
    return `PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
SELECT ?n ?s ?label WHERE {
  { SELECT (COUNT(DISTINCT ?x) AS ?n) WHERE { ?x a <${iri}> } }
  OPTIONAL {
    { SELECT DISTINCT ?s WHERE { ?s a <${iri}> . FILTER(isIRI(?s)) } LIMIT ${PAGE} }
    OPTIONAL { ?s rdfs:label ?label }
  }
}`;
  }

  async function lookup(iri: string) {
    ctrl?.abort();
    const mine = ++seq;
    const controller = new AbortController();
    ctrl = controller;
    rows = [];
    total = 0;
    if (!iri || UNSAFE_IRI.test(iri)) { status = 'error'; return; }
    status = 'loading';
    try {
      const res: any = await sparqlQuery(query(iri), { signal: controller.signal });
      // The selection moved on while this was in flight.
      if (mine !== seq) return;
      const bindings: any[] = res?.results?.bindings ?? [];
      const byIri = new Map<string, { label: string; lang: string | undefined }>();
      let n = 0;
      for (const b of bindings) {
        if (b.n?.value) n = Math.max(n, parseInt(b.n.value, 10) || 0);
        const s = b.s?.value;
        if (!s) continue;
        const prev = byIri.get(s);
        const lbl = b.label?.value ?? '';
        const lang = b.label?.['xml:lang'] ?? '';
        if (!prev) byIri.set(s, { label: lbl, lang: lbl ? lang : undefined });
        else if (lbl && (prev.lang === undefined || isBetterLang(lang, prev.lang))) byIri.set(s, { label: lbl, lang });
      }
      rows = [...byIri].map(([s, v]) => ({ iri: s, label: v.label }));
      total = Math.max(n, rows.length);
      status = 'done';
    } catch {
      if (mine === seq) status = 'error';
    }
  }

  $: lookup(classIri);

  onDestroy(() => { seq++; ctrl?.abort(); });
</script>

<section class="ci">
  <h4>{$t('components.classAxiomPanel.instances')}</h4>

  <h5>{$t('components.classAxiomPanel.instancesInModel')}</h5>
  {#if inModel.withSubclasses.length === 0}
    <p class="muted">{$t('components.classAxiomPanel.noInstancesModel')}</p>
  {:else}
    <p class="count">
      {$t('components.classAxiomPanel.directCount', { values: { count: inModel.direct.length } })}
      {#if hasSubclassInstances}
        · <label class="toggle">
          <input type="checkbox" bind:checked={includeSubclasses} />
          {$t('components.classAxiomPanel.includeSubclasses', { values: { count: inModel.withSubclasses.length } })}
        </label>
      {/if}
    </p>
    {#if modelList.length}
      <ul class="inst">
        {#each modelList.slice(0, shown) as iri (iri)}
          <li><Link to={modelHref(iri)} title={iri}>{label(iri) || shortenIRI(iri)}</Link>
            {#if label(iri)}<code class="curie">{shortenIRI(iri)}</code>{/if}</li>
        {/each}
      </ul>
      {#if modelRemaining > 0}
        <button type="button" class="more" on:click={() => (shown += PAGE)}>
          {$t('components.classAxiomPanel.showMore', { values: { count: modelRemaining } })}
        </button>
      {/if}
    {/if}
  {/if}

  <h5>{$t('components.classAxiomPanel.instancesInData')}</h5>
  <div aria-live="polite" class="data">
    {#if status === 'loading'}
      <p class="muted" role="status">{$t('components.classAxiomPanel.loadingInstances')}</p>
    {:else if status === 'error'}
      <p class="muted">{$t('components.classAxiomPanel.instancesError')}</p>
    {:else if total === 0}
      <p class="muted">{$t('components.classAxiomPanel.noInstancesData')}</p>
    {:else}
      <p class="count">{$t('components.classAxiomPanel.showingOf', { values: { shown: rows.length, total } })}</p>
      <ul class="inst">
        {#each rows as r (r.iri)}
          <li><Link to={dataHref(r.iri)} title={r.iri}>{r.label || shortenIRI(r.iri)}</Link>
            {#if r.label}<code class="curie">{shortenIRI(r.iri)}</code>{/if}</li>
        {/each}
      </ul>
    {/if}
  </div>
</section>

<style>
  .ci { border: 1px solid #f1f5f9; border-radius: 8px; padding: 0.4rem 0.55rem; background: #fbfdff; margin-top: 0.5rem; }
  .ci h4 { font-size: 0.78rem; font-weight: 700; color: #475569; margin: 0 0 0.25rem 0; }
  .ci h5 { font-size: 0.72rem; font-weight: 700; color: #64748b; margin: 0.35rem 0 0.15rem; text-transform: uppercase; letter-spacing: 0.03em; }
  .count { font-size: 0.76rem; color: #475569; margin: 0 0 0.2rem; }
  .toggle { display: inline-flex; align-items: center; gap: 0.25rem; cursor: pointer; }
  .inst { list-style: none; margin: 0; padding: 0; columns: 2; column-gap: 1rem; }
  .inst li { font-size: 0.8rem; padding: 0.08rem 0; break-inside: avoid; }
  .inst :global(a) { color: #2563eb; text-decoration: none; }
  .inst :global(a:hover) { text-decoration: underline; }
  .curie { font-size: 0.68rem; color: #94a3b8; margin-left: 0.3rem; }
  .muted { color: #94a3b8; font-size: 0.76rem; margin: 0.1rem 0; }
  .more { margin-top: 0.25rem; background: #eef5ff; color: #1565c0; border: 1px solid #bbdefb; padding: 0.12rem 0.5rem;
    border-radius: 6px; font-size: 0.72rem; cursor: pointer; }

  :global(:is([data-theme="dark"], .dark)) .ci { background: var(--bg-soft); border-color: var(--line-soft); }
  :global(:is([data-theme="dark"], .dark)) .ci h4,
  :global(:is([data-theme="dark"], .dark)) .ci h5,
  :global(:is([data-theme="dark"], .dark)) .count { color: var(--ink-600); }
  :global(:is([data-theme="dark"], .dark)) .inst :global(a) { color: #93c5fd; }
  :global(:is([data-theme="dark"], .dark)) .more { background: rgba(59,130,246,0.15); color: #93c5fd; border-color: rgba(59,130,246,0.3); }
</style>
