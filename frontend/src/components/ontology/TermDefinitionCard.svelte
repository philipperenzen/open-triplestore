<script>
  // One renderer for a vocabulary term's full linked-data definition, used by the
  // browser predicate popover, the graph edge/node detail, the ontology Property/
  // Class panels and the resource page. `variant='rich'` shows everything;
  // `variant='compact'` shows a header + one definition + a "More details" link.
  import { t, locale } from 'svelte-i18n';
  import { Sparkles } from 'lucide-svelte';
  import { shortenIRI } from '../../lib/rdf-utils.js';
  import { navigate } from '../../lib/router/index.js';
  import { langToFlag } from '../../lib/i18n/langFlag.js';
  import { pickLang, groupByLang } from '../../lib/ontology/termDisplay.js';
  import { lookupTerm, lookupTermSync } from '../../lib/ontology/termDictionary.js';
  import { openSparkExplain } from '../../lib/sparkHelp.js';

  /** @type {string} */
  export let iri = '';
  /** @type {'rich' | 'compact'} */
  export let variant = 'rich';
  /** @type {import('../../lib/ontology/termTypes').TermMeta | null} */
  export let meta = null;
  /** When true, render NOTHING (no skeleton, no placeholder) unless a bundled
   *  definition exists — for inline use next to custom, non-vocabulary terms. */
  export let hideEmpty = false;
  /** @type {(iri: string) => void} */
  export let onOpen = (i) => navigate(`/resource?iri=${encodeURIComponent(i)}`);

  let resolved = null;
  let loading = false;

  // Resolve metadata: a caller-supplied `meta` wins; otherwise look it up from the
  // bundled dictionary — synchronously if the file is already loaded (instant
  // paint), else asynchronously with a small skeleton. Guarded against IRI races.
  async function resolveMeta(i, supplied) {
    if (supplied) { resolved = supplied; loading = false; return; }
    if (!i) { resolved = null; loading = false; return; }
    const sync = lookupTermSync(i);
    if (sync !== undefined) { resolved = sync; loading = false; return; }
    resolved = null;
    loading = true;
    try {
      const m = await lookupTerm(i);
      if (i === iri) { resolved = m; loading = false; }
    } catch {
      if (i === iri) loading = false;
    }
  }
  $: resolveMeta(iri, meta);

  $: lang = ($locale || 'en').split('-')[0];

  // Short, friendly badge text + colour class per RDF term type.
  const TYPE_BADGE = {
    'owl:ObjectProperty': { key: 'object', cls: 'b-object' },
    'owl:DatatypeProperty': { key: 'datatype', cls: 'b-datatype' },
    'owl:AnnotationProperty': { key: 'annotation', cls: 'b-annotation' },
    'rdf:Property': { key: 'property', cls: 'b-property' },
    'owl:Class': { key: 'class', cls: 'b-class' },
    'rdfs:Class': { key: 'class', cls: 'b-class' },
    'skos:Concept': { key: 'concept', cls: 'b-concept' },
    'rdfs:Datatype': { key: 'datatype', cls: 'b-datatype' },
    'owl:NamedIndividual': { key: 'individual', cls: 'b-individual' },
    unknown: { key: 'term', cls: 'b-term' },
  };
  $: badge = resolved ? (TYPE_BADGE[resolved.termType] || TYPE_BADGE.unknown) : null;
  $: headLabel = resolved ? (pickLang(resolved.labels, lang) || shortenIRI(iri)) : shortenIRI(iri);
  $: compactDef = resolved ? (pickLang(resolved.definitions, lang) || pickLang(resolved.comments, lang)) : '';

  // Note sections (multi-language) and relationship sections (IRI lists), built
  // declaratively so the markup stays a single loop.
  $: noteSections = resolved ? [
    { label: $t('components.termDefinitionCard.definition'), values: resolved.definitions },
    { label: $t('components.termDefinitionCard.comment'), values: resolved.comments },
    { label: $t('components.termDefinitionCard.scopeNote'), values: resolved.scopeNotes },
    { label: $t('components.termDefinitionCard.example'), values: resolved.examples },
    { label: $t('components.termDefinitionCard.changeNote'), values: resolved.changeNotes },
    { label: $t('components.termDefinitionCard.editorialNote'), values: resolved.editorialNotes },
  ].filter((s) => s.values.length) : [];
  $: relSections = resolved ? [
    { label: 'rdfs:domain', iris: resolved.domain },
    { label: 'rdfs:range', iris: resolved.range },
    { label: 'rdfs:subPropertyOf', iris: resolved.subPropertyOf },
    { label: 'rdfs:subClassOf', iris: resolved.subClassOf },
    { label: 'owl:inverseOf', iris: resolved.inverseOf },
    { label: 'rdfs:isDefinedBy', iris: resolved.isDefinedBy },
    { label: 'rdfs:seeAlso', iris: resolved.seeAlso },
  ].filter((s) => s.iris.length) : [];
</script>

{#if loading && !hideEmpty}
  <div class="tdc tdc-{variant} tdc-skeleton" aria-busy="true">
    <div class="sk sk-a"></div><div class="sk sk-b"></div>
  </div>
{:else if resolved}
  <div class="tdc tdc-{variant}">
    <div class="tdc-head">
      <span class="tdc-label" title={iri}>{headLabel}</span>
      {#if badge}<span class="tdc-badge {badge.cls}">{$t(`components.termDefinitionCard.type.${badge.key}`)}</span>{/if}
      {#if resolved.deprecated}<span class="tdc-badge b-deprecated">{$t('components.termDefinitionCard.deprecated')}</span>{/if}
      <span class="tdc-src" title={$t('components.termDefinitionCard.source')}>{resolved.source}</span>
      <button
        class="tdc-spark"
        on:click|stopPropagation={() => openSparkExplain({ iri, label: headLabel })}
        title={$t('components.termDefinitionCard.askSpark')}
        aria-label={$t('components.termDefinitionCard.askSpark')}
      ><Sparkles size={13} /></button>
    </div>
    <div class="tdc-iri">{shortenIRI(iri)}</div>

    {#if variant === 'compact'}
      {#if compactDef}<p class="tdc-compact-def">{compactDef}</p>{/if}
      <button class="tdc-more" on:click|stopPropagation={() => onOpen(iri)}>{$t('components.termDefinitionCard.moreDetails')}</button>
    {:else}
      {#each noteSections as sec}
        <section class="tdc-sec">
          <h5 class="tdc-sec-h">{sec.label}</h5>
          {#each groupByLang(sec.values, lang) as v}
            <p class="tdc-val">
              {#if v.lang}<span class="tdc-lang" title={v.lang}>{langToFlag(v.lang)}{v.lang}</span>{/if}
              <span>{v.value}</span>
            </p>
          {/each}
        </section>
      {/each}

      {#if relSections.length}
        <div class="tdc-rels">
          {#each relSections as rel}
            <section class="tdc-rel">
              <h5 class="tdc-sec-h">{rel.label}</h5>
              <ul>
                {#each rel.iris as r}
                  <li><button class="tdc-link" on:click|stopPropagation={() => onOpen(r)} title={r}>{shortenIRI(r)}</button></li>
                {/each}
              </ul>
            </section>
          {/each}
        </div>
      {/if}

      {#if resolved.versionInfo.length}
        <div class="tdc-version">{$t('components.termDefinitionCard.version')}: {resolved.versionInfo.join(', ')}</div>
      {/if}
    {/if}
  </div>
{:else if variant === 'rich' && !hideEmpty}
  <div class="tdc tdc-rich tdc-empty">{$t('components.termDefinitionCard.noDefinition')}</div>
{/if}

<style>
  .tdc { font-size: 0.82rem; color: #1e293b; }
  .tdc-rich { display: flex; flex-direction: column; gap: 0.55rem; }
  .tdc-head { display: flex; align-items: center; flex-wrap: wrap; gap: 0.35rem; }
  .tdc-label { font-weight: 700; font-size: 0.95rem; color: #0f172a; }
  .tdc-iri { font-family: 'SF Mono', 'Fira Code', monospace; font-size: 0.72rem; color: #64748b; margin-top: -0.2rem; word-break: break-all; }
  .tdc-badge { font-size: 0.64rem; font-weight: 700; text-transform: uppercase; letter-spacing: 0.02em; padding: 1px 6px; border-radius: 999px; }
  .b-object { background: #dbeafe; color: #1d4ed8; }
  .b-datatype { background: #dcfce7; color: #15803d; }
  .b-annotation { background: #fef3c7; color: #92400e; }
  .b-property { background: #fce7f3; color: #be185d; }
  .b-class { background: #ede9fe; color: #6d28d9; }
  .b-concept { background: #e0f2fe; color: #0369a1; }
  .b-individual { background: #f1f5f9; color: #475569; }
  .b-term { background: #f1f5f9; color: #475569; }
  .b-deprecated { background: #fee2e2; color: #b91c1c; }
  .tdc-src { margin-left: auto; font-size: 0.64rem; font-weight: 700; color: #64748b; background: #f1f5f9; border-radius: 999px; padding: 1px 7px; text-transform: lowercase; }
  /* "Ask Spark" term helper — sits after the source pill, kept unobtrusive. */
  .tdc-spark { display: inline-flex; align-items: center; justify-content: center; padding: 2px; border: none; background: none; color: #7c5cff; cursor: pointer; border-radius: 6px; }
  .tdc-spark:hover { background: #ede9fe; color: #6d28d9; }

  .tdc-compact-def { margin: 0.35rem 0 0.3rem; color: #334155; line-height: 1.4; }
  .tdc-more { background: #eef5ff; color: #1565c0; border: 1px solid #bbdefb; padding: 0.18rem 0.55rem; border-radius: 6px; font-size: 0.72rem; cursor: pointer; }
  .tdc-more:hover { background: #e0ecff; }

  .tdc-sec-h { margin: 0 0 0.2rem; font-size: 0.68rem; font-weight: 700; text-transform: uppercase; letter-spacing: 0.03em; color: #64748b; }
  .tdc-sec { border-top: 1px solid #f1f5f9; padding-top: 0.4rem; }
  .tdc-val { margin: 0 0 0.25rem; line-height: 1.45; color: #334155; display: flex; gap: 0.4rem; }
  .tdc-lang { flex-shrink: 0; font-size: 0.66rem; font-weight: 700; color: #475569; background: #f1f5f9; border-radius: 4px; padding: 0 5px; height: fit-content; text-transform: uppercase; }

  .tdc-rels { display: grid; grid-template-columns: 1fr 1fr; gap: 0.4rem 0.7rem; border-top: 1px solid #f1f5f9; padding-top: 0.45rem; }
  .tdc-rel ul { list-style: none; margin: 0.1rem 0 0; padding: 0; }
  .tdc-rel li { padding: 0.05rem 0; }
  .tdc-link { background: none; border: none; color: #2563eb; cursor: pointer; padding: 0; font: inherit; font-size: 0.78rem; }
  .tdc-link:hover { text-decoration: underline; }
  .tdc-version { font-size: 0.7rem; color: #94a3b8; border-top: 1px solid #f1f5f9; padding-top: 0.35rem; }
  .tdc-empty { color: #94a3b8; font-style: italic; padding: 0.3rem 0; }

  .tdc-skeleton { display: flex; flex-direction: column; gap: 0.4rem; padding: 0.2rem 0; }
  .sk { height: 0.7rem; border-radius: 4px; background: linear-gradient(90deg, #eef2f7 25%, #e2e8f0 37%, #eef2f7 63%); background-size: 400% 100%; animation: sk 1.2s ease infinite; }
  .sk-a { width: 55%; } .sk-b { width: 80%; }
  @keyframes sk { 0% { background-position: 100% 50%; } 100% { background-position: 0 50%; } }

  /* Dark mode */
  :global(html.dark) .tdc { color: #e2e8f0; }
  :global(html.dark) .tdc-label { color: #f1f5f9; }
  :global(html.dark) .tdc-iri { color: #94a3b8; }
  :global(html.dark) .tdc-val { color: #cbd5e1; }
  :global(html.dark) .tdc-lang { color: #cbd5e1; background: #1e293b; }
  :global(html.dark) .tdc-src { color: #94a3b8; background: #1e293b; }
  :global(html.dark) .tdc-spark { color: #c4b5fd; }
  :global(html.dark) .tdc-spark:hover { background: rgba(124,58,237,0.22); color: #ddd6fe; }
  :global(html.dark) .tdc-sec, :global(html.dark) .tdc-rels, :global(html.dark) .tdc-version { border-top-color: #1e293b; }
  :global(html.dark) .tdc-more { background: rgba(59,130,246,0.15); color: #93c5fd; border-color: rgba(59,130,246,0.3); }
  :global(html.dark) .b-object { background: rgba(59,130,246,0.2); color: #93c5fd; }
  :global(html.dark) .b-datatype { background: rgba(16,185,129,0.18); color: #6ee7b7; }
  :global(html.dark) .b-annotation { background: rgba(245,158,11,0.18); color: #fcd34d; }
  :global(html.dark) .b-property { background: rgba(236,72,153,0.18); color: #f9a8d4; }
  :global(html.dark) .b-class { background: rgba(124,58,237,0.22); color: #c4b5fd; }
  :global(html.dark) .b-concept { background: rgba(2,132,199,0.22); color: #7dd3fc; }
  :global(html.dark) .b-deprecated { background: rgba(239,68,68,0.2); color: #fca5a5; }
</style>
