<script>
  // Predicate / edge info panel for the graph canvas. Opens when an edge is
  // tapped and explains the *predicate* (the relationship) rather than a node:
  // its short + full IRI, the vocabulary it comes from, and — when we can find
  // it — a human description and the domain/range inferred from the two
  // endpoints in the live graph.
  //
  // It deliberately mirrors the node inspector's look (insp-* classes are
  // re-implemented here as edge-* so the two panels feel like siblings) and is
  // positioned the same way, so only one of them is ever visible at a time
  // (GraphCanvas guarantees mutual exclusivity).
  //
  // IMPORTANT: this component only *imports* from the ontology lib helpers; it
  // never mutates them. It relies on their current exports:
  //   vocabularies.ts → NAMESPACES, VOCAB, kindOf
  //   prefixService.ts → prefixForNamespace
  import { createEventDispatcher } from 'svelte';
  import { X, Copy, Check, ArrowUpRight, ArrowRight, ArrowDownLeft, BookOpen } from 'lucide-svelte';
  import { shortenIRI } from '../lib/rdf-utils.js';
  import { NAMESPACES, VOCAB, kindOf } from '../lib/ontology/vocabularies.js';
  import { prefixForNamespace } from '../lib/ontology/prefixService.js';
  import { t } from 'svelte-i18n';

  /**
   * @typedef {Object} EdgeModel
   * @property {string} id
   * @property {string} predicate   full predicate IRI
   * @property {string} label       short predicate label (e.g. rdfs:label)
   * @property {{label:string, iri:string|null, rdfType:string|null, nodeType:string}} source
   * @property {{label:string, iri:string|null, rdfType:string|null, nodeType:string}} target
   */
  /** @type {EdgeModel | null} */
  export let edge = null;

  const dispatch = createEventDispatcher();

  let copied = false;
  function copy(s) {
    try {
      navigator.clipboard?.writeText(String(s ?? ''));
      copied = true;
      setTimeout(() => (copied = false), 1200);
    } catch { /* clipboard unavailable */ }
  }

  // Split an IRI into (namespace, local) at the last # or / — same rule the
  // rest of the app uses, kept local so we don't depend on a non-guaranteed
  // export from the lib.
  function splitNs(iri) {
    if (!iri) return { ns: '', local: '' };
    const h = iri.lastIndexOf('#');
    const s = iri.lastIndexOf('/');
    const i = Math.max(h, s);
    if (i < 0) return { ns: '', local: iri };
    return { ns: iri.slice(0, i + 1), local: iri.slice(i + 1) };
  }

  // Find the curated VOCAB term for this predicate (gives us comment + kind),
  // looking it up by exact IRI across every known vocabulary list.
  function findVocabTerm(iri) {
    if (!iri) return null;
    for (const list of Object.values(VOCAB)) {
      for (const term of list) if (term.iri === iri) return term;
    }
    return null;
  }

  // Human label for a node's rdf:type/kind, used for the domain/range hints.
  function endpointType(ep) {
    if (!ep) return null;
    if (ep.rdfType) return ep.rdfType;            // e.g. "owl:Class" — raw data
    if (ep.nodeType === 'literal') return '@literal'; // i18n token, see drLabel
    if (ep.nodeType === 'bnode') return '@bnode';
    return null;                                   // plain URI with no asserted type
  }

  // Built model — recomputed whenever the tapped edge changes.
  $: model = buildModel(edge);

  function buildModel(e) {
    if (!e) return null;
    const iri = e.predicate || '';
    const { ns, local } = splitNs(iri);
    // prefix: prefer the cached/seed reverse map, then the static NAMESPACES
    // table, else fall back to whatever shortenIRI produced.
    let prefix = prefixForNamespace(ns);
    if (!prefix) {
      for (const [p, nsIri] of Object.entries(NAMESPACES)) if (nsIri === ns) { prefix = p; break; }
    }
    const short = e.label || shortenIRI(iri) || local || iri;
    const term = findVocabTerm(iri);
    const kind = kindOf(iri); // 'rdfs' | 'owl' | … | 'custom'
    return {
      iri,
      short,
      local,
      ns,
      prefix,
      kind,
      comment: term?.comment || '',
      termKind: term?.kind || '',   // 'object' | 'datatype' | 'annotation' | …
      // Inferred from the actual endpoints in the graph (best-effort, not the
      // ontology's declared rdfs:domain/range — we say so in the UI).
      domain: endpointType(e.source),
      range: endpointType(e.target),
      sourceLabel: e.source?.label || '',
      targetLabel: e.target?.label || '',
    };
  }

  // i18n key for the predicate's role, from the curated term kind.
  function roleKey(termKind) {
    switch (termKind) {
      case 'object': return 'components.edgeInfo.roleObject';
      case 'datatype': return 'components.edgeInfo.roleDatatype';
      case 'annotation': return 'components.edgeInfo.roleAnnotation';
      case 'property': return 'components.edgeInfo.roleProperty';
      case 'class': return 'components.edgeInfo.roleClass';
      default: return 'components.edgeInfo.rolePredicate';
    }
  }
  // Translate a domain/range value: raw rdf:type IRIs pass through unchanged; the
  // literal / blank-node tokens from endpointType resolve to localized labels.
  function drLabel(v, tr) {
    if (!v) return '—';
    if (v === '@literal') return tr('components.edgeInfo.literal');
    if (v === '@bnode') return tr('components.edgeInfo.blankNode');
    return v;
  }

  // Whether the namespace looks like a recognised vocabulary (not 'custom').
  $: knownVocab = model && model.kind !== 'custom';
</script>

{#if model}
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="edge-panel" on:click|stopPropagation on:keydown|stopPropagation>
    <div class="edge-header">
      <span class="edge-type">{$t('components.edgeInfo.predicateBadge')}</span>
      <button class="edge-close" on:click={() => dispatch('close')} aria-label={$t('components.edgeInfo.closePanel')} title={$t('components.edgeInfo.closePanel')}>
        <X size={14} />
      </button>
    </div>

    <div class="edge-body">
      <h4 class="edge-title">{model.short}</h4>

      <p class="edge-explain">{$t('components.edgeInfo.explain')}</p>

      <!-- subject ─pred→ object, using the live endpoints -->
      {#if model.sourceLabel || model.targetLabel}
        <div class="edge-triple">
          <span class="edge-node">{model.sourceLabel || '?'}</span>
          <span class="edge-arrow"><ArrowRight size={12} /> {model.short}</span>
          <span class="edge-node">{model.targetLabel || '?'}</span>
        </div>
      {/if}

      {#if model.iri}
        <div class="edge-iri-row">
          <code class="edge-iri" title={model.iri}>{model.iri}</code>
          <button class="edge-mini-btn" on:click={() => copy(model.iri)} title={$t('components.edgeInfo.copyIri')}>
            {#if copied}<Check size={12} />{:else}<Copy size={12} />{/if}
          </button>
        </div>
      {/if}

      <!-- Vocabulary / namespace -->
      <div class="edge-meta-row">
        {#if model.prefix}
          <span class="edge-chip vocab" title={model.ns}>{model.prefix}</span>
        {/if}
        {#if model.termKind}
          <span class="edge-chip">{$t(roleKey(model.termKind))}</span>
        {/if}
      </div>

      {#if model.comment}
        <div class="edge-section">
          <div class="edge-section-head"><BookOpen size={12} /> {$t('components.edgeInfo.description')}</div>
          <p class="edge-desc">{model.comment}</p>
        </div>
      {/if}

      <!-- Vocabulary block -->
      <div class="edge-section">
        <div class="edge-section-head">{$t('components.edgeInfo.vocabulary')}</div>
        {#if knownVocab && model.prefix}
          <p class="edge-vocab-line">
            <strong>{model.prefix}</strong> — <code class="edge-ns" title={model.ns}>{model.ns}</code>
          </p>
        {:else if model.ns}
          <p class="edge-vocab-line edge-vocab-custom">
            {$t('components.edgeInfo.customNamespace')}
            <code class="edge-ns" title={model.ns}>{model.ns}</code>
          </p>
        {:else}
          <p class="edge-empty">{$t('components.edgeInfo.noNamespace')}</p>
        {/if}
      </div>

      <!-- Domain / range inferred from the connected nodes -->
      {#if model.domain || model.range}
        <div class="edge-section">
          <div class="edge-section-head">{$t('components.edgeInfo.domainRange')} <span class="edge-hint-tag">{$t('components.edgeInfo.inferred')}</span></div>
          <div class="edge-dr">
            <div class="edge-dr-row">
              <span class="edge-dr-key"><ArrowDownLeft size={11} /> {$t('components.edgeInfo.domain')}</span>
              <span class="edge-dr-val">{drLabel(model.domain, $t)}</span>
            </div>
            <div class="edge-dr-row">
              <span class="edge-dr-key"><ArrowUpRight size={11} /> {$t('components.edgeInfo.range')}</span>
              <span class="edge-dr-val">{drLabel(model.range, $t)}</span>
            </div>
          </div>
          <p class="edge-hint">{$t('components.edgeInfo.inferredHint')}</p>
        </div>
      {/if}

      {#if model.iri}
        <button class="edge-open-btn" on:click={() => dispatch('openPredicate', { iri: model.iri, label: model.short })}>
          <ArrowUpRight size={13} /> {$t('components.edgeInfo.openPredicate')}
        </button>
      {/if}
    </div>
  </div>
{/if}

<style>
  /* Mirrors the node inspector (.inspector-panel) so the two read as siblings. */
  .edge-panel {
    position: absolute;
    top: 52px;
    left: 10px;
    width: 320px;
    max-width: calc(100% - 80px);
    max-height: calc(100% - 110px);
    display: flex;
    flex-direction: column;
    background: rgba(255, 255, 255, 0.98);
    border: 1px solid #e2e8f0;
    border-radius: 10px;
    box-shadow: 0 6px 24px rgba(0, 0, 0, 0.16);
    z-index: 25;
    overflow: hidden;
  }

  .edge-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 8px 8px 8px 12px;
    background: #f8fafc;
    border-bottom: 1px solid #eef2f7;
    flex-shrink: 0;
  }

  .edge-type {
    font-size: 9px;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    padding: 2px 7px;
    border-radius: 4px;
    color: #fff;
    /* edge accent = pink/magenta, matching the predicate palette used for nodes */
    background: #be185d;
  }

  .edge-close {
    width: 24px; height: 24px; padding: 0;
    border: none; background: transparent; cursor: pointer;
    color: #94a3b8; border-radius: 6px;
    display: flex; align-items: center; justify-content: center;
    transition: background 0.12s, color 0.12s;
  }
  .edge-close:hover { background: #fee2e2; color: #dc2626; }

  .edge-body {
    padding: 10px 12px 12px;
    overflow-y: auto;
  }

  .edge-title {
    margin: 0;
    font-size: 0.92rem;
    font-weight: 700;
    color: #1e293b;
    word-break: break-word;
    line-height: 1.3;
  }

  .edge-explain {
    margin: 6px 0 0;
    font-size: 0.72rem;
    line-height: 1.45;
    color: #64748b;
  }

  .edge-triple {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 6px;
    margin-top: 9px;
    padding: 7px 8px;
    background: #fdf2f8;
    border: 1px solid #fbcfe8;
    border-radius: 7px;
  }
  .edge-node {
    font-size: 0.72rem;
    font-weight: 600;
    color: #334155;
    background: #fff;
    border: 1px solid #e2e8f0;
    border-radius: 5px;
    padding: 2px 6px;
    max-width: 110px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .edge-arrow {
    display: inline-flex;
    align-items: center;
    gap: 3px;
    font-size: 0.68rem;
    font-weight: 700;
    color: #be185d;
  }

  .edge-iri-row { display: flex; align-items: center; gap: 4px; margin-top: 9px; }
  .edge-iri {
    flex: 1; min-width: 0;
    font-family: 'IBM Plex Mono', monospace;
    font-size: 0.68rem;
    color: #475569;
    background: #f1f5f9;
    border-radius: 5px;
    padding: 4px 6px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .edge-mini-btn {
    flex-shrink: 0;
    width: 24px; height: 24px; padding: 0;
    border: 1px solid #e2e8f0;
    background: #fff;
    color: #64748b;
    border-radius: 5px;
    cursor: pointer;
    display: inline-flex; align-items: center; justify-content: center;
    transition: background 0.12s, border-color 0.12s, color 0.12s;
  }
  .edge-mini-btn:hover { background: #fdf2f8; border-color: #f9a8d4; color: #be185d; }

  .edge-meta-row { display: flex; flex-wrap: wrap; align-items: center; gap: 5px; margin-top: 8px; }
  .edge-chip {
    font-size: 0.68rem; font-weight: 600;
    padding: 1px 7px; border-radius: 8px;
    background: #e2e8f0; color: #475569;
  }
  .edge-chip.vocab { background: #fce7f3; color: #be185d; font-family: 'IBM Plex Mono', monospace; }

  .edge-section { margin-top: 13px; }
  .edge-section-head {
    display: flex; align-items: center; gap: 5px;
    font-size: 0.7rem; font-weight: 700;
    text-transform: uppercase; letter-spacing: 0.4px;
    color: #94a3b8;
    margin-bottom: 7px;
  }
  .edge-hint-tag {
    font-size: 0.58rem; font-weight: 700;
    text-transform: uppercase; letter-spacing: 0.4px;
    color: #a16207; background: #fef9c3;
    border-radius: 6px; padding: 0 5px;
  }

  .edge-desc {
    margin: 0;
    font-size: 0.78rem;
    line-height: 1.5;
    color: #334155;
  }

  .edge-vocab-line { margin: 0; font-size: 0.74rem; color: #475569; line-height: 1.5; }
  .edge-vocab-custom { color: #64748b; display: flex; flex-direction: column; gap: 4px; }
  .edge-ns {
    font-family: 'IBM Plex Mono', monospace;
    font-size: 0.66rem;
    color: #475569;
    background: #f1f5f9;
    border-radius: 4px;
    padding: 1px 5px;
    word-break: break-all;
  }

  .edge-dr { display: flex; flex-direction: column; gap: 6px; }
  .edge-dr-row { display: flex; align-items: baseline; gap: 8px; }
  .edge-dr-key {
    display: inline-flex; align-items: center; gap: 3px;
    flex-shrink: 0; width: 64px;
    font-size: 0.68rem; font-weight: 700;
    text-transform: uppercase; letter-spacing: 0.3px;
    color: #94a3b8;
  }
  .edge-dr-val { font-size: 0.78rem; font-weight: 600; color: #1e293b; word-break: break-word; }

  .edge-hint { font-size: 0.68rem; color: #b4bdca; margin: 6px 0 0; line-height: 1.4; }
  .edge-empty { font-size: 0.74rem; color: #94a3b8; margin: 0; }

  .edge-open-btn {
    display: inline-flex; align-items: center; gap: 5px;
    margin-top: 13px;
    padding: 5px 10px;
    font-size: 0.74rem; font-weight: 600;
    color: #be185d;
    background: #fce7f3;
    border: 1px solid #fbcfe8;
    border-radius: 6px;
    cursor: pointer;
    transition: background 0.12s, border-color 0.12s;
  }
  .edge-open-btn:hover { background: #fbcfe8; border-color: #f472b6; }

  /* ── Dark theme — same surfaces as the node inspector ── */
  :global(html.dark) .edge-panel { background: rgba(15, 23, 42, 0.98); border-color: rgba(255, 255, 255, 0.12); box-shadow: 0 6px 24px rgba(0, 0, 0, 0.55); }
  :global(html.dark) .edge-header { background: rgba(255, 255, 255, 0.03); border-bottom-color: rgba(255, 255, 255, 0.08); }
  :global(html.dark) .edge-close:hover { background: rgba(220, 38, 38, 0.2); color: #fca5a5; }
  :global(html.dark) .edge-title { color: #e2e8f0; }
  :global(html.dark) .edge-explain { color: #94a3b8; }
  :global(html.dark) .edge-triple { background: rgba(190, 24, 93, 0.12); border-color: rgba(236, 72, 153, 0.3); }
  :global(html.dark) .edge-node { background: #1e293b; border-color: rgba(255, 255, 255, 0.12); color: #cbd5e1; }
  :global(html.dark) .edge-arrow { color: #f9a8d4; }
  :global(html.dark) .edge-iri { background: #1e293b; color: #cbd5e1; }
  :global(html.dark) .edge-mini-btn { background: #1e293b; border-color: rgba(255, 255, 255, 0.12); color: #cbd5e1; }
  :global(html.dark) .edge-mini-btn:hover { background: rgba(190, 24, 93, 0.2); border-color: #ec4899; color: #f9a8d4; }
  :global(html.dark) .edge-chip { background: #1e293b; color: #cbd5e1; }
  :global(html.dark) .edge-chip.vocab { background: #4a1733; color: #f9a8d4; }
  :global(html.dark) .edge-section-head { color: #64748b; }
  :global(html.dark) .edge-hint-tag { background: rgba(202, 138, 4, 0.2); color: #fde047; }
  :global(html.dark) .edge-desc { color: #cbd5e1; }
  :global(html.dark) .edge-vocab-line { color: #cbd5e1; }
  :global(html.dark) .edge-vocab-custom { color: #94a3b8; }
  :global(html.dark) .edge-ns { background: #1e293b; color: #cbd5e1; }
  :global(html.dark) .edge-dr-key { color: #64748b; }
  :global(html.dark) .edge-dr-val { color: #e2e8f0; }
  :global(html.dark) .edge-hint { color: #475569; }
  :global(html.dark) .edge-empty { color: #64748b; }
  :global(html.dark) .edge-open-btn { background: rgba(190, 24, 93, 0.16); border-color: rgba(236, 72, 153, 0.4); color: #f9a8d4; }
  :global(html.dark) .edge-open-btn:hover { background: rgba(190, 24, 93, 0.26); }
</style>
