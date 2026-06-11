<script lang="ts">
  // A header card for the ontology viewer that answers "what is this, and what
  // standards is it built on?" — surfacing (1) the ontology's own metadata
  // (title / description / version / license / creator, read straight from the
  // owl:Ontology node) and (2) the recognised standard vocabularies it uses,
  // with their human title, description and spec link (from VOCAB_INFO, which
  // was defined but never shown anywhere before). Collapsible so it never
  // crowds out the term tables.
  import { DataFactory } from 'n3';
  import { ChevronDown, ExternalLink, BookOpen, Boxes } from 'lucide-svelte';
  import { shortenIRI } from '../../lib/rdf-utils';
  import { NAMESPACES, VOCAB_INFO } from '../../lib/ontology/vocabularies';
  import type { SchemaModel } from '../../lib/ontology/schema-model';

  export let model: SchemaModel | null = null;
  /** The parsed n3 Store backing `model` — used to read the owl:Ontology node. */
  export let store: any = null;
  export let graphIri: string = '';
  export let versionLabel: string = '';

  const { namedNode } = DataFactory;
  const RDF = 'http://www.w3.org/1999/02/22-rdf-syntax-ns#';
  const RDFS = 'http://www.w3.org/2000/01/rdf-schema#';
  const OWL = 'http://www.w3.org/2002/07/owl#';
  const SKOS = 'http://www.w3.org/2004/02/skos/core#';
  const DCT = 'http://purl.org/dc/terms/';
  const DC = 'http://purl.org/dc/elements/1.1/';
  const FOAF = 'http://xmlns.com/foaf/0.1/';
  const SCHEMA = 'http://schema.org/';

  // namespace IRI → registered prefix (for the VOCAB_INFO lookup)
  const NS_TO_PREFIX: Record<string, string> = Object.fromEntries(
    Object.entries(NAMESPACES).map(([p, iri]) => [iri as string, p]),
  );

  let expanded = true;

  /** First label/value for a subject across a preference-ordered predicate list,
   *  favouring an English literal when several languages are present. */
  function firstLit(s: any, preds: string[]): string {
    if (!store || !s) return '';
    for (const p of preds) {
      let objs: any[] = [];
      try {
        objs = store.getObjects(s, namedNode(p), null);
      } catch {
        objs = [];
      }
      if (!objs.length) continue;
      const en = objs.find((o) => o.language === 'en' || o.language === 'en-US');
      return (en || objs[0]).value;
    }
    return '';
  }

  function extractOntoMeta() {
    if (!store) return null;
    let subj: any = null;
    try {
      const onts = store.getSubjects(namedNode(RDF + 'type'), namedNode(OWL + 'Ontology'), null);
      if (onts.length) subj = onts[0];
    } catch {
      subj = null;
    }
    if (!subj && graphIri) subj = namedNode(graphIri);
    if (!subj) return null;

    const meta = {
      iri: subj.termType === 'NamedNode' ? subj.value : '',
      title: firstLit(subj, [DCT + 'title', DC + 'title', RDFS + 'label', SKOS + 'prefLabel']),
      description: firstLit(subj, [
        DCT + 'description', DC + 'description', RDFS + 'comment', SKOS + 'definition',
      ]),
      version: firstLit(subj, [OWL + 'versionInfo', DCT + 'hasVersion', SCHEMA + 'version']),
      license: firstLit(subj, [DCT + 'license', SCHEMA + 'license']),
      creator: firstLit(subj, [DCT + 'creator', DCT + 'publisher', DC + 'creator', FOAF + 'maker']),
      modified: firstLit(subj, [DCT + 'modified', DCT + 'issued', DCT + 'date']),
      homepage: firstLit(subj, [FOAF + 'homepage', SCHEMA + 'url']),
    };
    const hasAny = meta.title || meta.description || meta.version || meta.license || meta.creator || meta.iri;
    return hasAny ? meta : null;
  }

  function isUrl(v: string): boolean {
    return /^https?:\/\//i.test(v);
  }

  $: onto = store ? extractOntoMeta() : null;

  // Recognised standards in use, richest-first, deduped, with their VOCAB_INFO.
  $: standards = model
    ? [...model.namespaces.values()]
        .map((n) => ({ entry: n, info: VOCAB_INFO[NS_TO_PREFIX[n.ns]] }))
        .filter((x) => !!x.info)
        .sort((a, b) => (b.entry.count || 0) - (a.entry.count || 0))
    : [];

  $: counts = model
    ? [
        { label: 'Classes', n: model.classes.size },
        { label: 'Properties', n: model.properties.size },
        { label: 'Shapes', n: model.shapes.size },
        { label: 'Concepts', n: model.concepts.size },
        { label: 'Namespaces', n: model.namespaces.size },
      ].filter((c) => c.n > 0)
    : [];

  $: heading = onto?.title || (graphIri ? shortenIRI(graphIri) : 'Ontology');
</script>

{#if model}
  <section class="oih" class:collapsed={!expanded}>
    <button class="oih-bar" on:click={() => (expanded = !expanded)} aria-expanded={expanded}>
      <BookOpen size={16} class="oih-bar-icon" />
      <span class="oih-title">{heading}</span>
      {#if versionLabel}<span class="oih-pill">v{versionLabel}</span>{/if}
      {#if onto?.version}<span class="oih-pill ghost">{onto.version}</span>{/if}
      <span class="oih-counts">
        {#each counts as c}<span class="oih-count"><strong>{c.n.toLocaleString()}</strong> {c.label}</span>{/each}
      </span>
      <ChevronDown size={16} class="oih-chev" />
    </button>

    {#if expanded}
      <div class="oih-body">
        {#if onto?.description}
          <p class="oih-desc">{onto.description}</p>
        {/if}

        {#if onto && (onto.iri || onto.license || onto.creator || onto.modified || onto.homepage)}
          <div class="oih-meta">
            {#if onto.iri}
              <span class="oih-meta-item" title={onto.iri}><span class="k">IRI</span> <code>{shortenIRI(onto.iri)}</code></span>
            {/if}
            {#if onto.creator}<span class="oih-meta-item"><span class="k">By</span> {onto.creator}</span>{/if}
            {#if onto.modified}<span class="oih-meta-item"><span class="k">Updated</span> {onto.modified.slice(0, 10)}</span>{/if}
            {#if onto.license}
              <span class="oih-meta-item"><span class="k">License</span>
                {#if isUrl(onto.license)}<a href={onto.license} target="_blank" rel="noopener noreferrer">{shortenIRI(onto.license)} <ExternalLink size={11} /></a>{:else}{onto.license}{/if}
              </span>
            {/if}
            {#if onto.homepage && isUrl(onto.homepage)}
              <a class="oih-meta-item link" href={onto.homepage} target="_blank" rel="noopener noreferrer"><span class="k">Home</span> {shortenIRI(onto.homepage)} <ExternalLink size={11} /></a>
            {/if}
          </div>
        {/if}

        {#if standards.length}
          <div class="oih-std-head"><Boxes size={13} /> Built on {standards.length} standard{standards.length === 1 ? '' : 's'}</div>
          <div class="oih-std-grid">
            {#each standards as s}
              <div class="oih-std" title={s.entry.ns}>
                <div class="oih-std-top">
                  <span class="oih-std-name">{s.info.title}</span>
                  {#if s.entry.prefix}<code class="oih-std-prefix">{s.entry.prefix}:</code>{/if}
                  <span class="oih-std-count">{s.entry.count}</span>
                  {#if s.info.homepage}
                    <a class="oih-std-spec" href={s.info.homepage} target="_blank" rel="noopener noreferrer" title="Open specification">spec <ExternalLink size={10} /></a>
                  {/if}
                </div>
                <p class="oih-std-desc">{s.info.description}</p>
              </div>
            {/each}
          </div>
        {/if}
      </div>
    {/if}
  </section>
{/if}

<style>
  .oih {
    border: 1px solid var(--line-soft, #e2e8f0);
    border-radius: 14px;
    background: linear-gradient(180deg, rgba(31,152,151,0.06), rgba(31,152,151,0.01));
    overflow: hidden;
  }
  .oih-bar {
    width: 100%;
    display: flex;
    align-items: center;
    gap: 0.55rem;
    padding: 0.6rem 0.85rem;
    background: none;
    border: none;
    cursor: pointer;
    text-align: left;
    color: var(--ink-900, #18222b);
  }
  :global(.oih-bar-icon) { color: var(--brand-600, #167c80); flex: none; }
  .oih-title { font-weight: 700; font-size: 0.98rem; }
  .oih-pill {
    font-size: 0.68rem; font-weight: 700; padding: 1px 8px; border-radius: 999px;
    background: var(--brand-600, #167c80); color: #fff;
  }
  .oih-pill.ghost { background: rgba(21,58,67,0.08); color: var(--ink-700, #475661); }
  .oih-counts { display: flex; gap: 0.7rem; margin-left: auto; flex-wrap: wrap; }
  .oih-count { font-size: 0.72rem; color: var(--ink-500, #75828d); white-space: nowrap; }
  .oih-count strong { color: var(--ink-900, #18222b); font-size: 0.8rem; }
  :global(.oih-chev) { color: var(--ink-500, #75828d); flex: none; transition: transform var(--dur-base, 220ms) var(--ease-out, ease); }
  .oih.collapsed :global(.oih-chev) { transform: rotate(-90deg); }

  .oih-body { padding: 0 0.85rem 0.85rem; display: flex; flex-direction: column; gap: 0.6rem; animation: otsFadeIn var(--dur-base, 220ms) var(--ease-out, ease); }
  .oih-desc { margin: 0; font-size: 0.86rem; line-height: 1.5; color: var(--ink-700, #475661); max-width: 80ch; }

  .oih-meta { display: flex; flex-wrap: wrap; gap: 0.4rem 0.9rem; font-size: 0.76rem; color: var(--ink-700, #475661); }
  .oih-meta-item { display: inline-flex; align-items: center; gap: 0.3rem; }
  .oih-meta-item .k { font-size: 0.62rem; text-transform: uppercase; letter-spacing: 0.5px; color: var(--ink-500, #75828d); font-weight: 700; }
  .oih-meta-item code, .oih-std-prefix { font-family: var(--font-mono, monospace); font-size: 0.72rem; }
  .oih-meta-item a, .oih-meta-item.link { color: var(--brand-600, #167c80); text-decoration: none; display: inline-flex; align-items: center; gap: 0.2rem; }
  .oih-meta-item a:hover { text-decoration: underline; }

  .oih-std-head { display: flex; align-items: center; gap: 0.35rem; font-size: 0.72rem; font-weight: 700; text-transform: uppercase; letter-spacing: 0.5px; color: var(--ink-500, #75828d); margin-top: 0.1rem; }
  .oih-std-grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(240px, 1fr)); gap: 0.5rem; }
  .oih-std { border: 1px solid var(--line-soft, #e2e8f0); border-radius: 10px; padding: 0.5rem 0.6rem; background: var(--bg-strong, #fff); }
  .oih-std-top { display: flex; align-items: center; gap: 0.4rem; flex-wrap: wrap; }
  .oih-std-name { font-weight: 700; font-size: 0.82rem; color: var(--ink-900, #18222b); }
  .oih-std-prefix { color: var(--brand-600, #167c80); }
  .oih-std-count { font-size: 0.66rem; font-weight: 700; color: var(--ink-500, #75828d); background: rgba(21,58,67,0.06); padding: 0 0.4rem; border-radius: 999px; }
  .oih-std-spec { margin-left: auto; font-size: 0.68rem; color: var(--brand-600, #167c80); text-decoration: none; display: inline-flex; align-items: center; gap: 0.15rem; }
  .oih-std-spec:hover { text-decoration: underline; }
  .oih-std-desc { margin: 0.3rem 0 0; font-size: 0.74rem; line-height: 1.4; color: var(--ink-700, #475661); }

  :global(:is([data-theme="dark"], .dark)) .oih { border-color: var(--line-strong); background: linear-gradient(180deg, rgba(126,214,208,0.07), rgba(126,214,208,0.01)); }
  :global(:is([data-theme="dark"], .dark)) .oih-title,
  :global(:is([data-theme="dark"], .dark)) .oih-count strong,
  :global(:is([data-theme="dark"], .dark)) .oih-std-name { color: var(--ink-900); }
  :global(:is([data-theme="dark"], .dark)) .oih-std { background: var(--bg-strong); border-color: var(--line-strong); }
  :global(:is([data-theme="dark"], .dark)) .oih-pill.ghost { background: rgba(255,255,255,0.08); color: var(--ink-700); }
  :global(:is([data-theme="dark"], .dark)) .oih-std-count { background: rgba(255,255,255,0.08); }
</style>
