<script>
  // Renders vocabulary annotation prose (rdfs:comment / skos:definition /
  // dct:description) with its light markup resolved: paragraphs, markdown and
  // bare links, and `[[term]]` references to sibling terms in the same
  // vocabulary. See lib/annotationText.ts for the grammar.
  //
  // No `{@html}` anywhere — every segment is rendered as its own element, so
  // annotation text can never inject markup.
  import { parseAnnotation, resolveTermRef } from '../../lib/annotationText.js';
  import { safeExternalUrl } from '../../lib/safeUrl.js';
  import { navigate } from '../../lib/router/index.js';

  /** The raw annotation literal. */
  export let text = '';

  /** IRI of the term this annotation describes — the base for `[[term]]` refs. */
  export let baseIri = '';

  /**
   * Optional handler for a `[[term]]` click, given the resolved IRI. Panels that
   * show terms in-place (TermDefinitionCard, the ontology browser) pass their own
   * navigation; without one we fall back to the resource detail route.
   * @type {((iri: string) => void) | null}
   */
  export let onOpenTerm = null;

  $: paragraphs = parseAnnotation(text);

  function openTerm(name) {
    const iri = resolveTermRef(name, baseIri);
    if (!iri) return;
    if (onOpenTerm) onOpenTerm(iri);
    else navigate(`/resource?iri=${encodeURIComponent(iri)}`);
  }
</script>

<div class="annotation">
  {#each paragraphs as para}
    <p class="ann-p">
      {#each para as seg}
        {#if seg.kind === 'text'}{seg.text}
        {:else if seg.kind === 'link'}
          <a class="ann-link" href={safeExternalUrl(seg.href)} target="_blank" rel="noopener noreferrer" title={seg.href}>{seg.text}</a>
        {:else}
          {#if resolveTermRef(seg.name, baseIri)}
            <button
              type="button"
              class="ann-term"
              title={resolveTermRef(seg.name, baseIri)}
              on:click|stopPropagation={() => openTerm(seg.name)}
            >{seg.text}</button>
          {:else}
            <span class="ann-term-plain" title={seg.name}>{seg.text}</span>
          {/if}
        {/if}
      {/each}
    </p>
  {/each}
</div>

<style>
  .annotation { display: block; }
  .ann-p {
    margin: 0 0 0.55rem;
    line-height: 1.55;
    white-space: pre-wrap;
  }
  .ann-p:last-child { margin-bottom: 0; }

  .ann-link {
    color: var(--brand-600, #2F7A8C);
    text-decoration: underline;
    text-underline-offset: 2px;
    overflow-wrap: anywhere;
  }
  .ann-link:hover { color: var(--brand-500, #3a95a6); }

  /* A sibling-term reference reads as a link but stays a button (no href). */
  .ann-term {
    display: inline;
    padding: 0;
    border: none;
    background: none;
    font: inherit;
    color: var(--brand-600, #2F7A8C);
    cursor: pointer;
    text-decoration: underline dotted;
    text-underline-offset: 2px;
  }
  .ann-term:hover { color: var(--brand-500, #3a95a6); text-decoration-style: solid; }
  .ann-term-plain { font-style: italic; }

  :global(html.dark) .ann-link,
  :global(html.dark) .ann-term { color: var(--brand-300, #7ED6D0); }
</style>
